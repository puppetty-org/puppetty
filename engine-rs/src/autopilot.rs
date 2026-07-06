use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::credentials::{get_credential, list_refs};
use crate::decider::{ask_credential_choice, ask_decider, VerdictKind};
use crate::keyexpand::expand_input;
use crate::policy::{evaluate, Policy};
use crate::protocol::{cursor_at_prompt, is_promptish, MISALIGNED_CURSOR_QUIET_FACTOR};
use crate::session::Session;

const POLL_MS: u64 = 250;

pub struct PilotOptions {
    pub policy: Arc<Policy>,
    pub quiet_ms: u64,
    /// Seconds before an unanswered prompt escalates to onUnanswered.
    pub prompt_timeout: u64,
    pub decider: Option<String>,
    /// Echo activity to stderr (attached sessions only).
    pub log_stderr: bool,
}

pub struct Autopilot {
    task: tokio::task::JoinHandle<()>,
    pub cancelled: Arc<AtomicBool>,
}

impl Autopilot {
    pub fn stop(&self) {
        self.task.abort();
    }
}

/// Optional prompt-answering layer on top of a Session, driven by the loaded
/// policy: class `auto` answers directly; `confirm` and `forbid` are never
/// answered headless — they fall through to onUnanswered (a GUI routes them
/// to ask-human instead). Unmatched prompt-looking lines go to the decider.
pub fn attach_autopilot(session: Arc<Session>, opts: PilotOptions) -> Autopilot {
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();

    let task = tokio::spawn(async move {
        let log = |m: &str| {
            if opts.log_stderr {
                eprintln!("\x1b[2m[puppetty] {m}\x1b[0m");
            }
        };
        let answer = |text: &str, why: &str| {
            log(&format!("auto-answer ({why}): {:?}", text));
            session.log_event(
                "answer",
                json!({ "text": text, "by": why, "source": "autopilot" }),
            );
            session.write(text);
        };
        let cancel = |why: &str| {
            flag.store(true, Ordering::SeqCst);
            log(&format!("cancelling child ({why})"));
            session.log_event("cancel", json!({ "why": why, "source": "autopilot" }));
            session.write("\x03");
            let s = session.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                s.kill();
            });
        };

        let mut handled_state: Option<String> = None;
        let mut unanswered_since: Option<Instant> = None;
        let mut answer_counts: HashMap<String, u32> = HashMap::new();
        let mut last_seen_data = session.last_data_instant();
        let mut poll = tokio::time::interval(Duration::from_millis(POLL_MS));
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        // Mirrors the Node engine's markUnanswered: first sighting arms the
        // timer; a later sighting past promptTimeout triggers onUnanswered.
        macro_rules! mark_unanswered {
            ($reason:expr) => {{
                let reason: String = $reason;
                match unanswered_since {
                    None => {
                        unanswered_since = Some(Instant::now());
                        log(&format!(
                            "waiting for input I won't answer ({reason}); {} in {}s unless output resumes",
                            opts.policy.on_unanswered.action, opts.prompt_timeout
                        ));
                        session.log_event("prompt-unanswerable", json!({ "reason": reason }));
                    }
                    Some(since) if since.elapsed() > Duration::from_secs(opts.prompt_timeout) => {
                        unanswered_since = None;
                        if opts.policy.on_unanswered.action == "cancel" {
                            cancel(&reason);
                        }
                    }
                    Some(_) => {}
                }
            }};
        }

        loop {
            poll.tick().await;
            if session.is_exited() {
                continue;
            }
            let last_data = session.last_data_instant();
            if last_data != last_seen_data {
                last_seen_data = last_data;
                unanswered_since = None; // output resumed
            }
            if last_data.elapsed() < Duration::from_millis(opts.quiet_ms) {
                continue;
            }

            let snap = session.snapshot(false);
            let screen_text = snap.lines.join("\n");
            let line = snap
                .lines
                .iter()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
                .unwrap_or_default();
            if line.is_empty() {
                continue;
            }

            // Cursor away from the input point means the "prompt" may just
            // be paused output — hold judgment until a much longer silence.
            if !cursor_at_prompt(&snap.lines, snap.cursor_x, snap.cursor_y)
                && last_data.elapsed()
                    < Duration::from_millis(opts.quiet_ms * MISALIGNED_CURSOR_QUIET_FACTOR)
            {
                continue;
            }

            // Screen-state key: the last 500 chars (deterministic, order-reversed).
            let state_key: String = screen_text.chars().rev().take(500).collect();
            if handled_state.as_deref() == Some(&state_key) {
                if unanswered_since.is_some() {
                    mark_unanswered!("prompt still pending".to_string());
                }
                continue;
            }

            let m = evaluate(&opts.policy, &line, &screen_text);

            if let Some(m) = &m {
                // "ignore" rules: an idle shell/REPL prompt, not a question —
                // mark handled with no events, no escalation.
                if m.class == "ignore" {
                    handled_state = Some(state_key);
                    continue;
                }

                if m.class == "auto" && (m.rule.action == "send" || m.rule.action == "enter") {
                    let count = answer_counts.entry(line.clone()).or_insert(0);
                    *count += 1;
                    if *count > 3 {
                        cancel(&format!(
                            "answered the same prompt {} times — giving up",
                            *count - 1
                        ));
                        continue;
                    }
                    handled_state = Some(state_key);
                    let rule_name = m.rule.name.clone().unwrap_or_default();
                    let text = if m.rule.action == "enter" {
                        "\r".to_string()
                    } else {
                        expand_input(
                            m.rule.text.as_deref().unwrap_or(""),
                            m.rule.enter != Some(false),
                        )
                    };
                    answer(&text, &format!("rule:{rule_name}"));
                    continue;
                }

                if m.class == "credential" {
                    handled_state = Some(state_key);
                    let rule_name = m.rule.name.clone().unwrap_or_default();
                    let mut cred_ref = m.rule.cred_ref.clone();
                    // AI-decided credential: the decider picks from ref NAMES
                    // only — it never sees secret values.
                    if m.rule.ai == Some(true) && cred_ref.is_none() {
                        let refs = list_refs();
                        let decider_cmd = opts
                            .decider
                            .clone()
                            .or_else(|| {
                                m.rule.decider.as_ref().and_then(|d| {
                                    opts.policy
                                        .deciders
                                        .get(d)
                                        .and_then(|dd| dd.command.clone())
                                })
                            })
                            .or_else(|| default_decider(&opts.policy));
                        let Some(decider_cmd) = decider_cmd.filter(|_| !refs.is_empty()) else {
                            session.log_event(
                                "prompt-detected",
                                json!({
                                "line": clip(&line), "class": "credential", "rule": rule_name }),
                            );
                            mark_unanswered!(format!(
                                "rule:{rule_name} (AI credential needs a --decider and stored credentials)"));
                            continue;
                        };
                        log(&format!(
                            "asking decider to pick a credential from: {}",
                            refs.join(", ")
                        ));
                        session.log_event(
                            "decider-asked",
                            json!({ "line": clip(&line), "refs": refs }),
                        );
                        let tail: String = tail_chars(&screen_text, 2_000);
                        let verdict = ask_credential_choice(&decider_cmd, &tail, &refs).await;
                        if session.is_exited() {
                            continue;
                        }
                        session.log_event("decider-said", json!({ "verdict": clip(&verdict.raw) }));
                        match verdict.kind {
                            VerdictKind::Cred(r) if refs.contains(&r) => cred_ref = Some(r),
                            VerdictKind::Cancel => {
                                cancel("decider declined to provide a credential");
                                continue;
                            }
                            _ => {
                                mark_unanswered!(format!(
                                    "rule:{rule_name} (AI did not pick a valid credential)"
                                ));
                                continue;
                            }
                        }
                    }
                    let secret = cred_ref.as_deref().and_then(get_credential);
                    let Some(secret) = secret else {
                        session.log_event(
                            "prompt-detected",
                            json!({
                            "line": clip(&line), "class": "credential",
                            "rule": rule_name, "ref": cred_ref }),
                        );
                        mark_unanswered!(format!(
                            "rule:{rule_name} (credential {:?} not found in store)",
                            cred_ref.unwrap_or_default()
                        ));
                        continue;
                    };
                    // Log the ref, NEVER the secret; write straight to the PTY.
                    log(&format!(
                        "auto-answer (rule:{rule_name}): <credential:{}>",
                        cred_ref.as_deref().unwrap_or("")
                    ));
                    session.log_event(
                        "answer",
                        json!({
                        "by": format!("credential:{}", cred_ref.as_deref().unwrap_or("")),
                        "source": "autopilot", "redacted": true }),
                    );
                    session.write(&format!("{secret}\r"));
                    continue;
                }

                if m.class == "forbid" || m.class == "confirm" {
                    // onDanger "decider": a danger-word escalation (not an
                    // explicit confirm rule) may fall through to the LLM,
                    // which gets an extra caution preamble.
                    let danger_to_decider = m.class == "confirm"
                        && m.danger
                        && opts.policy.on_danger == "decider"
                        && (opts.decider.is_some() || default_decider(&opts.policy).is_some());
                    if !danger_to_decider {
                        handled_state = Some(state_key);
                        let rule_name = m.rule.name.clone().unwrap_or_default();
                        session.log_event(
                            "prompt-detected",
                            json!({
                            "line": clip(&line), "class": m.class, "rule": rule_name }),
                        );
                        mark_unanswered!(if m.class == "forbid" {
                            format!("rule:{rule_name} (class forbid — never automated)")
                        } else {
                            format!(
                                "rule:{rule_name} (class confirm — needs a human; no GUI attached)"
                            )
                        });
                        continue;
                    }
                }
            }

            if !is_promptish(&line) {
                handled_state = Some(state_key);
                continue;
            }

            let danger_visible = opts
                .policy
                .danger_re
                .as_ref()
                .map(|re| re.is_match(&screen_text).unwrap_or(false))
                .unwrap_or(false);

            let decider_cmd = opts
                .decider
                .clone()
                .or_else(|| {
                    m.as_ref()
                        .filter(|m| m.rule.action == "decider")
                        .and_then(|m| m.rule.decider.as_ref())
                        .and_then(|d| opts.policy.deciders.get(d))
                        .and_then(|dd| dd.command.clone())
                })
                .or_else(|| default_decider(&opts.policy));

            if let Some(decider_cmd) = decider_cmd {
                // onDanger "human": danger words visible on an unmatched
                // prompt also mean a human decides, not the LLM.
                if danger_visible && opts.policy.on_danger == "human" {
                    handled_state = Some(state_key);
                    session.log_event(
                        "prompt-detected",
                        json!({ "line": clip(&line), "class": "confirm", "danger": true }),
                    );
                    mark_unanswered!(
                        "danger words visible — needs a human (onDanger: human)".to_string()
                    );
                    continue;
                }
                handled_state = Some(state_key);
                log(&format!("asking decider about: {:?}", clip_n(&line, 80)));
                session.log_event(
                    "decider-asked",
                    json!({ "line": clip(&line), "danger": danger_visible }),
                );
                let tail = tail_chars(&screen_text, 2_000);
                let verdict = ask_decider(&decider_cmd, &tail, danger_visible).await;
                if session.is_exited() {
                    continue;
                }
                log(&format!("decider said: {}", verdict.raw));
                session.log_event("decider-said", json!({ "verdict": clip(&verdict.raw) }));
                match verdict.kind {
                    VerdictKind::Send(text) => answer(&format!("{text}\r"), "decider"),
                    VerdictKind::Enter => answer("\r", "decider"),
                    VerdictKind::Cancel => cancel("decider said CANCEL"),
                    _ => mark_unanswered!("decider said WAIT".to_string()),
                }
                continue;
            }

            handled_state = Some(state_key);
            session.log_event(
                "prompt-detected",
                json!({ "line": clip(&line), "class": "unmatched" }),
            );
            mark_unanswered!("unrecognized prompt and no decider configured".to_string());
        }
    });

    Autopilot { task, cancelled }
}

/// Config-level default LLM CLI: `deciders.default.command` in
/// ~/.puppetty/config.json, used when no --decider flag is given. Users pick
/// their own CLI (claude, codex, anything stdin→one-line); none configured
/// means rules-only automation, which is fully supported.
fn default_decider(policy: &Policy) -> Option<String> {
    policy
        .deciders
        .get("default")
        .and_then(|dd| dd.command.clone())
}

fn clip(s: &str) -> String {
    clip_n(s, 200)
}

fn clip_n(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn tail_chars(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    let start = chars.len().saturating_sub(n);
    chars[start..].iter().collect()
}
