use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::client::{self, list_sessions};

// MCP server over stdio: sessions exposed as tools so an agent drives them
// natively. Hand-rolled JSON-RPC (initialize / tools/list / tools/call) —
// the protocol surface puppetty needs is small enough to not warrant an SDK.

fn screen_text(res: &Value) -> String {
    res["lines"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|l| l.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn status_line(res: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(alive) = res["alive"].as_bool() {
        parts.push(if alive {
            "running".to_string()
        } else {
            format!("exited({})", res["exitCode"])
        });
    }
    if let Some(reason) = res["reason"].as_str() {
        parts.push(format!("wait ended: {reason}"));
    }
    if let Some(class) = res["promptClass"].as_str() {
        let rule = res["promptRule"]
            .as_str()
            .map(|r| format!(" ({r})"))
            .unwrap_or_default();
        parts.push(format!("prompt class: {class}{rule}"));
    }
    parts.join(" · ")
}

fn text_result(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }] })
}

fn screen_result(res: &Value) -> Value {
    let status = status_line(res);
    let mut text = String::new();
    if !status.is_empty() {
        text.push_str(&format!("[{status}]\n"));
    }
    text.push_str(&screen_text(res));
    text_result(if text.is_empty() {
        "(no output)".into()
    } else {
        text
    })
}

fn error_result(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": format!("error: {message}") }], "isError": true })
}

async fn req(name: &str, msg: Value, timeout_ms: u64) -> Result<Value, String> {
    let res = client::request(name, &msg, timeout_ms).await?;
    if res["ok"].as_bool() == Some(true) {
        Ok(res)
    } else {
        Err(res["error"]
            .as_str()
            .unwrap_or("request failed")
            .to_string())
    }
}

fn tool_defs() -> Value {
    let string = |desc: &str| json!({ "type": "string", "description": desc });
    let boolean = |desc: &str| json!({ "type": "boolean", "description": desc });
    let number = |desc: &str| json!({ "type": "number", "description": desc });
    json!([
        {
            "name": "puppetty_start_session",
            "title": "Start a terminal session",
            "description": "Launch a command inside a controllable pseudo-terminal (ConPTY) session that survives across tool calls. Use this for any interactive program that may prompt for input (installers, scaffolders, REPLs, TUIs, another agent). Returns the session name and its initial screen. Drive it with puppetty_send / puppetty_read / puppetty_wait.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "array", "items": { "type": "string" }, "minItems": 1,
                                 "description": "Command and args, e.g. [\"npm\",\"create\",\"vite@latest\",\"my-app\"]" },
                    "name": string("Optional session name; auto-derived from the command if omitted"),
                    "cwd": string("Working directory for the session"),
                    "cwdOf": string("Use another session's working directory (companion sessions)"),
                    "auto": boolean("Auto-answer safe prompts per the policy config (off by default)"),
                },
                "required": ["command"],
            },
        },
        {
            "name": "puppetty_send",
            "title": "Send text to a session",
            "description": "Type text into a session, followed by Enter (unless enter=false). Use this to answer a prompt or issue a command. Do NOT use this for secrets: password/passphrase prompts are classified \"forbid\" and must be handled by a human.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": string("Session name"),
                    "text": string("Text to type"),
                    "enter": boolean("Append Enter after the text (default true)"),
                },
                "required": ["name", "text"],
            },
        },
        {
            "name": "puppetty_keys",
            "title": "Send named keys to a session",
            "description": "Send special keys to navigate TUIs. Keys: enter, tab, esc, space, backspace, up, down, left, right, home, end, pageup, pagedown, ctrl-c, ctrl-d, ctrl-z.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": string("Session name"),
                    "keys": { "type": "array", "items": { "type": "string" }, "minItems": 1,
                              "description": "Keys in order, e.g. [\"down\",\"down\",\"enter\"]" },
                },
                "required": ["name", "keys"],
            },
        },
        {
            "name": "puppetty_read",
            "title": "Read a session screen",
            "description": "Return the current rendered screen of a session (what a human would see). Use to inspect state.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": string("Session name"),
                    "scrollback": boolean("Include scrollback history, not just the visible screen"),
                },
                "required": ["name"],
            },
        },
        {
            "name": "puppetty_wait",
            "title": "Wait for a session condition",
            "description": "Block until a condition is met, then return the screen. Combine conditions; the first met wins (child exit and timeout always apply). Recommended: after puppetty_send, use waitFor a known output, or gone=\"esc to interrupt\" for agent TUIs, or prompt=true to detect that the session is blocked waiting for input. Avoids fixed sleeps.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": string("Session name"),
                    "waitFor": string("Resolve when the screen matches this regex"),
                    "sinceStart": boolean("With waitFor: only match lines that changed after the wait began"),
                    "gone": string("Resolve when this regex is ABSENT from the screen"),
                    "stable": number("Resolve when the rendered screen is unchanged for N ms"),
                    "prompt": boolean("Resolve when the session settles on a prompt-looking line"),
                    "idleMs": number("Resolve after N ms of no output"),
                    "flags": string("Regex flags for waitFor/gone, e.g. \"i\""),
                    "timeoutSec": number("Give up after N seconds (default 60)"),
                },
                "required": ["name"],
            },
        },
        {
            "name": "puppetty_list",
            "title": "List sessions",
            "description": "List live puppetty sessions with their command, pid, and working directory.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "puppetty_kill",
            "title": "Kill a session",
            "description": "Terminate a session (Ctrl+C then hard kill). Use when a session is stuck or no longer needed.",
            "inputSchema": {
                "type": "object",
                "properties": { "name": string("Session name") },
                "required": ["name"],
            },
        },
    ])
}

/// Start a detached session by respawning this binary, mirroring the Node
/// MCP server (which shells out to `puppetty run -d`).
async fn spawn_detached(args: Vec<String>) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("run").arg("-d").args(&args);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    // First stdout line only — the detached host inherits `run -d`'s stdout
    // pipe on Windows, so waiting for EOF (output().await) hangs forever.
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut line = String::new();
    let read = tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line);
    let n = tokio::time::timeout(std::time::Duration::from_secs(20), read)
        .await
        .map_err(|_| "session did not start within 20s".to_string())?
        .map_err(|e| e.to_string())?;
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    let name = line.trim().to_string();
    if n == 0 || name.is_empty() {
        return Err("session failed to start".into());
    }
    Ok(name)
}

async fn call_tool(name: &str, args: &Value) -> Value {
    let result: Result<Value, String> = async {
        match name {
            "puppetty_start_session" => {
                let mut run_args = Vec::new();
                if let Some(n) = args["name"].as_str() {
                    run_args.extend(["--name".into(), n.to_string()]);
                }
                if let Some(c) = args["cwd"].as_str() {
                    run_args.extend(["--cwd".into(), c.to_string()]);
                }
                if let Some(c) = args["cwdOf"].as_str() {
                    run_args.extend(["--cwd-of".into(), c.to_string()]);
                }
                if args["auto"].as_bool() == Some(true) {
                    run_args.push("--auto".into());
                }
                run_args.push("--".into());
                for c in args["command"].as_array().cloned().unwrap_or_default() {
                    run_args.push(c.as_str().unwrap_or("").to_string());
                }
                let started = spawn_detached(run_args).await?;
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                let res = req(&started, json!({ "op": "read", "source": "mcp" }), 5_000)
                    .await
                    .ok();
                Ok(text_result(format!(
                    "session \"{started}\" started.\n{}",
                    res.map(|r| screen_text(&r)).unwrap_or_else(|| "(starting…)".into())
                )))
            }
            "puppetty_send" => {
                req(
                    args["name"].as_str().unwrap_or(""),
                    json!({ "op": "send", "data": args["text"], "enter": args["enter"].as_bool() != Some(false), "source": "mcp" }),
                    5_000,
                )
                .await?;
                Ok(text_result(format!("sent to \"{}\"", args["name"].as_str().unwrap_or(""))))
            }
            "puppetty_keys" => {
                req(
                    args["name"].as_str().unwrap_or(""),
                    json!({ "op": "keys", "keys": args["keys"], "source": "mcp" }),
                    5_000,
                )
                .await?;
                let keys = args["keys"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|k| k.as_str()).collect::<Vec<_>>().join(" "))
                    .unwrap_or_default();
                Ok(text_result(format!("sent keys to \"{}\": {keys}", args["name"].as_str().unwrap_or(""))))
            }
            "puppetty_read" => {
                let res = req(
                    args["name"].as_str().unwrap_or(""),
                    json!({ "op": "read", "scrollback": args["scrollback"].as_bool() == Some(true), "source": "mcp" }),
                    5_000,
                )
                .await?;
                Ok(screen_result(&res))
            }
            "puppetty_wait" => {
                let timeout_ms = (args["timeoutSec"].as_f64().unwrap_or(60.0) * 1000.0) as u64;
                let mut msg = json!({ "op": "wait", "source": "mcp", "timeoutMs": timeout_ms });
                let obj = msg.as_object_mut().unwrap();
                for (from, to) in [
                    ("waitFor", "pattern"),
                    ("gone", "gone"),
                    ("flags", "flags"),
                ] {
                    if let Some(v) = args[from].as_str() {
                        obj.insert(to.into(), v.into());
                    }
                }
                if args["sinceStart"].as_bool() == Some(true) {
                    obj.insert("sinceStart".into(), true.into());
                }
                if let Some(v) = args["stable"].as_u64() {
                    obj.insert("stable".into(), v.into());
                }
                if args["prompt"].as_bool() == Some(true) {
                    obj.insert("prompt".into(), true.into());
                }
                if let Some(v) = args["idleMs"].as_u64() {
                    obj.insert("idleMs".into(), v.into());
                }
                let res = req(args["name"].as_str().unwrap_or(""), msg, timeout_ms + 5_000).await?;
                Ok(screen_result(&res))
            }
            "puppetty_list" => {
                let sessions = list_sessions().await;
                if sessions.is_empty() {
                    return Ok(text_result("(no live sessions)".into()));
                }
                let text = sessions
                    .iter()
                    .map(|s| {
                        let status = if s["alive"].as_bool() == Some(true) {
                            "running".to_string()
                        } else {
                            format!("exited({})", s["exitCode"])
                        };
                        format!(
                            "{}\t{status}\t{}\t{}",
                            s["name"].as_str().unwrap_or("?"),
                            s["command"].as_str().unwrap_or(""),
                            s["cwd"].as_str().unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(text_result(text))
            }
            "puppetty_kill" => {
                req(
                    args["name"].as_str().unwrap_or(""),
                    json!({ "op": "kill", "source": "mcp" }),
                    5_000,
                )
                .await?;
                Ok(text_result(format!("killed \"{}\"", args["name"].as_str().unwrap_or(""))))
            }
            other => Err(format!("unknown tool: {other}")),
        }
    }
    .await;
    result.unwrap_or_else(|e| error_result(&e))
}

pub async fn run_mcp_server() -> i32 {
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    eprintln!("[puppetty] MCP server ready on stdio");

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = msg["id"].clone();
        let method = msg["method"].as_str().unwrap_or("");
        if id.is_null() {
            continue; // notification — nothing to answer
        }
        let result = match method {
            "initialize" => json!({
                "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05"),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "puppetty", "version": env!("CARGO_PKG_VERSION") },
            }),
            "ping" => json!({}),
            "tools/list" => json!({ "tools": tool_defs() }),
            "tools/call" => {
                call_tool(
                    msg["params"]["name"].as_str().unwrap_or(""),
                    &msg["params"]["arguments"],
                )
                .await
            }
            _ => {
                let err = json!({ "jsonrpc": "2.0", "id": id,
                    "error": { "code": -32601, "message": format!("method not found: {method}") } });
                let _ = stdout.write_all(format!("{err}\n").as_bytes()).await;
                let _ = stdout.flush().await;
                continue;
            }
        };
        let reply = json!({ "jsonrpc": "2.0", "id": id, "result": result });
        if stdout
            .write_all(format!("{reply}\n").as_bytes())
            .await
            .is_err()
        {
            break;
        }
        let _ = stdout.flush().await;
    }
    0
}
