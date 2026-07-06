use std::collections::HashMap;
use std::io::Write;
#[cfg(windows)]
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::sync::watch;

use crate::eventlog::EventLog;
use crate::policy::{evaluate, Policy};
use crate::protocol::{is_promptish, key_seq, meta_path, pipe_path};
use crate::screen::{Screen, Snapshot};

/// ConPTY needs a real executable path — resolve bare names via PATH/PATHEXT.
#[cfg(windows)]
pub fn resolve_executable(cmd: &str) -> String {
    if cmd.contains('/') || cmd.contains('\\') {
        return cmd.to_string();
    }
    let path = std::env::var("PATH").unwrap_or_default();
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    resolve_in_path(cmd, &path, &exts).unwrap_or_else(|| cmd.to_string())
}

/// PATH search honoring PATHEXT. A name without an executable extension is
/// never accepted as-is: npm installs a POSIX sh shim (`codex`) next to the
/// real `codex.cmd`, and CreateProcess cannot run the former — matching it
/// used to shadow the .cmd and break the spawn.
#[cfg(windows)]
fn resolve_in_path(cmd: &str, path: &str, pathext: &str) -> Option<String> {
    let exts: Vec<String> = pathext
        .split(';')
        .filter(|e| !e.is_empty())
        .map(|e| e.to_lowercase())
        .collect();
    let lower = cmd.to_lowercase();
    let runnable_as_is =
        exts.iter().any(|e| lower.ends_with(e.as_str())) || lower.ends_with(".ps1");
    for dir in path.split(';').filter(|d| !d.is_empty()) {
        if runnable_as_is {
            let full = Path::new(dir).join(cmd);
            if full.is_file() {
                return Some(full.to_string_lossy().into_owned());
            }
        }
        for ext in &exts {
            let full = Path::new(dir).join(format!("{cmd}{ext}"));
            if full.is_file() {
                return Some(full.to_string_lossy().into_owned());
            }
        }
    }
    None
}

#[cfg(not(windows))]
pub fn resolve_executable(cmd: &str) -> String {
    cmd.to_string()
}

/// .bat/.cmd scripts can't be spawned directly under ConPTY — route them
/// through the command interpreter.
#[cfg(windows)]
fn wrap_batch_script(file: String, args: Vec<String>) -> (String, Vec<String>) {
    if file.to_lowercase().ends_with(".ps1") {
        // CreateProcess can't run .ps1 directly (Windows would ask which app
        // opens it) — route through PowerShell.
        let wrapped = ["-NoLogo".into(), "-File".into(), file]
            .into_iter()
            .chain(args)
            .collect();
        return ("powershell.exe".into(), wrapped);
    }
    if file.to_lowercase().ends_with(".bat") || file.to_lowercase().ends_with(".cmd") {
        let wrapped = std::iter::once("/c".to_string())
            .chain(std::iter::once(file))
            .chain(args)
            .collect();
        (
            std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into()),
            wrapped,
        )
    } else {
        (file, args)
    }
}

#[cfg(not(windows))]
fn wrap_batch_script(file: String, args: Vec<String>) -> (String, Vec<String>) {
    (file, args)
}

pub struct SpawnOptions {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub cols: u16,
    pub rows: u16,
    pub cwd: String,
    /// Grace period between child exit and host shutdown (clients get a
    /// window to read the final screen; the Node engine uses 3s detached).
    pub exit_grace: Duration,
    /// Linger after the child exits (run --keep): the final screen stays
    /// readable until an explicit kill releases the session.
    pub keep: bool,
    pub logger: Option<Arc<EventLog>>,
    /// Used to classify prompts for read/wait clients.
    pub policy: Option<Arc<Policy>>,
}

type AutoToggle = Box<dyn FnMut(bool) -> bool + Send>;

pub struct Session {
    pub name: String,
    pub command_display: String,
    pub cwd: String,
    pub started_at: String,
    pub pid: u32,
    pub exited: AtomicBool,
    pub exit_code: Mutex<Option<i32>>,
    /// A kill was requested — a lingering (--keep) session must not outlive it.
    released: AtomicBool,
    size: Mutex<(u16, u16)>,
    screen: Mutex<Screen>,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    last_data: Mutex<Instant>,
    attach: Mutex<HashMap<u64, UnboundedSender<String>>>,
    attach_pending: Mutex<Vec<u8>>, // carry split UTF-8 across chunks
    dsr_tail: Mutex<Vec<u8>>,       // carry split escape sequences across chunks
    next_attach_id: AtomicU64,
    /// Raw-byte mirror for the in-process attached mode (`run` w/o -d).
    pub mirror: Mutex<Option<UnboundedSender<Vec<u8>>>>,
    /// Flipped once cleanup is done and the host process should exit.
    pub shutdown: watch::Sender<bool>,
    pub logger: Option<Arc<EventLog>>,
    pub policy: Option<Arc<Policy>>,
    /// Runtime autopilot toggle wired by the host (GUI set-auto op).
    pub auto_toggle: Mutex<Option<AutoToggle>>,
}

impl Session {
    /// Spawn the child under a PTY, start the output/exit pumps, and write
    /// the session registry entry. Must be called inside a tokio runtime.
    pub fn spawn(opts: SpawnOptions) -> anyhow::Result<Arc<Session>> {
        let (file, args) = wrap_batch_script(resolve_executable(&opts.command), opts.args.clone());

        let pty = native_pty_system();
        let pair = pty.openpty(PtySize {
            rows: opts.rows,
            cols: opts.cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut cmd = CommandBuilder::new(&file);
        cmd.args(&args);
        cmd.cwd(&opts.cwd);
        let mut child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let pid = child.process_id().unwrap_or(0);
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let killer = child.clone_killer();
        let (shutdown, _) = watch::channel(false);

        let session = Arc::new(Session {
            name: opts.name.clone(),
            command_display: std::iter::once(opts.command.clone())
                .chain(opts.args.iter().cloned())
                .collect::<Vec<_>>()
                .join(" "),
            cwd: opts.cwd.clone(),
            started_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            pid,
            exited: AtomicBool::new(false),
            exit_code: Mutex::new(None),
            released: AtomicBool::new(false),
            size: Mutex::new((opts.cols, opts.rows)),
            screen: Mutex::new(Screen::new(opts.cols, opts.rows)),
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            killer: Mutex::new(killer),
            last_data: Mutex::new(Instant::now()),
            attach: Mutex::new(HashMap::new()),
            attach_pending: Mutex::new(Vec::new()),
            dsr_tail: Mutex::new(Vec::new()),
            next_attach_id: AtomicU64::new(1),
            mirror: Mutex::new(None),
            shutdown,
            logger: opts.logger.clone(),
            policy: opts.policy.clone(),
            auto_toggle: Mutex::new(None),
        });

        session.write_meta()?;

        // Output pump: blocking PTY reads on a plain thread, forwarded into
        // the async world over a channel.
        let (data_tx, mut data_rx) = unbounded_channel::<Vec<u8>>();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if data_tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let s = session.clone();
        tokio::spawn(async move {
            while let Some(chunk) = data_rx.recv().await {
                s.on_data(&chunk);
            }
        });

        // Exit pump: blocking wait on a thread; cleanup after the grace
        // window so clients can still read the final screen.
        let (exit_tx, exit_rx) = tokio::sync::oneshot::channel::<i32>();
        std::thread::spawn(move || {
            let code = child
                .wait()
                .map(|status| status.exit_code() as i32)
                .unwrap_or(-1);
            let _ = exit_tx.send(code);
        });
        let s = session.clone();
        let grace = opts.exit_grace;
        let keep = opts.keep;
        tokio::spawn(async move {
            let code = exit_rx.await.unwrap_or(-1);
            s.exited.store(true, Ordering::SeqCst);
            *s.exit_code.lock().unwrap() = Some(code);
            s.broadcast(json!({ "event": "exit", "exitCode": code }));
            if let Some(logger) = &s.logger {
                logger.close(code);
            }
            if keep && !s.released.load(Ordering::SeqCst) {
                // --keep: linger with the final screen; kill() releases us.
                return;
            }
            tokio::time::sleep(grace).await;
            let _ = std::fs::remove_file(meta_path(&s.name));
            let _ = s.shutdown.send(true);
        });

        Ok(session)
    }

    fn write_meta(&self) -> anyhow::Result<()> {
        let (cols, rows) = *self.size.lock().unwrap();
        let meta = json!({
            "name": self.name,
            "pid": self.pid,
            "hostPid": std::process::id(),
            "command": self.command_display,
            "cols": cols,
            "rows": rows,
            "cwd": self.cwd,
            "startedAt": self.started_at,
            "pipe": pipe_path(&self.name),
        });
        std::fs::write(meta_path(&self.name), serde_json::to_string_pretty(&meta)?)?;
        Ok(())
    }

    pub fn is_exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    pub fn last_data_instant(&self) -> Instant {
        *self.last_data.lock().unwrap()
    }

    pub fn snapshot(&self, scrollback: bool) -> Snapshot {
        self.screen.lock().unwrap().snapshot(scrollback)
    }

    pub fn log_event(&self, kind: &str, detail: Value) {
        if let Some(logger) = &self.logger {
            logger.event(kind, detail);
        }
    }

    fn on_data(&self, chunk: &[u8]) {
        *self.last_data.lock().unwrap() = Instant::now();
        self.screen.lock().unwrap().write(chunk);
        if let Some(logger) = &self.logger {
            logger.output(chunk);
        }
        self.answer_cursor_queries(chunk);
        if let Some(mirror) = self.mirror.lock().unwrap().as_ref() {
            let _ = mirror.send(chunk.to_vec());
        }
        // Attach clients get UTF-8 text; hold back split multi-byte tails
        // until the next chunk completes them.
        if self.attach.lock().unwrap().is_empty() {
            return;
        }
        let text = {
            let mut pending = self.attach_pending.lock().unwrap();
            pending.extend_from_slice(chunk);
            let valid = match std::str::from_utf8(&pending) {
                Ok(_) => pending.len(),
                Err(e) if pending.len() - e.valid_up_to() < 4 => e.valid_up_to(),
                Err(_) => pending.len(), // hopeless bytes: flush lossily
            };
            let text = String::from_utf8_lossy(&pending[..valid]).into_owned();
            pending.drain(..valid);
            text
        };
        if !text.is_empty() {
            self.broadcast(json!({ "event": "data", "data": text }));
        }
    }

    /// We are the terminal, so cursor position queries (DSR, `ESC [ 6 n`) in
    /// the output stream are addressed to us and must be answered on the PTY
    /// input. ConPTY itself sends one at startup (INHERIT_CURSOR) and blocks
    /// the child until the report arrives; TUI apps also query at will.
    fn answer_cursor_queries(&self, chunk: &[u8]) {
        const QUERY: &[u8] = b"\x1b[6n";
        let mut tail = self.dsr_tail.lock().unwrap();
        tail.extend_from_slice(chunk);
        let hits = tail.windows(QUERY.len()).filter(|w| *w == QUERY).count();
        let keep = tail.len().saturating_sub(QUERY.len() - 1);
        tail.drain(..keep);
        drop(tail);
        for _ in 0..hits {
            let snap = self.screen.lock().unwrap().snapshot(false);
            self.write(&format!(
                "\x1b[{};{}R",
                snap.cursor_y + 1,
                snap.cursor_x + 1
            ));
        }
    }

    fn broadcast(&self, event: Value) {
        let line = event.to_string();
        self.attach
            .lock()
            .unwrap()
            .retain(|_, tx| tx.send(line.clone()).is_ok());
    }

    pub fn write(&self, data: &str) {
        if self.exited.load(Ordering::SeqCst) {
            return;
        }
        let mut w = self.writer.lock().unwrap();
        let _ = w.write_all(data.as_bytes());
        let _ = w.flush();
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        *self.size.lock().unwrap() = (cols, rows);
        if !self.exited.load(Ordering::SeqCst) {
            let _ = self.master.lock().unwrap().resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
        self.screen.lock().unwrap().resize(cols, rows);
    }

    pub fn kill(self: &Arc<Self>) {
        self.released.store(true, Ordering::SeqCst);
        if self.exited.load(Ordering::SeqCst) {
            // Child already gone: release a lingering (--keep) host.
            let _ = std::fs::remove_file(meta_path(&self.name));
            let _ = self.shutdown.send(true);
            return;
        }
        #[cfg(windows)]
        {
            // TerminateProcess via the cloned killer — a guaranteed hard kill.
            let _ = self.killer.lock().unwrap().kill();
        }
        #[cfg(unix)]
        {
            // portable-pty's cloned killer sends a single SIGHUP on unix;
            // programs that trap or ignore HUP survive while kill reports
            // success. Signal the child's whole process group instead (the
            // child is a session leader via setsid, so pgid == pid) and
            // escalate to SIGKILL after a grace period if it is still alive.
            let pid = self.pid as i32;
            if pid <= 0 {
                let _ = self.killer.lock().unwrap().kill();
                return;
            }
            unsafe { libc::kill(-pid, libc::SIGHUP) };
            let s = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(3_000)).await;
                if !s.exited.load(Ordering::SeqCst) {
                    unsafe { libc::kill(-(s.pid as i32), libc::SIGKILL) };
                }
            });
        }
    }

    fn snapshot_value(&self, scrollback: bool) -> Value {
        let snap = self.screen.lock().unwrap().snapshot(scrollback);
        json!({
            "lines": snap.lines,
            "cursor": { "x": snap.cursor_x, "y": snap.cursor_y },
        })
    }

    /// Register an attach client: push channel gets {event:...} lines.
    pub fn attach_client(&self, tx: UnboundedSender<String>, req: &Value) -> u64 {
        self.log_event(
            "attach",
            json!({ "source": req["source"].as_str().unwrap_or("attach") }),
        );
        let (cols, rows) = *self.size.lock().unwrap();
        if let (Some(c), Some(r)) = (req["cols"].as_u64(), req["rows"].as_u64()) {
            if (c as u16, r as u16) != (cols, rows) {
                self.resize(c as u16, r as u16);
            }
        }
        let (cols, rows) = *self.size.lock().unwrap();
        let exited = self.exited.load(Ordering::SeqCst);
        let exit_code = *self.exit_code.lock().unwrap();
        let _ = tx.send(
            json!({
                "event": "attached",
                "name": self.name,
                "cols": cols,
                "rows": rows,
                "alive": !exited,
                "exitCode": exit_code,
            })
            .to_string(),
        );
        if req["replay"].as_bool() != Some(false) {
            let restore = self.screen.lock().unwrap().restore_sequence();
            let _ =
                tx.send(json!({ "event": "data", "data": restore, "replay": true }).to_string());
        }
        if exited {
            let _ = tx.send(json!({ "event": "exit", "exitCode": exit_code }).to_string());
        }
        let id = self.next_attach_id.fetch_add(1, Ordering::SeqCst);
        self.attach.lock().unwrap().insert(id, tx);
        id
    }

    pub fn detach_client(&self, id: u64) {
        if self.attach.lock().unwrap().remove(&id).is_some() {
            self.log_event("detach", json!({}));
        }
    }

    /// One-shot control op — same request/response shapes as the Node engine.
    pub async fn handle_request(self: &Arc<Self>, req: Value) -> Value {
        let source = req["source"].as_str().unwrap_or("pipe").to_string();
        match req["op"].as_str().unwrap_or("") {
            "info" => {
                let (cols, rows) = *self.size.lock().unwrap();
                json!({
                    "ok": true,
                    "name": self.name,
                    "pid": self.pid,
                    "command": self.command_display,
                    "cwd": self.cwd,
                    "startedAt": self.started_at,
                    "alive": !self.exited.load(Ordering::SeqCst),
                    "exitCode": *self.exit_code.lock().unwrap(),
                    "cols": cols,
                    "rows": rows,
                })
            }
            "send" => {
                if self.exited.load(Ordering::SeqCst) {
                    return json!({ "ok": false, "error": "process has exited" });
                }
                self.log_event(
                    "send",
                    json!({
                    "text": req["data"], "enter": req["enter"].as_bool() == Some(true),
                    "source": source }),
                );
                self.write(req["data"].as_str().unwrap_or(""));
                if req["enter"].as_bool() == Some(true) {
                    // Small gap so TUI apps register the text before Enter.
                    let delay = req["enterDelay"].as_u64().unwrap_or(50);
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    self.write("\r");
                }
                json!({ "ok": true })
            }
            "keys" => {
                if self.exited.load(Ordering::SeqCst) {
                    return json!({ "ok": false, "error": "process has exited" });
                }
                self.log_event("keys", json!({ "keys": req["keys"], "source": source }));
                let keys: Vec<String> = req["keys"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|k| k.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                for key in &keys {
                    let Some(seq) = key_seq(key) else {
                        return json!({ "ok": false, "error": format!("unknown key: {key}") });
                    };
                    self.write(seq);
                    tokio::time::sleep(Duration::from_millis(30)).await;
                }
                json!({ "ok": true })
            }
            "read" => {
                let mut out = json!({
                    "ok": true,
                    "alive": !self.exited.load(Ordering::SeqCst),
                    "exitCode": *self.exit_code.lock().unwrap(),
                });
                merge(
                    &mut out,
                    self.snapshot_value(req["scrollback"].as_bool() == Some(true)),
                );
                out
            }
            "wait" => self.wait(&req, &source).await,
            "resize" => {
                let cols = req["cols"].as_u64().unwrap_or(120) as u16;
                let rows = req["rows"].as_u64().unwrap_or(30) as u16;
                self.resize(cols, rows);
                json!({ "ok": true })
            }
            "kill" => {
                self.log_event("kill", json!({ "source": source }));
                let s = self.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    s.kill();
                });
                json!({ "ok": true })
            }
            "set-auto" => {
                let mut toggle = self.auto_toggle.lock().unwrap();
                match toggle.as_mut() {
                    None => json!({ "ok": false, "error": "auto toggle not supported" }),
                    Some(f) => {
                        let enabled = req["enabled"].as_bool() == Some(true);
                        let auto = f(enabled);
                        self.log_event("set-auto", json!({ "enabled": enabled, "source": source }));
                        json!({ "ok": true, "auto": auto })
                    }
                }
            }
            other => json!({ "ok": false, "error": format!("unknown op: {other}") }),
        }
    }

    /// Block until the first requested condition is met. Mirrors the Node
    /// engine: pattern / gone / stable / prompt / idle, with exit and timeout
    /// always active; bare waits default to idle 2000ms.
    async fn wait(self: &Arc<Self>, req: &Value, source: &str) -> Value {
        let timeout_ms = req["timeoutMs"].as_u64().unwrap_or(60_000);
        let stable = req["stable"].as_u64();
        let prompt = req["prompt"].as_bool() == Some(true);
        let has_condition = req["pattern"].is_string()
            || req["gone"].is_string()
            || stable.is_some()
            || prompt
            || req["idleMs"].is_u64();
        let idle_ms = req["idleMs"]
            .as_u64()
            .or(if has_condition { None } else { Some(2_000) });

        let flags = req["flags"].as_str().unwrap_or("");
        let build = |pat: &str| -> Result<regex::Regex, regex::Error> {
            let mut b = regex::RegexBuilder::new(pat);
            b.case_insensitive(flags.contains('i'));
            b.multi_line(flags.contains('m'));
            b.dot_matches_new_line(flags.contains('s'));
            b.build()
        };
        let pattern = match req["pattern"].as_str().map(build).transpose() {
            Ok(p) => p,
            Err(e) => return json!({ "ok": false, "error": format!("bad pattern: {e}") }),
        };
        let gone = match req["gone"].as_str().map(build).transpose() {
            Ok(p) => p,
            Err(e) => return json!({ "ok": false, "error": format!("bad pattern: {e}") }),
        };

        let baseline: Option<Vec<String>> = if req["sinceStart"].as_bool() == Some(true) {
            Some(self.screen.lock().unwrap().snapshot(false).lines)
        } else {
            None
        };

        let started = Instant::now();
        let mut last_text: Option<String> = None;
        let mut stable_since = Instant::now();
        let mut poll = tokio::time::interval(Duration::from_millis(120));
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let reason = loop {
            poll.tick().await;
            let snap = self.screen.lock().unwrap().snapshot(false);
            let text = snap.lines.join("\n");
            if last_text.as_deref() != Some(&text) {
                last_text = Some(text.clone());
                stable_since = Instant::now();
            }
            if let Some(re) = &pattern {
                let target = match &baseline {
                    Some(base) => snap
                        .lines
                        .iter()
                        .enumerate()
                        .filter(|(i, l)| base.get(*i) != Some(*l))
                        .map(|(_, l)| l.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    None => text.clone(),
                };
                if re.is_match(&target) {
                    break "pattern";
                }
            }
            if let Some(re) = &gone {
                if !re.is_match(&text) {
                    break "gone";
                }
            }
            let stable_for = stable_since.elapsed().as_millis() as u64;
            if let Some(ms) = stable {
                if stable_for >= ms {
                    break "stable";
                }
            }
            if prompt && stable_for >= req["quietMs"].as_u64().unwrap_or(700) {
                let line = snap
                    .lines
                    .iter()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .map(|l| l.trim())
                    .unwrap_or("");
                if !line.is_empty() && is_promptish(line) {
                    break "prompt";
                }
            }
            if let Some(ms) = idle_ms {
                if self.last_data.lock().unwrap().elapsed().as_millis() as u64 >= ms {
                    break "idle";
                }
            }
            if self.exited.load(Ordering::SeqCst) {
                break "exit";
            }
            if started.elapsed().as_millis() as u64 >= timeout_ms {
                break "timeout";
            }
        };

        let snap = self.snapshot(false);
        let mut detail = json!({
            "reason": reason,
            "waitedMs": started.elapsed().as_millis() as u64,
            "source": source,
        });
        if let Some(p) = req["pattern"].as_str() {
            detail["pattern"] = p.into();
        }
        if let Some(g) = req["gone"].as_str() {
            detail["gone"] = g.into();
        }
        self.log_event("wait", detail);

        let mut out = json!({
            "ok": true,
            "reason": reason,
            "waitedMs": started.elapsed().as_millis() as u64,
            "alive": !self.exited.load(Ordering::SeqCst),
            "exitCode": *self.exit_code.lock().unwrap(),
        });
        if reason == "prompt" {
            merge(&mut out, self.classify_prompt(&snap.lines));
        }
        merge(&mut out, self.snapshot_value(false));
        out
    }

    /// Classify the last visible line against the policy so clients (GUI or
    /// agent) know who may answer. Returns {} without a policy or prompt line.
    fn classify_prompt(&self, lines: &[String]) -> Value {
        let Some(policy) = &self.policy else {
            return json!({});
        };
        let line = lines
            .iter()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim().to_string())
            .unwrap_or_default();
        if line.is_empty() {
            return json!({});
        }
        let m = evaluate(policy, &line, &lines.join("\n"));
        json!({
            "promptLine": line,
            "promptClass": m.as_ref().map(|m| m.class).unwrap_or("unmatched"),
            "promptRule": m.as_ref().and_then(|m| m.rule.name.clone()),
            "promptAction": m.as_ref().map(|m| m.rule.action.clone()),
            "promptText": m.as_ref().and_then(|m| {
                (m.class == "auto").then(|| m.rule.text.clone()).flatten()
            }),
        })
    }
}

fn merge(into: &mut Value, from: Value) {
    if let (Some(a), Some(b)) = (into.as_object_mut(), from.as_object()) {
        for (k, v) in b {
            a.insert(k.clone(), v.clone());
        }
    }
}

#[cfg(test)]
#[cfg(windows)]
mod resolve_tests {
    use super::resolve_in_path;

    const PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

    fn fixture_dir(tag: &str, files: &[&str]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "puppetty-resolve-test-{}-{tag}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for f in files {
            std::fs::write(dir.join(f), "").unwrap();
        }
        dir
    }

    #[test]
    fn bare_name_skips_the_posix_shim_and_finds_the_cmd() {
        // npm installs `codex` (sh script) next to `codex.cmd`; CreateProcess
        // can only run the latter.
        let dir = fixture_dir("shim", &["codex", "codex.cmd", "codex.ps1"]);
        let path = dir.to_string_lossy().into_owned();
        let hit = resolve_in_path("codex", &path, PATHEXT).unwrap();
        assert!(hit.to_lowercase().ends_with("codex.cmd"), "got {hit}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bare_name_with_only_a_posix_shim_resolves_nothing() {
        let dir = fixture_dir("onlyshim", &["onlyshim"]);
        let path = dir.to_string_lossy().into_owned();
        assert_eq!(resolve_in_path("onlyshim", &path, PATHEXT), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicit_extension_still_matches_as_is() {
        let dir = fixture_dir("asis", &["tool.exe", "script.ps1"]);
        let path = dir.to_string_lossy().into_owned();
        let exe = resolve_in_path("tool.exe", &path, PATHEXT).unwrap();
        assert!(exe.to_lowercase().ends_with("tool.exe"), "got {exe}");
        // .ps1 is not in PATHEXT but wrap_batch_script can run it.
        let ps1 = resolve_in_path("script.ps1", &path, PATHEXT).unwrap();
        assert!(ps1.to_lowercase().ends_with("script.ps1"), "got {ps1}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pathext_order_is_respected() {
        let dir = fixture_dir("order", &["dup.cmd", "dup.exe"]);
        let path = dir.to_string_lossy().into_owned();
        let hit = resolve_in_path("dup", &path, PATHEXT).unwrap();
        assert!(hit.to_lowercase().ends_with("dup.exe"), "got {hit}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end through the real PTY: spawn, render, detect exit. On
    /// Windows this guards the ConPTY cursor-query handshake — without the
    /// DSR reply the child never starts and this test hangs at the timeout.
    #[tokio::test(flavor = "multi_thread")]
    async fn echo_renders_and_exits() {
        #[cfg(windows)]
        let (command, args) = (
            "cmd",
            vec!["/c".into(), "echo rust-engine-test-marker".into()],
        );
        #[cfg(unix)]
        let (command, args) = (
            "sh",
            vec!["-c".into(), "echo rust-engine-test-marker".into()],
        );
        let session = Session::spawn(SpawnOptions {
            name: format!("rs-selftest-{}", std::process::id()),
            command: command.into(),
            args,
            cols: 80,
            rows: 24,
            cwd: ".".into(),
            exit_grace: Duration::from_millis(100),
            keep: false,
            logger: None,
            policy: None,
        })
        .unwrap();

        let mut shutdown = session.shutdown.subscribe();
        tokio::time::timeout(Duration::from_secs(15), async {
            while !*shutdown.borrow() {
                shutdown.changed().await.unwrap();
            }
        })
        .await
        .expect("child did not exit — ConPTY handshake likely wedged");

        assert!(session.exited.load(Ordering::SeqCst));
        let res = session.handle_request(json!({ "op": "read" })).await;
        let text = res["lines"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|l| l.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("rust-engine-test-marker"),
            "screen was: {text}"
        );
    }
}
