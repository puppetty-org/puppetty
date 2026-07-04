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
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tokio::sync::Mutex;

type WriterMap = Arc<Mutex<HashMap<String, WriteHalf<NamedPipeClient>>>>;

struct Writers(WriterMap);

fn pipe_path(name: &str) -> String {
    format!(r"\\.\pipe\puppetty-{}", name)
}

fn puppetty_home() -> PathBuf {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    PathBuf::from(home).join(".puppetty")
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

async fn one_shot(name: &str, req: Value, timeout_ms: u64) -> Result<Value, String> {
    let fut = async {
        let client = ClientOptions::new()
            .open(pipe_path(name))
            .map_err(|e| format!("cannot reach session \"{name}\": {e}"))?;
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
    let out = cmd.output().await.map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
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
    let client = ClientOptions::new()
        .open(pipe_path(&name))
        .map_err(|e| format!("cannot reach session \"{name}\": {e}"))?;
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
    state.0.lock().await.insert(name.clone(), write);

    let writers: WriterMap = state.0.clone();
    tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(msg) = serde_json::from_str::<Value>(&line) {
                let _ = app.emit("session-msg", json!({"name": name, "msg": msg}));
            }
        }
        writers.lock().await.remove(&name);
        let _ = app.emit(
            "session-msg",
            json!({"name": name, "msg": {"event": "disconnected"}}),
        );
    });
    Ok(())
}

async fn attached_write(state: &State<'_, Writers>, name: &str, msg: Value) -> Result<(), String> {
    let mut writers = state.0.lock().await;
    let write = writers
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
    let mut cmd = tokio::process::Command::new("cmd");
    cmd.arg("/C").arg(&command);
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

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .manage(Writers(Arc::new(Mutex::new(HashMap::new()))))
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            start_session,
            attach_session,
            write_session,
            resize_session,
            kill_session,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running puppetty-gui");
}
