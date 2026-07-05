// puppetty-gui: Tauri shell over the puppetty session engine.
// The engine is the single source of truth (DESIGN.md D6): this backend is a
// named-pipe *client* — one attach connection per session streams PTY bytes
// to the webview; one-shot connections serve info/wait/kill.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, WriteHalf};
#[cfg(windows)]
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tokio::sync::Mutex;

#[cfg(windows)]
type SessionStream = NamedPipeClient;
#[cfg(not(windows))]
type SessionStream = tokio::net::UnixStream;

// Writer entries carry an attach generation: every window shares this one
// backend, so when a tab is torn off (detach here, re-attach from the new
// window) the old connection's reader must not broadcast a "disconnected"
// event that the new attach would misinterpret.
type WriterMap = Arc<Mutex<HashMap<String, (u64, WriteHalf<SessionStream>)>>>;

struct Writers(WriterMap);

static ATTACH_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// Mirrors engine-rs/src/protocol.rs: named pipe on Windows, Unix domain
// socket under ~/.puppetty/run elsewhere.
#[cfg(windows)]
fn pipe_path(name: &str) -> String {
    format!(r"\\.\pipe\puppetty-{}", name)
}

#[cfg(not(windows))]
fn pipe_path(name: &str) -> String {
    // Mirrors engine-rs/src/protocol.rs: sockets live in ~/.puppetty/run
    // (short and stable across login contexts), long names are hashed to
    // stay under the sun_path cap.
    let dir = puppetty_home().join("run");
    let _ = std::fs::create_dir_all(&dir);
    let file = if name.len() <= 40 {
        format!("{name}.sock")
    } else {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        name.hash(&mut h);
        let prefix: String = name.chars().take(24).collect();
        format!("{prefix}-{:016x}.sock", h.finish())
    };
    dir.join(file).to_string_lossy().into_owned()
}

// Prefer the endpoint the session host recorded in the registry (it
// survives engine-version and environment differences), else compute it.
fn endpoint_for(name: &str) -> String {
    let meta = puppetty_home().join("sessions").join(format!("{name}.json"));
    if let Ok(text) = std::fs::read_to_string(meta) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if let Some(p) = v["pipe"].as_str() {
                if !p.is_empty() {
                    return p.to_string();
                }
            }
        }
    }
    pipe_path(name)
}

fn puppetty_home() -> PathBuf {
    #[cfg(windows)]
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    #[cfg(not(windows))]
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".puppetty")
}

// GUI-process settings that must be readable BEFORE the webview exists
// (localStorage lives inside it). Currently: opt-in remote debugging.
fn gui_config_path() -> PathBuf {
    puppetty_home().join("gui.json")
}

fn read_gui_config() -> Value {
    std::fs::read_to_string(gui_config_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}))
}

// True when no window state had been saved when this process started — the
// only launch that should be auto-sized to the 120x28 default grid. Captured
// at startup because the window-state plugin writes the file on close.
static FIRST_RUN: OnceLock<bool> = OnceLock::new();

fn window_state_path() -> PathBuf {
    #[cfg(windows)]
    let base = PathBuf::from(std::env::var("APPDATA").unwrap_or_default());
    #[cfg(target_os = "macos")]
    let base = PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join("Library")
        .join("Application Support");
    #[cfg(not(any(windows, target_os = "macos")))]
    let base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(x) if !x.is_empty() => PathBuf::from(x),
        _ => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"),
    };
    base.join("dev.hinase.puppetty").join(".window-state.json")
}

#[tauri::command]
fn is_first_run() -> bool {
    *FIRST_RUN.get_or_init(|| !window_state_path().exists())
}

// Full system font enumeration lives here because the webview's Local Font
// Access API is Chromium-only (absent from WKWebView/WebKitGTK). Monospace
// comes from each face's own fixed-pitch flag. Async so the directory scan
// runs off the main thread.
#[tauri::command]
async fn list_mono_fonts() -> Vec<String> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut families: Vec<String> = db
        .faces()
        .filter(|f| f.monospaced)
        .filter_map(|f| f.families.first().map(|(name, _)| name.clone()))
        .collect();
    families.sort();
    families.dedup();
    families
}

// The shell for new tabs: PowerShell on Windows (the GUI's home platform),
// the user's $SHELL elsewhere with a per-OS fallback.
#[tauri::command]
fn default_shell() -> Vec<String> {
    #[cfg(windows)]
    {
        vec!["pwsh".to_string()]
    }
    #[cfg(not(windows))]
    {
        let fallback = if cfg!(target_os = "macos") { "/bin/zsh" } else { "/bin/sh" };
        let sh = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| fallback.to_string());
        vec![sh]
    }
}

#[tauri::command]
fn get_remote_debug() -> bool {
    read_gui_config()["remoteDebugPort"].as_u64().is_some()
}

#[tauri::command]
fn set_remote_debug(enabled: bool) -> Result<(), String> {
    let mut cfg = read_gui_config();
    let obj = cfg.as_object_mut().ok_or("bad gui.json")?;
    if enabled {
        obj.insert("remoteDebugPort".into(), 9223.into());
    } else {
        obj.remove("remoteDebugPort");
    }
    std::fs::create_dir_all(puppetty_home()).map_err(|e| e.to_string())?;
    std::fs::write(
        gui_config_path(),
        serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

// Path to the Rust engine binary (puppetty-engine), resolved once. Order:
// PUPPETTY_ENGINE env var → the bundled sidecar next to this app's exe →
// the repo dev tree (engine-rs/target) → PATH.
fn puppetty_bin() -> Result<String, String> {
    static BIN: OnceLock<Option<String>> = OnceLock::new();
    BIN.get_or_init(|| {
        let exe = if cfg!(windows) {
            "puppetty-engine.exe"
        } else {
            "puppetty-engine"
        };
        if let Ok(p) = std::env::var("PUPPETTY_ENGINE") {
            if !p.trim().is_empty() {
                return Some(p);
            }
        }
        if let Ok(me) = std::env::current_exe() {
            // current_exe() returns the path as executed — launching through
            // the ~/.local/bin symlink would make the sidecar lookup search
            // ~/.local/bin. Canonicalize back to the real install location.
            let me = me.canonicalize().unwrap_or(me);
            if let Some(dir) = me.parent() {
                let sidecar = dir.join(exe);
                if sidecar.exists() {
                    return Some(sidecar.to_string_lossy().into_owned());
                }
            }
        }
        for profile in ["release", "debug"] {
            let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../engine-rs/target")
                .join(profile)
                .join(exe);
            if dev.exists() {
                return Some(dev.to_string_lossy().into_owned());
            }
        }
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join(exe))
                .find(|p| p.is_file())
                .map(|p| p.to_string_lossy().into_owned())
        })
    })
    .clone()
    .ok_or_else(|| {
        "puppetty-engine not found — reinstall the app, or set PUPPETTY_ENGINE \
         to the path of the puppetty-engine binary"
            .to_string()
    })
}

// Run the puppetty engine CLI, optionally piping `stdin_data`, return stdout.
async fn run_cli(args: &[&str], stdin_data: Option<String>) -> Result<String, String> {
    use tokio::io::AsyncWriteExt as _;
    let mut cmd = tokio::process::Command::new(puppetty_bin()?);
    cmd.args(args);
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    if stdin_data.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    if let Some(data) = stdin_data {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(data.as_bytes()).await.map_err(|e| e.to_string())?;
            drop(stdin); // close so the CLI's stdin 'end' fires
        }
    }
    let out = child.wait_with_output().await.map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

// Named-pipe connects race the server re-creating its next instance (and a
// fresh session may not be listening yet): retry NOT_FOUND/PIPE_BUSY briefly.
#[cfg(windows)]
async fn open_pipe(name: &str) -> Result<SessionStream, String> {
    for _ in 0..30 {
        match ClientOptions::new().open(endpoint_for(name)) {
            Ok(c) => return Ok(c),
            Err(e) if matches!(e.raw_os_error(), Some(2) | Some(231)) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => return Err(format!("cannot reach session \"{name}\": {e}")),
        }
    }
    Err(format!("cannot reach session \"{name}\": not responding"))
}

// Unix equivalent: the socket file may not exist yet (fresh session) or the
// listener may not be accepting yet: retry NotFound/ConnectionRefused briefly.
#[cfg(not(windows))]
async fn open_pipe(name: &str) -> Result<SessionStream, String> {
    use std::io::ErrorKind;
    for _ in 0..30 {
        match tokio::net::UnixStream::connect(endpoint_for(name)).await {
            Ok(c) => return Ok(c),
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => return Err(format!("cannot reach session \"{name}\": {e}")),
        }
    }
    Err(format!("cannot reach session \"{name}\": not responding"))
}

async fn one_shot(name: &str, req: Value, timeout_ms: u64) -> Result<Value, String> {
    let fut = async {
        let client = open_pipe(name).await?;
        let (read, mut write) = tokio::io::split(client);
        write
            .write_all(format!("{req}\n").as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        let mut line = String::new();
        BufReader::new(read)
            .read_line(&mut line)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::from_str::<Value>(&line).map_err(|e| e.to_string())
    };
    tokio::time::timeout(Duration::from_millis(timeout_ms), fut)
        .await
        .map_err(|_| format!("session \"{name}\" did not respond"))?
}

#[tauri::command]
async fn list_sessions() -> Result<Vec<Value>, String> {
    let dir = puppetty_home().join("sessions");
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Ok(out),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        if let Ok(info) = one_shot(&name, json!({"op": "info"}), 2_000).await {
            if info["ok"].as_bool() == Some(true) {
                out.push(info);
            }
        }
    }
    Ok(out)
}

#[tauri::command]
async fn start_session(
    command: Vec<String>,
    name: Option<String>,
    cwd_of: Option<String>,
    auto: Option<bool>,
) -> Result<String, String> {
    if command.is_empty() {
        return Err("empty command".into());
    }
    let mut cmd = tokio::process::Command::new(puppetty_bin()?);
    cmd.arg("run").arg("-d");
    if let Some(n) = &name {
        cmd.args(["--name", n]);
    }
    if let Some(c) = &cwd_of {
        cmd.args(["--cwd-of", c]);
    }
    // Auto-answer mode: the daemon runs the policy autopilot. A huge
    // prompt-timeout disables auto-cancel — the attached GUI human answers any
    // secret/danger prompt the autopilot won't (never a headless cancel).
    if auto.unwrap_or(false) {
        cmd.args(["--auto", "--prompt-timeout", "31536000"]);
    }
    cmd.arg("--").args(&command);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    // Read the session name from the FIRST stdout line instead of waiting for
    // EOF: on Windows the detached host inherits `run -d`'s stdout pipe, so
    // the pipe never closes and output().await would hang forever.
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut line = String::new();
    let read = tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line);
    let n = tokio::time::timeout(Duration::from_secs(20), read)
        .await
        .map_err(|_| "session did not start within 20s".to_string())?
        .map_err(|e| e.to_string())?;
    tokio::spawn(async move {
        let _ = child.wait().await; // reap; exit code carries no extra info here
    });
    let name = line.trim().to_string();
    if n == 0 || name.is_empty() {
        return Err("session failed to start".into());
    }
    Ok(name)
}

#[tauri::command]
async fn attach_session(
    app: AppHandle,
    state: State<'_, Writers>,
    name: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    {
        let writers = state.0.lock().await;
        if writers.contains_key(&name) {
            return Ok(()); // already attached
        }
    }
    let client = open_pipe(&name).await?;
    let (read, mut write) = tokio::io::split(client);
    write
        .write_all(
            format!(
                "{}\n",
                json!({"op": "attach", "source": "human-gui", "cols": cols, "rows": rows})
            )
            .as_bytes(),
        )
        .await
        .map_err(|e| e.to_string())?;
    let generation = ATTACH_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    state.0.lock().await.insert(name.clone(), (generation, write));

    let writers: WriterMap = state.0.clone();
    tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(msg) = serde_json::from_str::<Value>(&line) {
                let _ = app.emit("session-msg", json!({"name": name, "msg": msg}));
            }
        }
        // Only announce the disconnect if this connection is still the
        // registered one — a tear-off replaces it deliberately.
        let was_current = {
            let mut w = writers.lock().await;
            if w.get(&name).map(|(g, _)| *g) == Some(generation) {
                w.remove(&name);
                true
            } else {
                false
            }
        };
        if !was_current {
            return;
        }
        let _ = app.emit(
            "session-msg",
            json!({"name": name, "msg": {"event": "disconnected"}}),
        );
    });
    Ok(())
}

async fn attached_write(state: &State<'_, Writers>, name: &str, msg: Value) -> Result<(), String> {
    let mut writers = state.0.lock().await;
    let (_, write) = writers
        .get_mut(name)
        .ok_or_else(|| format!("not attached to \"{name}\""))?;
    write
        .write_all(format!("{msg}\n").as_bytes())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn write_session(state: State<'_, Writers>, name: String, data: String) -> Result<(), String> {
    attached_write(&state, &name, json!({"op": "input", "data": data})).await
}

#[tauri::command]
async fn resize_session(
    state: State<'_, Writers>,
    name: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    attached_write(&state, &name, json!({"op": "resize", "cols": cols, "rows": rows})).await
}

// Drop this backend's attach connection without touching the session (the
// engine keeps it alive). Used when a tab is torn off into a new window and
// when a window closes with tabs still open.
#[tauri::command]
async fn detach_session(state: State<'_, Writers>, name: String) -> Result<(), String> {
    state.0.lock().await.remove(&name);
    Ok(())
}

// Tear-off target: a fresh window that attaches to one existing session.
// MUST be async: a synchronous command runs on the main thread, and webview
// creation there deadlocks the event loop it needs to pump (frozen white
// window, app-wide). Async commands run on a worker thread and the builder
// dispatches to the main loop correctly.
#[tauri::command]
async fn open_session_window(
    app: AppHandle,
    name: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    static WINDOW_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let label = format!(
        "tear-{}",
        WINDOW_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let query: String = name
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    let win = tauri::WebviewWindowBuilder::new(
        &app,
        &label,
        tauri::WebviewUrl::App(format!("index.html?session={query}").into()),
    )
    .title("puppetty")
    .decorations(false)
    .transparent(true)
    // Hidden until the UI is ready — the webview paints white before the
    // app CSS loads. The frontend calls show() at the end of boot.
    .visible(false)
    .inner_size(width.max(720.0), height.max(480.0))
    .min_inner_size(720.0, 480.0)
    .build()
    .map_err(|e| e.to_string())?;
    // Drop-point placement; physical pixels to match the drag coordinates.
    let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
    Ok(())
}

#[tauri::command]
async fn kill_session(name: String) -> Result<(), String> {
    one_shot(&name, json!({"op": "kill", "source": "human-gui"}), 5_000)
        .await
        .map(|_| ())
}

// Attach/detach the policy autopilot on a live session. Returns the resulting
// auto state.
#[tauri::command]
async fn set_auto(name: String, enabled: bool) -> Result<bool, String> {
    let res = one_shot(
        &name,
        json!({"op": "set-auto", "enabled": enabled, "source": "human-gui"}),
        5_000,
    )
    .await?;
    if res["ok"].as_bool() == Some(true) {
        Ok(res["auto"].as_bool().unwrap_or(enabled))
    } else {
        Err(res["error"].as_str().unwrap_or("set-auto failed").to_string())
    }
}

// Poll-style prompt detection for the "needs input" banner and ask-human
// dialog: a short wait op. Returns the reason plus, when reason is 'prompt',
// the policy classification (class/rule/line) so the GUI can decide whether
// to pop a secure-input dialog (forbid) or a confirm dialog (confirm).
#[tauri::command]
async fn check_prompt(name: String) -> Result<Value, String> {
    let res = one_shot(
        &name,
        json!({"op": "wait", "prompt": true, "timeoutMs": 1_200, "source": "gui-poll"}),
        6_000,
    )
    .await?;
    Ok(json!({
        "reason": res["reason"].as_str().unwrap_or("unknown"),
        "promptClass": res["promptClass"],
        "promptRule": res["promptRule"],
        "promptLine": res["promptLine"],
        "promptText": res["promptText"],
    }))
}

// Decision feed: parsed tail of the session's newest .jsonl event log.
#[tauri::command]
fn read_events(name: String) -> Result<Vec<Value>, String> {
    let dir = puppetty_home().join("logs");
    let prefix = format!("{name}-");
    let newest = std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter(|e| {
            let f = e.file_name().to_string_lossy().to_string();
            f.starts_with(&prefix) && f.ends_with(".jsonl")
        })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
    let Some(newest) = newest else {
        return Ok(Vec::new());
    };
    let text = std::fs::read_to_string(newest.path()).map_err(|e| e.to_string())?;
    Ok(text
        .lines()
        .rev()
        .take(100)
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect())
}

// ---- Policy config (delegated to the Node CLI, the single source of truth) ----

#[tauri::command]
async fn config_effective() -> Result<Value, String> {
    let out = run_cli(&["config", "show"], None).await?;
    serde_json::from_str(&out).map_err(|e| e.to_string())
}

#[tauri::command]
fn config_read_user() -> Result<String, String> {
    let path = puppetty_home().join("config.json");
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
async fn config_write_user(text: String) -> Result<(), String> {
    // Validate through the engine before persisting.
    run_cli(&["config", "validate"], Some(text.clone())).await?;
    let path = puppetty_home().join("config.json");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

// ---- Credentials (delegated to the Node keyring wrapper) ----

#[tauri::command]
async fn cred_list() -> Result<Vec<String>, String> {
    let out = run_cli(&["cred", "list"], None).await?;
    if out.starts_with('(') || out.is_empty() {
        return Ok(vec![]);
    }
    Ok(out.lines().map(|l| l.to_string()).collect())
}

#[tauri::command]
async fn cred_set(reference: String, secret: String) -> Result<(), String> {
    run_cli(&["cred", "set", &reference, "--stdin"], Some(secret)).await.map(|_| ())
}

#[tauri::command]
async fn cred_rm(reference: String) -> Result<(), String> {
    run_cli(&["cred", "rm", &reference], None).await.map(|_| ())
}

// ---- AI helper (design-time regex suggestions from a user-configured CLI) ----

// Run the user's AI command (e.g. `claude -p`) through the shell so PATH and
// .cmd shims resolve, pipe `input` on stdin (same contract as the policy
// decider), and return stdout. Used by the rule editor's "Suggest regex".
#[tauri::command]
async fn ai_complete(command: String, input: String) -> Result<String, String> {
    let command = command.trim().to_string();
    if command.is_empty() {
        return Err("no AI command configured".into());
    }
    #[cfg(windows)]
    let mut cmd = {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(&command);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(&command);
        c
    };
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        drop(stdin);
    }
    let out = child.wait_with_output().await.map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() {
            "AI command failed".into()
        } else {
            err
        })
    }
}

// ---- Notifications ----

#[tauri::command]
fn notify(app: AppHandle, title: String, body: String) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| e.to_string())
}

// GUI apps launched from Finder/the dock inherit launchd's minimal PATH
// (/usr/bin:/bin:...), so user-configured commands — deciders, the AI helper
// (`claude -p`) — don't resolve. Ask the user's login shell for its PATH once
// at startup and adopt it. Markers guard against rc files that print output.
#[cfg(not(windows))]
fn adopt_login_shell_path() {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let Ok(out) = std::process::Command::new(&shell)
        .args(["-lc", "printf '__PPT[%s]TPP__' \"$PATH\""])
        .output()
    else {
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    if let Some(path) = text
        .split("__PPT[")
        .nth(1)
        .and_then(|rest| rest.split("]TPP__").next())
    {
        if !path.trim().is_empty() {
            std::env::set_var("PATH", path.trim());
        }
    }
}

// Remote debugging is a WebView2 (CDP) capability; WKWebView has no CDP and
// WebKitGTK's inspector works differently, so the Settings toggle only
// exists where it can deliver.
#[tauri::command]
fn remote_debug_supported() -> bool {
    cfg!(windows)
}

fn main() {
    #[cfg(not(windows))]
    adopt_login_shell_path();
    let _ = is_first_run(); // capture before the plugin can write the file
    // Opt-in remote debugging (CDP): WebView2 reads this env var at webview
    // creation, so it must be set before the builder runs. Toggled in
    // Settings; off by default — CDP means full control of this window.
    #[cfg(windows)]
    if let Some(port) = read_gui_config()["remoteDebugPort"].as_u64() {
        let mut args = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").unwrap_or_default();
        if !args.contains("--remote-debugging-port") {
            if !args.is_empty() {
                args.push(' ');
            }
            args.push_str(&format!("--remote-debugging-port={port}"));
            std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", &args);
        }
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(
            // Never let the plugin restore visibility: windows start hidden
            // (anti-white-flash) and the frontend reveals them when ready.
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        & !tauri_plugin_window_state::StateFlags::VISIBLE,
                )
                .build(),
        )
        .manage(Writers(Arc::new(Mutex::new(HashMap::new()))))
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            start_session,
            attach_session,
            write_session,
            resize_session,
            kill_session,
            detach_session,
            open_session_window,
            set_auto,
            check_prompt,
            read_events,
            config_effective,
            config_read_user,
            config_write_user,
            cred_list,
            cred_set,
            cred_rm,
            ai_complete,
            notify,
            get_remote_debug,
            set_remote_debug,
            is_first_run,
            default_shell,
            list_mono_fonts,
            remote_debug_supported,
        ])
        .run(tauri::generate_context!())
        .expect("error while running puppetty-gui");
}
