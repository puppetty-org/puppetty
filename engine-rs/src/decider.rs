use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

// Ask an external command (a script, or an LLM CLI like `claude -p`) what to
// do about a suspected prompt. The context goes to the command's stdin; the
// command prints exactly one directive line:
//   SEND:<text> / ENTER / CANCEL / WAIT     (prompt mode)
//   CRED:<name> / CANCEL                    (credential-choice mode)
// Anything else is treated as WAIT.

const INSTRUCTIONS: &str =
    "You are supervising a terminal program that appears to be waiting for input.
Below is the tail of its output. Decide how to respond.
Reply with EXACTLY ONE line and nothing else, in one of these forms:
SEND:<text>   (type <text> and press Enter — e.g. SEND:y or SEND:my-project)
ENTER         (just press Enter)
CANCEL        (abort the program; use for password prompts or anything unsafe)
WAIT          (it is not actually waiting for input)

--- terminal output tail ---
";

fn cred_instructions(refs: &[String]) -> String {
    format!(
        "You are supervising a terminal program that is asking for a credential (a password, passphrase, or token).
Below is the tail of its output. Choose which stored credential should be used.
Available credentials — NAMES ONLY, you never see their values: {}
Reply with EXACTLY ONE line and nothing else:
CRED:<name>   (use this credential — <name> MUST be one of the list above)
CANCEL        (do not provide any credential; use if none clearly fits or it's unsafe)

--- terminal output tail ---
",
        refs.join(", ")
    )
}

pub struct Verdict {
    pub kind: VerdictKind,
    pub raw: String,
}

pub enum VerdictKind {
    Send(String),
    Enter,
    Cancel,
    Wait,
    Cred(String),
}

async fn run_decider(decider_cmd: &str, input: String, timeout: Duration) -> String {
    let run = async {
        #[cfg(windows)]
        let mut cmd = {
            let mut c = tokio::process::Command::new("cmd");
            c.arg("/C").arg(decider_cmd);
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            c
        };
        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = tokio::process::Command::new("sh");
            c.arg("-c").arg(decider_cmd);
            c
        };
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = cmd.spawn().map_err(|e| format!("(decider error: {e})"))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input.as_bytes()).await;
            drop(stdin);
        }
        let out = child
            .wait_with_output()
            .await
            .map_err(|e| format!("(decider error: {e})"))?;
        Ok::<String, String>(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    match tokio::time::timeout(timeout, run).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => e,
        Err(_) => "(decider timeout)".into(),
    }
}

pub async fn ask_decider(decider_cmd: &str, tail: &str) -> Verdict {
    let out = run_decider(
        decider_cmd,
        format!("{INSTRUCTIONS}{tail}\n"),
        Duration::from_secs(60),
    )
    .await;
    parse_verdict(&out)
}

pub async fn ask_credential_choice(decider_cmd: &str, tail: &str, refs: &[String]) -> Verdict {
    let out = run_decider(
        decider_cmd,
        format!("{}{tail}\n", cred_instructions(refs)),
        Duration::from_secs(60),
    )
    .await;
    parse_cred_verdict(&out)
}

pub fn parse_verdict(output: &str) -> Verdict {
    let line = output
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("SEND:") || *l == "ENTER" || *l == "CANCEL" || *l == "WAIT");
    match line {
        Some(l) if l.starts_with("SEND:") => Verdict {
            kind: VerdictKind::Send(l[5..].to_string()),
            raw: l.to_string(),
        },
        Some("ENTER") => Verdict {
            kind: VerdictKind::Enter,
            raw: "ENTER".into(),
        },
        Some("CANCEL") => Verdict {
            kind: VerdictKind::Cancel,
            raw: "CANCEL".into(),
        },
        Some(l) => Verdict {
            kind: VerdictKind::Wait,
            raw: l.to_string(),
        },
        None => Verdict {
            kind: VerdictKind::Wait,
            raw: output.trim().chars().take(200).collect(),
        },
    }
}

pub fn parse_cred_verdict(output: &str) -> Verdict {
    let line = output
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("CRED:") || *l == "CANCEL");
    match line {
        Some(l) if l.starts_with("CRED:") => Verdict {
            kind: VerdictKind::Cred(l[5..].trim().to_string()),
            raw: l.to_string(),
        },
        Some(_) => Verdict {
            kind: VerdictKind::Cancel,
            raw: "CANCEL".into(),
        },
        None => Verdict {
            kind: VerdictKind::Wait,
            raw: output.trim().chars().take(200).collect(),
        },
    }
}
