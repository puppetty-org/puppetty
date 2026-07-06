use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Regression test for #53: `run -d` with piped stdout must reach EOF as
/// soon as the launcher exits. PowerShell pipelines (`$n = puppetty run -d
/// ...`) read until EOF, not process exit — if the detached host inherits
/// the pipe's write end, EOF only comes when the whole session dies and the
/// caller hangs for the session's lifetime.
#[test]
fn run_d_stdout_reaches_eof_promptly() {
    let exe = env!("CARGO_BIN_EXE_puppetty-engine");
    let name = format!("eof-test-{}", std::process::id());
    // A child that lives ~30s — far longer than the pass threshold, so a
    // leaked handle turns into a clear failure rather than a flaky pass.
    #[cfg(windows)]
    let tail = ["--", "cmd", "/c", "ping", "-n", "30", "127.0.0.1"];
    #[cfg(unix)]
    let tail = ["--", "sh", "-c", "sleep 30"];

    let mut child = Command::new(exe)
        .args(["run", "-d", "--name", &name, "--no-log"])
        .args(tail)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn run -d");

    let start = Instant::now();
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .expect("read stdout to EOF");
    let to_eof = start.elapsed();
    let _ = child.wait();

    let killed = Command::new(exe).args(["kill", &name]).status();
    assert!(
        killed.map(|s| s.success()).unwrap_or(false),
        "session should be live and killable (stdout was: {out:?})"
    );
    assert!(out.contains(&name), "run -d prints the session name");
    assert!(
        to_eof < Duration::from_secs(15),
        "stdout EOF took {to_eof:?} — a handle leaked into the detached host (#53)"
    );
}
