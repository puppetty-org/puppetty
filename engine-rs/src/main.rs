mod autopilot;
mod client;
mod credentials;
mod decider;
mod eventlog;
mod keyexpand;
mod mcp;
mod policy;
mod protocol;
mod screen;
mod server;
mod session;
mod svg;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use crate::autopilot::{attach_autopilot, Autopilot, PilotOptions};
use crate::client::{free_name, list_sessions};
use crate::policy::{load_policy, parse_jsonc, user_config_path, Policy};
use crate::protocol::meta_path;
use crate::session::{Session, SpawnOptions};

#[derive(Parser)]
#[command(
    name = "puppetty-engine",
    version,
    about = "puppetty — controllable virtual terminal sessions for AI agents (Rust engine)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, clap::Args)]
struct RunOpts {
    #[arg(long)]
    name: Option<String>,
    /// Run the session in the background and return
    #[arg(short = 'd', long)]
    detach: bool,
    #[arg(long)]
    cols: Option<u16>,
    #[arg(long)]
    rows: Option<u16>,
    /// Working directory for the session
    #[arg(long)]
    cwd: Option<String>,
    /// Use another session's cwd (companion sessions)
    #[arg(long = "cwd-of")]
    cwd_of: Option<String>,
    /// Answer prompts per policy (~/.puppetty/config.json)
    #[arg(long)]
    auto: bool,
    /// Consult this command for unrecognized prompts (implies --auto)
    #[arg(long)]
    decider: Option<String>,
    /// Silence before prompt detection (default 700)
    #[arg(long = "quiet-ms", default_value_t = 700)]
    quiet_ms: u64,
    /// Seconds before an unanswered prompt escalates
    #[arg(long = "prompt-timeout")]
    prompt_timeout: Option<u64>,
    /// Disable the session event log (.cast/.jsonl)
    #[arg(long = "no-log")]
    no_log: bool,
    /// Keep the session readable after the child exits, until `puppetty
    /// kill` (detached only; without it the host stops ~3s after exit)
    #[arg(long, visible_alias = "linger")]
    keep: bool,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    command: Vec<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start a session (attached by default; -d for detached background)
    Run(RunOpts),
    /// (internal) Session host process for detached sessions
    #[command(hide = true)]
    Host(RunOpts),
    /// Type text into a session (appends Enter unless --no-enter)
    Send {
        name: String,
        #[arg(long = "no-enter")]
        no_enter: bool,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        text: Vec<String>,
    },
    /// Press named keys (enter, tab, esc, up, down, ctrl-c, ...)
    Keys {
        name: String,
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// Print the rendered screen
    Read {
        name: String,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        scrollback: bool,
        /// Replay the session's newest .cast log instead of the live screen
        /// (recovers the final screen after the session is gone)
        #[arg(long)]
        last: bool,
    },
    /// Save a session screen as an SVG image (colors and styling included)
    Snap {
        name: String,
        /// Output file (default: <name>.svg)
        #[arg(long, short = 'o')]
        out: Option<String>,
        /// Render from the newest .cast log instead of the live screen
        #[arg(long)]
        last: bool,
    },
    /// Block until a condition is met, then print the screen
    Wait {
        name: String,
        /// Resolve when the screen matches this regex
        #[arg(long = "for")]
        pattern: Option<String>,
        /// Resolve when this regex no longer matches the screen
        #[arg(long)]
        gone: Option<String>,
        /// Resolve when the rendered screen is unchanged for N ms
        #[arg(long)]
        stable: Option<u64>,
        /// Resolve when settled on a prompt-looking line
        #[arg(long)]
        prompt: bool,
        /// With --prompt: screen quiet time before judging (default 700;
        /// tripled when the cursor is not at the prompt line)
        #[arg(long = "quiet-ms")]
        quiet_ms: Option<u64>,
        /// Resolve when no output bytes for N ms
        #[arg(long)]
        idle: Option<u64>,
        /// Timeout in seconds (exit code 1)
        #[arg(long, default_value_t = 60)]
        timeout: u64,
        /// Only match lines that changed after the wait began
        #[arg(long = "since-start")]
        since_start: bool,
        /// Regex flags (i, m, s)
        #[arg(long, default_value = "")]
        flags: String,
        #[arg(long)]
        json: bool,
    },
    /// Attach this terminal to a session (detach: Ctrl+])
    Attach { name: String },
    /// List live sessions
    List {
        #[arg(long)]
        json: bool,
    },
    /// Print session info as JSON
    Info { name: String },
    /// Terminate a session's child process
    Kill { name: String },
    /// Run as an MCP server (stdio) for AI agents
    Mcp,
    /// Manage credentials in the OS keyring (set/list/rm)
    Cred {
        action: String,
        #[arg(name = "ref")]
        cred_ref: Option<String>,
        /// Read the secret from stdin (used by the GUI)
        #[arg(long)]
        stdin: bool,
    },
    /// Policy config: show the effective merged policy, or validate stdin
    Config { action: String },
}

const SUBCOMMANDS: &[&str] = &[
    "run", "host", "send", "keys", "read", "snap", "wait", "attach", "list", "info", "kill", "mcp",
    "cred", "config", "help",
];

/// `puppetty python` means `puppetty run python` — same implicit-run
/// convention as the Node CLI.
fn normalize_argv(mut argv: Vec<String>) -> Vec<String> {
    if let Some(first) = argv.get(1) {
        let looks_like_flag = first.starts_with('-');
        let known = SUBCOMMANDS.contains(&first.as_str())
            || (looks_like_flag && ["-h", "--help", "-V", "--version"].contains(&first.as_str()));
        if !known {
            argv.insert(1, "run".into());
        }
    }
    argv
}

#[tokio::main]
async fn main() {
    let argv = normalize_argv(std::env::args().collect());
    let cli = Cli::parse_from(argv);
    let code = match cli.cmd {
        Cmd::Run(opts) => cmd_run(opts).await,
        Cmd::Host(opts) => host_main(opts, false).await,
        Cmd::Send {
            name,
            no_enter,
            text,
        } => request_and_report(
            &name,
            json!({ "op": "send", "data": text.join(" "), "enter": !no_enter, "source": "cli" }),
        )
        .await,
        Cmd::Keys { name, keys } => {
            request_and_report(
                &name,
                json!({ "op": "keys", "keys": keys, "source": "cli" }),
            )
            .await
        }
        Cmd::Read {
            name,
            json: as_json,
            scrollback,
            last,
        } => {
            if last {
                cmd_read_last(&name, as_json, scrollback)
            } else {
                cmd_read(&name, as_json, scrollback).await
            }
        }
        Cmd::Snap { name, out, last } => cmd_snap(&name, out, last).await,
        Cmd::Wait {
            name,
            pattern,
            gone,
            stable,
            prompt,
            quiet_ms,
            idle,
            timeout,
            since_start,
            flags,
            json: as_json,
        } => {
            cmd_wait(
                &name,
                pattern,
                gone,
                stable,
                prompt,
                quiet_ms,
                idle,
                timeout,
                since_start,
                &flags,
                as_json,
            )
            .await
        }
        Cmd::Attach { name } => cmd_attach(&name).await,
        Cmd::List { json: as_json } => cmd_list(as_json).await,
        Cmd::Info { name } => cmd_info(&name).await,
        Cmd::Kill { name } => {
            let code = request_and_report(&name, json!({ "op": "kill", "source": "cli" })).await;
            if code == 0 {
                println!("killed {name}");
            }
            code
        }
        Cmd::Mcp => mcp::run_mcp_server().await,
        Cmd::Cred {
            action,
            cred_ref,
            stdin,
        } => cmd_cred(&action, cred_ref.as_deref(), stdin).await,
        Cmd::Config { action } => cmd_config(&action).await,
    };
    std::process::exit(code);
}

fn fail(msg: &str) -> i32 {
    eprintln!("puppetty: {msg}");
    2
}

async fn request_and_report(name: &str, req: Value) -> i32 {
    match client::request(name, &req, 10_000).await {
        Ok(res) if res["ok"].as_bool() == Some(true) => 0,
        Ok(res) => fail(res["error"].as_str().unwrap_or("request failed")),
        Err(e) => fail(&e),
    }
}

// ---- run / host ----

fn command_base_name(command: &str) -> String {
    Path::new(command)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "session".into())
}

async fn cmd_run(mut opts: RunOpts) -> i32 {
    if opts.keep && !opts.detach {
        // Attached mode already shows the final screen in your terminal.
        return fail("--keep only makes sense with -d/--detach");
    }
    // Resolve cwd (cwd-of wins), then the session name, then dispatch.
    let cwd = if let Some(target) = &opts.cwd_of {
        match client::request(target, &json!({ "op": "info" }), 5_000).await {
            Ok(info) if info["ok"].as_bool() == Some(true) && info["cwd"].is_string() => {
                info["cwd"].as_str().unwrap().to_string()
            }
            _ => return fail(&format!("cannot resolve cwd of session \"{target}\"")),
        }
    } else if let Some(c) = &opts.cwd {
        std::fs::canonicalize(c)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| c.clone())
    } else {
        std::env::current_dir()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_else(|_| ".".into())
    };
    if !Path::new(&cwd).exists() {
        return fail(&format!("cwd does not exist: {cwd}"));
    }
    opts.cwd = Some(cwd.clone());

    let name = match &opts.name {
        Some(n) => {
            let live = list_sessions().await;
            if live.iter().any(|s| s["name"].as_str() == Some(n)) {
                return fail(&format!("session \"{n}\" already exists"));
            }
            n.clone()
        }
        None => free_name(&command_base_name(&opts.command[0])).await,
    };
    opts.name = Some(name.clone());

    if !opts.detach {
        return host_main(opts, true).await;
    }

    // Detached: respawn ourselves as the host, fully disowned, then wait for
    // the session to answer so startup failures surface here.
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return fail(&e.to_string()),
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("host")
        .arg("--name")
        .arg(&name)
        .arg("--cwd")
        .arg(&cwd)
        .arg("--cols")
        .arg(opts.cols.unwrap_or(120).to_string())
        .arg("--rows")
        .arg(opts.rows.unwrap_or(30).to_string())
        .arg("--quiet-ms")
        .arg(opts.quiet_ms.to_string());
    if opts.auto {
        cmd.arg("--auto");
    }
    if let Some(d) = &opts.decider {
        cmd.arg("--decider").arg(d);
    }
    if let Some(t) = opts.prompt_timeout {
        cmd.arg("--prompt-timeout").arg(t.to_string());
    }
    if opts.no_log {
        cmd.arg("--no-log");
    }
    if opts.keep {
        cmd.arg("--keep");
    }
    cmd.arg("--").args(&opts.command);
    // Host stderr goes to a scratch file so a startup failure can be
    // reported with its actual cause instead of a bare "failed to start".
    let err_path = std::env::temp_dir().join(format!(
        "puppetty-host-{name}-{}.stderr",
        std::process::id()
    ));
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    match std::fs::File::create(&err_path) {
        Ok(f) => cmd.stderr(f),
        Err(_) => cmd.stderr(std::process::Stdio::null()),
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0008 | 0x0800_0000); // DETACHED_PROCESS | CREATE_NO_WINDOW
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // survive the parent's terminal/signals
    }
    let mut host = match cmd.spawn() {
        Ok(h) => h,
        Err(e) => {
            let _ = std::fs::remove_file(&err_path);
            return fail(&e.to_string());
        }
    };
    let mut host_died = false;
    for _ in 0..100 {
        if meta_path(&name).exists()
            && client::request(&name, &json!({ "op": "info" }), 1_000)
                .await
                .is_ok()
        {
            let _ = std::fs::remove_file(&err_path);
            println!("{name}");
            eprintln!(
                "[puppetty] detached session \"{name}\" started — read: puppetty read {name}"
            );
            return 0;
        }
        if host_died {
            break;
        }
        // The host exiting this early means startup failed — one more loop
        // iteration to close any lost race with its registry write, then
        // report instead of polling out the full 10s.
        host_died = matches!(host.try_wait(), Ok(Some(_)));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut msg = format!("detached session \"{name}\" failed to start");
    if let Ok(err_out) = std::fs::read_to_string(&err_path) {
        let err_out = err_out.trim();
        if !err_out.is_empty() {
            msg.push_str(&format!("\n{err_out}"));
        }
    }
    let _ = std::fs::remove_file(&err_path);
    fail(&msg)
}

async fn host_main(opts: RunOpts, attached: bool) -> i32 {
    let name = opts.name.clone().expect("host requires --name");
    let cwd = opts.cwd.clone().unwrap_or_else(|| ".".into());
    let policy = match load_policy(&cwd) {
        Ok(p) => Arc::new(p),
        Err(e) => return fail(&e),
    };
    let tty_size = if attached {
        crossterm::terminal::size().ok()
    } else {
        None
    };
    let cols = opts.cols.or(tty_size.map(|s| s.0)).unwrap_or(120);
    let rows = opts.rows.or(tty_size.map(|s| s.1)).unwrap_or(30);

    let command_display = opts.command.join(" ");
    let logger = if !opts.no_log && policy.logging.enabled {
        match crate::eventlog::EventLog::new(&name, &command_display, cols, rows, &policy.logging) {
            Ok(l) => Some(Arc::new(l)),
            Err(e) => return fail(&format!("cannot open session log: {e}")),
        }
    } else {
        None
    };

    let session = match Session::spawn(SpawnOptions {
        name,
        command: opts.command[0].clone(),
        args: opts.command[1..].to_vec(),
        cols,
        rows,
        cwd,
        exit_grace: Duration::from_millis(if attached { 100 } else { 3_000 }),
        keep: opts.keep,
        logger,
        policy: Some(policy.clone()),
    }) {
        Ok(s) => s,
        Err(e) => return fail(&e.to_string()),
    };
    let mut shutdown = session.shutdown.subscribe();

    // Unix: bind the control socket before anything reports the session as
    // started — a bind failure must be fatal, not a reachable-by-no-one
    // session (the exit pump cleans up the registry entry after the kill).
    #[cfg(not(windows))]
    let srv_listener = match server::bind(&session.name) {
        Ok(l) => l,
        Err(e) => {
            session.kill();
            return fail(&format!("cannot create the session control endpoint: {e}"));
        }
    };

    let srv = session.clone();
    #[cfg(not(windows))]
    tokio::spawn(async move {
        let _ = server::serve(srv_listener, srv).await;
    });
    #[cfg(windows)]
    tokio::spawn(async move {
        let _ = server::serve(srv).await;
    });

    // Autopilot: --auto/--decider attach it now; the set-auto op toggles it
    // at runtime (a GUI attaches a human, so a runtime-enabled pilot gets an
    // effectively-infinite prompt timeout and never auto-cancels).
    let pilot: Arc<Mutex<Option<Autopilot>>> = Arc::new(Mutex::new(None));
    let make_pilot = {
        let session = session.clone();
        let policy = policy.clone();
        let decider = opts.decider.clone();
        let quiet_ms = opts.quiet_ms;
        move |prompt_timeout: u64| {
            attach_autopilot(
                session.clone(),
                PilotOptions {
                    policy: policy.clone(),
                    quiet_ms,
                    prompt_timeout,
                    decider: decider.clone(),
                    log_stderr: attached,
                },
            )
        }
    };
    let default_timeout = opts
        .prompt_timeout
        .unwrap_or(policy.on_unanswered.after_sec);
    if opts.auto || opts.decider.is_some() {
        *pilot.lock().unwrap() = Some(make_pilot(default_timeout));
    }
    {
        let pilot = pilot.clone();
        let make_pilot = make_pilot.clone();
        *session.auto_toggle.lock().unwrap() = Some(Box::new(move |enabled| {
            let mut p = pilot.lock().unwrap();
            if enabled && p.is_none() {
                *p = Some(make_pilot(31_536_000));
            } else if !enabled {
                if let Some(old) = p.take() {
                    old.stop();
                }
            }
            p.is_some()
        }));
    }

    if attached {
        run_attached(&session).await;
        if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
            let name = session.name.clone();
            eprintln!(
                "\x1b[2m[puppetty] session \"{name}\" — control it from another terminal: puppetty send {name} \"...\"\x1b[0m"
            );
        }
    }

    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow() {
            break;
        }
    }
    if attached {
        let _ = crossterm::terminal::disable_raw_mode();
    }
    let mut code = session.exit_code.lock().unwrap().unwrap_or(0);
    // "gave up on a prompt" must be distinguishable from "completed".
    let cancelled = pilot
        .lock()
        .unwrap()
        .as_ref()
        .map(|p| p.cancelled.load(std::sync::atomic::Ordering::SeqCst))
        .unwrap_or(false);
    // Children killed by our own Ctrl+C report STATUS_CONTROL_C_EXIT on
    // Windows rather than 0 — both mean "gave up on a prompt" here.
    const STATUS_CONTROL_C_EXIT: i32 = -1_073_741_510; // 0xC000013A
    if cancelled && (code == 0 || code == STATUS_CONTROL_C_EXIT) {
        code = 130;
    }
    code
}

/// Attached mode: mirror PTY output to stdout, forward raw stdin to the PTY.
async fn run_attached(session: &Arc<Session>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    if is_tty {
        let _ = crossterm::terminal::enable_raw_mode();
    }
    if let Ok((c, r)) = crossterm::terminal::size() {
        session.resize(c, r);
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    *session.mirror.lock().unwrap() = Some(tx);
    tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(chunk) = rx.recv().await {
            if stdout.write_all(&chunk).await.is_err() {
                break;
            }
            let _ = stdout.flush().await;
        }
    });

    let s = session.clone();
    tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 1024];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    // Content is never logged: a human may be typing a secret.
                    s.log_event("stdin", json!({ "bytes": n, "source": "human-cli" }));
                    s.write(&String::from_utf8_lossy(&buf[..n]));
                }
            }
        }
    });

    // Track terminal resizes (poll: no signal on Windows).
    let s = session.clone();
    tokio::spawn(async move {
        let mut last = crossterm::terminal::size().ok();
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let now = crossterm::terminal::size().ok();
            if now != last {
                last = now;
                if let Some((c, r)) = now {
                    s.resize(c, r);
                }
            }
        }
    });
}

// ---- attach (remote) ----

async fn cmd_attach(name: &str) -> i32 {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    #[cfg(windows)]
    let stream = {
        use tokio::net::windows::named_pipe::ClientOptions;
        match ClientOptions::new().open(crate::protocol::endpoint_for(name)) {
            Ok(s) => s,
            Err(e) => return fail(&format!("cannot reach session \"{name}\" ({e})")),
        }
    };
    #[cfg(not(windows))]
    let stream = match tokio::net::UnixStream::connect(crate::protocol::endpoint_for(name)).await {
        Ok(s) => s,
        Err(e) => return fail(&format!("cannot reach session \"{name}\" ({e})")),
    };

    let (read_half, mut write_half) = tokio::io::split(stream);
    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let size = crossterm::terminal::size().ok();
    let mut attach_req = json!({ "op": "attach", "source": "human-cli-attach" });
    if let Some((c, r)) = size {
        attach_req["cols"] = c.into();
        attach_req["rows"] = r.into();
    }
    if write_half
        .write_all(format!("{attach_req}\n").as_bytes())
        .await
        .is_err()
    {
        return fail(&format!("cannot reach session \"{name}\""));
    }
    eprintln!("\x1b[2m[puppetty] attached to \"{name}\" — Ctrl+] to detach\x1b[0m");
    if is_tty {
        let _ = crossterm::terminal::enable_raw_mode();
    }

    // stdin → input ops; Ctrl+] detaches.
    let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel::<(i32, Option<String>)>();
    let stdin_done = done_tx.clone();
    let write_half = Arc::new(tokio::sync::Mutex::new(write_half));
    let stdin_writer = write_half.clone();
    let stdin_task = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 1024];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if buf[..n].contains(&0x1d) {
                        let mut w = stdin_writer.lock().await;
                        let _ = w.write_all(b"{\"op\":\"detach\"}\n").await;
                        let _ = stdin_done.send((
                            0,
                            Some("\n[puppetty] detached (session keeps running)".to_string()),
                        ));
                        break;
                    }
                    let msg = json!({ "op": "input", "data": String::from_utf8_lossy(&buf[..n]) });
                    let mut w = stdin_writer.lock().await;
                    if w.write_all(format!("{msg}\n").as_bytes()).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Resize watcher.
    let resize_writer = write_half.clone();
    tokio::spawn(async move {
        let mut last = crossterm::terminal::size().ok();
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let now = crossterm::terminal::size().ok();
            if now != last {
                last = now;
                if let Some((c, r)) = now {
                    let msg = json!({ "op": "resize", "cols": c, "rows": r });
                    let mut w = resize_writer.lock().await;
                    let _ = w.write_all(format!("{msg}\n").as_bytes()).await;
                }
            }
        }
    });

    // Server events → stdout.
    let events_done = done_tx;
    let mut lines = BufReader::new(read_half).lines();
    let events_task = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            match msg["event"].as_str().unwrap_or("") {
                "data" => {
                    let _ = stdout
                        .write_all(msg["data"].as_str().unwrap_or("").as_bytes())
                        .await;
                    let _ = stdout.flush().await;
                }
                "exit" => {
                    let code = msg["exitCode"].as_i64().unwrap_or(0) as i32;
                    let _ = events_done
                        .send((code, Some(format!("\n[puppetty] session exited ({code})"))));
                    break;
                }
                _ => {}
            }
        }
        let _ = events_done.send((0, None));
    });

    let (code, msg) = done_rx.recv().await.unwrap_or((0, None));
    if is_tty {
        let _ = crossterm::terminal::disable_raw_mode();
    }
    if let Some(m) = msg {
        eprintln!("{m}");
    }
    stdin_task.abort();
    events_task.abort();
    code
}

// ---- client commands ----

async fn cmd_read(name: &str, as_json: bool, scrollback: bool) -> i32 {
    let req = json!({ "op": "read", "scrollback": scrollback, "source": "cli" });
    match client::request(name, &req, 10_000).await {
        Ok(res) if res["ok"].as_bool() == Some(true) => {
            if as_json {
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                print_lines(&res);
                if res["alive"].as_bool() == Some(false) {
                    eprintln!("[puppetty] process exited (code {})", res["exitCode"]);
                }
            }
            0
        }
        Ok(res) => fail(res["error"].as_str().unwrap_or("read failed")),
        Err(e) => fail(&with_last_hint(name, &e, "read")),
    }
}

/// A dead session still has its recording — point at it instead of leaving
/// the user with a bare "cannot reach". `cmd` is the subcommand to suggest
/// (read or snap).
fn with_last_hint(name: &str, err: &str, cmd: &str) -> String {
    if crate::eventlog::latest_cast(name).is_some() {
        format!("{err}\n(the session is gone; its final screen: puppetty {cmd} {name} --last)")
    } else {
        err.to_string()
    }
}

/// Replay a .cast recording into a fresh Screen sized from its header.
fn replay_cast(cast: &Path) -> Result<crate::screen::Screen, String> {
    let text = std::fs::read_to_string(cast)
        .map_err(|e| format!("cannot read {}: {e}", cast.display()))?;
    let mut lines = text.lines();
    let header: Value = lines
        .next()
        .and_then(|l| serde_json::from_str(l).ok())
        .unwrap_or_else(|| json!({}));
    let mut screen = crate::screen::Screen::new(
        header["width"].as_u64().unwrap_or(120) as u16,
        header["height"].as_u64().unwrap_or(30) as u16,
    );
    for line in lines {
        let Ok(ev) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if ev[1].as_str() == Some("o") {
            if let Some(data) = ev[2].as_str() {
                screen.write(data.as_bytes());
            }
        }
    }
    Ok(screen)
}

/// read --last: rebuild the final screen from the newest .cast recording.
/// Works after the session host is gone — for one-shot CLIs the final screen
/// is the whole point.
fn cmd_read_last(name: &str, as_json: bool, scrollback: bool) -> i32 {
    let Some(cast) = crate::eventlog::latest_cast(name) else {
        return fail(&format!("no session log found for \"{name}\""));
    };
    let screen = match replay_cast(&cast) {
        Ok(s) => s,
        Err(e) => return fail(&e),
    };
    let snap = screen.snapshot(scrollback);
    let exit_code = crate::eventlog::exit_code_for(&cast);
    if as_json {
        let out = json!({
            "ok": true,
            "source": "log",
            "logFile": cast.to_string_lossy(),
            "alive": false,
            "exitCode": exit_code,
            "lines": snap.lines,
            "cursor": { "x": snap.cursor_x, "y": snap.cursor_y },
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        for line in &snap.lines {
            println!("{line}");
        }
        eprintln!("[puppetty] replayed from {}", cast.display());
        match exit_code {
            Some(code) => eprintln!("[puppetty] process exited (code {code})"),
            None => eprintln!("[puppetty] no exit recorded — the session may still be live"),
        }
    }
    0
}

/// snap: render the screen (live via the restore sequence, or --last from
/// the newest recording) to a standalone SVG file.
async fn cmd_snap(name: &str, out: Option<String>, last: bool) -> i32 {
    let (screen, show_cursor) = if last {
        let Some(cast) = crate::eventlog::latest_cast(name) else {
            return fail(&format!("no session log found for \"{name}\""));
        };
        match replay_cast(&cast) {
            // No exit event yet: still running, so the cursor is real.
            Ok(s) => (s, crate::eventlog::exit_code_for(&cast).is_none()),
            Err(e) => return fail(&e),
        }
    } else {
        let req = json!({ "op": "read", "restore": true, "source": "cli" });
        match client::request(name, &req, 10_000).await {
            Ok(res) if res["ok"].as_bool() == Some(true) => {
                let mut screen = crate::screen::Screen::new(
                    res["cols"].as_u64().unwrap_or(120) as u16,
                    res["rows"].as_u64().unwrap_or(30) as u16,
                );
                screen.write(res["restore"].as_str().unwrap_or("").as_bytes());
                (screen, res["alive"].as_bool() == Some(true))
            }
            Ok(res) => return fail(res["error"].as_str().unwrap_or("snap failed")),
            Err(e) => return fail(&with_last_hint(name, &e, "snap")),
        }
    };
    let rendered = svg::render(&screen.styled_snapshot(), show_cursor);
    let path = out.unwrap_or_else(|| format!("{name}.svg"));
    if let Err(e) = std::fs::write(&path, rendered) {
        return fail(&format!("cannot write {path}: {e}"));
    }
    println!("{path}");
    0
}

#[allow(clippy::too_many_arguments)]
async fn cmd_wait(
    name: &str,
    pattern: Option<String>,
    gone: Option<String>,
    stable: Option<u64>,
    prompt: bool,
    quiet_ms: Option<u64>,
    idle: Option<u64>,
    timeout_secs: u64,
    since_start: bool,
    flags: &str,
    as_json: bool,
) -> i32 {
    let mut req = json!({
        "op": "wait",
        "timeoutMs": timeout_secs * 1_000,
        "flags": flags,
        "source": "cli",
    });
    let obj = req.as_object_mut().unwrap();
    if let Some(p) = pattern {
        obj.insert("pattern".into(), p.into());
    }
    if let Some(g) = gone {
        obj.insert("gone".into(), g.into());
    }
    if let Some(s) = stable {
        obj.insert("stable".into(), s.into());
    }
    if prompt {
        obj.insert("prompt".into(), true.into());
    }
    if let Some(q) = quiet_ms {
        obj.insert("quietMs".into(), q.into());
    }
    if let Some(i) = idle {
        obj.insert("idleMs".into(), i.into());
    }
    if since_start {
        obj.insert("sinceStart".into(), true.into());
    }

    match client::request(name, &req, timeout_secs * 1_000 + 5_000).await {
        Ok(res) if res["ok"].as_bool() == Some(true) => {
            let reason = res["reason"].as_str().unwrap_or("unknown");
            if as_json {
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                print_lines(&res);
                eprintln!(
                    "[puppetty] wait ended: {reason} after {}ms",
                    res["waitedMs"]
                );
            }
            i32::from(reason == "timeout")
        }
        Ok(res) => fail(res["error"].as_str().unwrap_or("wait failed")),
        Err(e) => fail(&with_last_hint(name, &e, "read")),
    }
}

async fn cmd_list(as_json: bool) -> i32 {
    let sessions = list_sessions().await;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&Value::Array(sessions)).unwrap()
        );
    } else if sessions.is_empty() {
        println!("(no live sessions)");
    } else {
        for s in &sessions {
            let status = if s["alive"].as_bool() == Some(true) {
                "alive".to_string()
            } else {
                format!("exited({})", s["exitCode"])
            };
            println!(
                "{}\tpid={}\t{status}\t{}\t{}",
                s["name"].as_str().unwrap_or("?"),
                s["pid"],
                s["command"].as_str().unwrap_or(""),
                s["cwd"].as_str().unwrap_or("")
            );
        }
    }
    0
}

async fn cmd_info(name: &str) -> i32 {
    match client::request(name, &json!({ "op": "info" }), 5_000).await {
        Ok(res) if res["ok"].as_bool() == Some(true) => {
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
            0
        }
        Ok(res) => fail(res["error"].as_str().unwrap_or("info failed")),
        Err(e) => fail(&e),
    }
}

fn print_lines(res: &Value) {
    if let Some(lines) = res["lines"].as_array() {
        for line in lines {
            println!("{}", line.as_str().unwrap_or(""));
        }
    }
}

// ---- cred / config ----

/// Read a secret from the TTY without echoing it.
fn read_hidden(prompt: &str) -> Result<String, String> {
    use crossterm::event::{Event, KeyCode, KeyModifiers};
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Err("cannot read a secret without a TTY".into());
    }
    eprint!("{prompt}");
    crossterm::terminal::enable_raw_mode().map_err(|e| e.to_string())?;
    let mut buf = String::new();
    let result = loop {
        match crossterm::event::read() {
            Ok(Event::Key(k)) => match k.code {
                KeyCode::Enter => break Ok(buf.clone()),
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err("cancelled".into())
                }
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c) => buf.push(c),
                _ => {}
            },
            Ok(_) => {}
            Err(e) => break Err(e.to_string()),
        }
    };
    let _ = crossterm::terminal::disable_raw_mode();
    eprintln!();
    result
}

async fn cmd_cred(action: &str, cred_ref: Option<&str>, stdin: bool) -> i32 {
    match action {
        "list" => {
            let refs = credentials::list_refs();
            if refs.is_empty() {
                println!("(no stored credentials)");
            } else {
                println!("{}", refs.join("\n"));
            }
            0
        }
        "set" => {
            let Some(cred_ref) = cred_ref else {
                return fail("usage: puppetty cred set <ref> [--stdin]");
            };
            let secret = if stdin {
                let mut buf = String::new();
                use tokio::io::AsyncReadExt;
                if tokio::io::stdin().read_to_string(&mut buf).await.is_err() {
                    return fail("cannot read stdin");
                }
                buf.trim_end_matches('\n')
                    .trim_end_matches('\r')
                    .to_string()
            } else {
                match read_hidden(&format!("Secret for \"{cred_ref}\" (input hidden): ")) {
                    Ok(s) => s,
                    Err(e) => return fail(&e),
                }
            };
            if secret.is_empty() {
                return fail("empty secret, nothing stored");
            }
            match credentials::set_credential(cred_ref, &secret) {
                Ok(()) => {
                    println!("stored credential \"{cred_ref}\"");
                    0
                }
                Err(e) => fail(&e),
            }
        }
        "rm" => {
            let Some(cred_ref) = cred_ref else {
                return fail("usage: puppetty cred rm <ref>");
            };
            if credentials::delete_credential(cred_ref) {
                println!("removed \"{cred_ref}\"");
            } else {
                println!("\"{cred_ref}\" not found");
            }
            0
        }
        _ => fail("usage: puppetty cred set|list|rm <ref>"),
    }
}

async fn cmd_config(action: &str) -> i32 {
    match action {
        "show" => {
            let cwd = std::env::current_dir()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_else(|_| ".".into());
            let p: Policy = match load_policy(&cwd) {
                Ok(p) => p,
                Err(e) => return fail(&e),
            };
            // Include disabled rules (with the flag) so a GUI can offer
            // enable/disable; the autopilot uses the compiled set only.
            let rules: Vec<Value> = p
                .rules
                .iter()
                .map(|r| {
                    json!({
                        "name": r.name, "match": r.pattern, "flags": r.flags,
                        "action": r.action, "class": r.class, "ref": r.cred_ref,
                        "text": r.text, "scope": r.scope, "ai": r.ai,
                        "describe": r.describe, "enter": r.enter,
                        "disabled": r.disabled == Some(true),
                    })
                })
                .collect();
            let out = json!({
                "rules": rules,
                "dangerWords": p.danger_words,
                "onDanger": p.on_danger,
                "onUnanswered": { "afterSec": p.on_unanswered.after_sec, "do": p.on_unanswered.action },
                "sources": { "user": p.sources.0, "project": p.sources.1 },
                "userConfigPath": user_config_path().to_string_lossy(),
            });
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
            0
        }
        "validate" => {
            use tokio::io::AsyncReadExt;
            let mut text = String::new();
            if tokio::io::stdin().read_to_string(&mut text).await.is_err() {
                return fail("cannot read stdin");
            }
            let check = || -> Result<(), String> {
                let obj = parse_jsonc(&text)?;
                if let Some(rules) = obj.get("rules").and_then(|r| r.as_array()) {
                    for r in rules {
                        let pattern = r["match"].as_str().unwrap_or("");
                        let flags = r["flags"].as_str().unwrap_or("");
                        policy::compile_pattern(pattern, flags)?;
                    }
                }
                Ok(())
            };
            match check() {
                Ok(()) => {
                    println!("ok");
                    0
                }
                Err(e) => {
                    eprintln!("invalid: {e}");
                    1
                }
            }
        }
        _ => fail("usage: puppetty config show|validate"),
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    fn norm(args: &[&str]) -> Vec<String> {
        normalize_argv(args.iter().map(|s| s.to_string()).collect())
    }

    /// v0.2.0 feedback: `run --help` must show help, not be swallowed by the
    /// trailing command capture.
    #[test]
    fn run_help_is_recognized() {
        for args in [
            ["puppetty-engine", "run", "--help"].as_slice(),
            ["puppetty-engine", "--help"].as_slice(),
        ] {
            match Cli::try_parse_from(args.iter().copied()) {
                Err(e) => assert_eq!(e.kind(), clap::error::ErrorKind::DisplayHelp),
                Ok(_) => panic!("--help did not trigger help for {args:?}"),
            }
        }
    }

    #[test]
    fn implicit_run_inserts_only_for_unknown_commands() {
        assert_eq!(norm(&["puppetty", "python"])[1], "run");
        assert_eq!(norm(&["puppetty", "-d", "python"])[1], "run");
        assert_eq!(norm(&["puppetty", "run", "python"])[1], "run");
        assert_eq!(norm(&["puppetty", "read", "x"])[1], "read");
        assert_eq!(norm(&["puppetty", "--help"])[1], "--help");
        assert_eq!(norm(&["puppetty", "-V"])[1], "-V");
    }

    #[test]
    fn keep_and_linger_parse() {
        for flag in ["--keep", "--linger"] {
            let cli =
                Cli::try_parse_from(["puppetty-engine", "run", "-d", flag, "--", "codex"]).unwrap();
            let Cmd::Run(opts) = cli.cmd else {
                panic!("expected run");
            };
            assert!(opts.keep && opts.detach);
        }
    }

    #[test]
    fn snap_parses_and_is_a_known_subcommand() {
        let cli =
            Cli::try_parse_from(["puppetty-engine", "snap", "codex", "--last", "-o", "x.svg"])
                .unwrap();
        let Cmd::Snap { name, out, last } = cli.cmd else {
            panic!("expected snap");
        };
        assert_eq!(
            (name.as_str(), out.as_deref(), last),
            ("codex", Some("x.svg"), true)
        );
        // Not in SUBCOMMANDS would turn `puppetty snap x` into `run snap x`.
        assert_eq!(norm(&["puppetty", "snap", "x"])[1], "snap");
    }

    #[test]
    fn read_last_parses() {
        let cli = Cli::try_parse_from(["puppetty-engine", "read", "codex", "--last"]).unwrap();
        let Cmd::Read { last, .. } = cli.cmd else {
            panic!("expected read");
        };
        assert!(last);
    }

    #[test]
    fn wait_quiet_ms_parses() {
        let cli = Cli::try_parse_from([
            "puppetty-engine",
            "wait",
            "x",
            "--prompt",
            "--quiet-ms",
            "1500",
        ])
        .unwrap();
        let Cmd::Wait {
            prompt, quiet_ms, ..
        } = cli.cmd
        else {
            panic!("expected wait");
        };
        assert!(prompt);
        assert_eq!(quiet_ms, Some(1500));
    }
}
