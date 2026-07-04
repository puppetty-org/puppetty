mod client;
mod protocol;
mod screen;
mod server;
mod session;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use crate::protocol::{meta_path, sessions_dir};
use crate::session::{Session, SpawnOptions};

#[derive(Parser)]
#[command(
    name = "puppetty-engine",
    version,
    about = "puppetty session engine (Rust port, alpha)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start a session (attached by default; -d for detached background)
    Run {
        #[arg(short = 'd', long)]
        detached: bool,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value_t = 120)]
        cols: u16,
        #[arg(long, default_value_t = 30)]
        rows: u16,
        #[arg(long)]
        cwd: Option<String>,
        /// Start in the working directory of another session
        #[arg(long = "cwd-of")]
        cwd_of: Option<String>,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// (internal) Session host process for detached sessions
    #[command(hide = true)]
    Host {
        #[arg(long)]
        name: String,
        #[arg(long, default_value_t = 120)]
        cols: u16,
        #[arg(long, default_value_t = 30)]
        rows: u16,
        #[arg(long)]
        cwd: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
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
    /// List live sessions
    List {
        #[arg(long)]
        json: bool,
    },
    /// Print session info as JSON
    Info { name: String },
    /// Terminate a session's child process
    Kill { name: String },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match cli.cmd {
        Cmd::Run {
            detached,
            name,
            cols,
            rows,
            cwd,
            cwd_of,
            command,
        } => cmd_run(detached, name, cols, rows, cwd, cwd_of, command).await,
        Cmd::Host {
            name,
            cols,
            rows,
            cwd,
            command,
        } => host_main(name, cols, rows, cwd, command, false).await,
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
        } => cmd_read(&name, as_json, scrollback).await,
        Cmd::Wait {
            name,
            pattern,
            gone,
            stable,
            prompt,
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
                idle,
                timeout,
                since_start,
                &flags,
                as_json,
            )
            .await
        }
        Cmd::List { json: as_json } => cmd_list(as_json).await,
        Cmd::Info { name } => cmd_info(&name).await,
        Cmd::Kill { name } => {
            request_and_report(&name, json!({ "op": "kill", "source": "cli" })).await
        }
    };
    std::process::exit(code);
}

fn fail(msg: &str) -> i32 {
    eprintln!("puppetty-engine: {msg}");
    1
}

async fn request_and_report(name: &str, req: Value) -> i32 {
    match client::request(name, &req, 10_000).await {
        Ok(res) if res["ok"].as_bool() == Some(true) => 0,
        Ok(res) => fail(res["error"].as_str().unwrap_or("request failed")),
        Err(e) => fail(&e),
    }
}

// ---- run ----

fn default_name(command: &str) -> String {
    let stem = Path::new(command)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "session".into());
    if !meta_path(&stem).exists() {
        return stem;
    }
    for i in 2.. {
        let candidate = format!("{stem}-{i}");
        if !meta_path(&candidate).exists() {
            return candidate;
        }
    }
    unreachable!()
}

async fn cmd_run(
    detached: bool,
    name: Option<String>,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
    cwd_of: Option<String>,
    command: Vec<String>,
) -> i32 {
    let cwd = if let Some(target) = cwd_of {
        match client::request(&target, &json!({ "op": "info" }), 5_000).await {
            Ok(info) if info["ok"].as_bool() == Some(true) => {
                info["cwd"].as_str().unwrap_or(".").to_string()
            }
            Ok(_) | Err(_) => return fail(&format!("cannot resolve --cwd-of {target}")),
        }
    } else {
        cwd.unwrap_or_else(|| {
            std::env::current_dir()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_else(|_| ".".into())
        })
    };
    let name = name.unwrap_or_else(|| default_name(&command[0]));
    if meta_path(&name).exists() {
        return fail(&format!("session \"{name}\" already exists"));
    }

    if !detached {
        return host_main(name, cols, rows, cwd, command, true).await;
    }

    // Detached: respawn ourselves as the host, fully disowned, then wait for
    // the registry entry to appear so failures surface here.
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return fail(&e.to_string()),
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("host")
        .arg("--name")
        .arg(&name)
        .arg("--cols")
        .arg(cols.to_string())
        .arg("--rows")
        .arg(rows.to_string())
        .arg("--cwd")
        .arg(&cwd)
        .arg("--")
        .args(&command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0008 | 0x0800_0000); // DETACHED_PROCESS | CREATE_NO_WINDOW
    }
    if let Err(e) = cmd.spawn() {
        return fail(&e.to_string());
    }
    for _ in 0..50 {
        if meta_path(&name).exists() {
            println!("{name}");
            return 0;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    fail("session host did not start")
}

// ---- host (attached and detached) ----

async fn host_main(
    name: String,
    cols: u16,
    rows: u16,
    cwd: String,
    command: Vec<String>,
    attached: bool,
) -> i32 {
    let opts = SpawnOptions {
        name,
        command: command[0].clone(),
        args: command[1..].to_vec(),
        cols,
        rows,
        cwd,
        // Attached exits promptly like a normal terminal; detached lingers so
        // clients can read the final screen.
        exit_grace: Duration::from_millis(if attached { 100 } else { 3_000 }),
    };
    let session = match Session::spawn(opts) {
        Ok(s) => s,
        Err(e) => return fail(&e.to_string()),
    };
    let mut shutdown = session.shutdown.subscribe();

    let srv = session.clone();
    tokio::spawn(async move {
        let _ = server::serve(srv).await;
    });

    if attached {
        run_attached(&session).await;
    }

    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow() {
            break;
        }
    }
    if attached {
        let _ = crossterm::terminal::disable_raw_mode();
    }
    let code = session.exit_code.lock().unwrap().unwrap_or(0);
    code
}

/// Attached mode: mirror PTY output to stdout, forward raw stdin to the PTY.
async fn run_attached(session: &Arc<Session>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let _ = crossterm::terminal::enable_raw_mode();
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
                Ok(n) => s.write(&String::from_utf8_lossy(&buf[..n])),
            }
        }
    });
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
            }
            0
        }
        Ok(res) => fail(res["error"].as_str().unwrap_or("read failed")),
        Err(e) => fail(&e),
    }
}

#[allow(clippy::too_many_arguments)]
async fn cmd_wait(
    name: &str,
    pattern: Option<String>,
    gone: Option<String>,
    stable: Option<u64>,
    prompt: bool,
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
    if let Some(i) = idle {
        obj.insert("idleMs".into(), i.into());
    }
    if since_start {
        obj.insert("sinceStart".into(), true.into());
    }

    match client::request(name, &req, timeout_secs * 1_000 + 10_000).await {
        Ok(res) if res["ok"].as_bool() == Some(true) => {
            let reason = res["reason"].as_str().unwrap_or("unknown");
            if as_json {
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                print_lines(&res);
            }
            eprintln!("wait: {reason}");
            i32::from(reason == "timeout")
        }
        Ok(res) => fail(res["error"].as_str().unwrap_or("wait failed")),
        Err(e) => fail(&e),
    }
}

async fn cmd_list(as_json: bool) -> i32 {
    let mut sessions = Vec::new();
    if let Ok(entries) = std::fs::read_dir(sessions_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            if let Ok(info) = client::request(&name, &json!({ "op": "info" }), 2_000).await {
                if info["ok"].as_bool() == Some(true) {
                    sessions.push(info);
                }
            }
        }
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&Value::Array(sessions)).unwrap()
        );
    } else if sessions.is_empty() {
        println!("(no live sessions)");
    } else {
        for s in &sessions {
            println!(
                "{}\tpid {}\t{}",
                s["name"].as_str().unwrap_or("?"),
                s["pid"],
                s["command"].as_str().unwrap_or("")
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
