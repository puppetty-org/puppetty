use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use serde_json::{json, Value};

use crate::policy::Logging;

// Per-session logs, same formats as the Node engine:
//   <name>-<ts>.cast   asciinema v2 — output stream only. Input is
//                      deliberately NOT recorded (non-echoed secrets must
//                      never land in a log file).
//   <name>-<ts>.jsonl  structured control events with source attribution.
//                      Attached-terminal keystrokes are byte counts only.

pub fn logs_dir() -> PathBuf {
    let dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".puppetty")
        .join("logs");
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// Retention: prune oldest-first past retentionDays / maxTotalMB.
pub fn prune(dir: &PathBuf, logging: &Logging) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, std::time::SystemTime, u64)> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some((e.path(), meta.modified().ok()?, meta.len()))
        })
        .collect();
    files.sort_by_key(|f| f.1);
    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs(logging.retention_days * 86_400);
    let mut total: u64 = files.iter().map(|f| f.2).sum();
    let max_bytes = logging.max_total_mb * 1024 * 1024;
    for (path, mtime, size) in files {
        if mtime >= cutoff && total <= max_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

/// Newest .cast log for a session (read --last). The timestamp in the file
/// name sorts lexicographically, so the max name is the newest recording;
/// the pattern is anchored so session "codex" never matches "codex-2"'s logs.
pub fn latest_cast(name: &str) -> Option<PathBuf> {
    latest_cast_in(&logs_dir(), name)
}

fn latest_cast_in(dir: &std::path::Path, name: &str) -> Option<PathBuf> {
    let re = regex::Regex::new(&format!(
        r"^{}-\d{{4}}-\d{{2}}-\d{{2}}T\d{{2}}-\d{{2}}-\d{{2}}-\d{{3}}Z\.cast$",
        regex::escape(name)
    ))
    .ok()?;
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| re.is_match(n))
        })
        .max()
}

/// Replay a .cast recording into a fresh Screen sized from its header.
pub fn replay_cast(cast: &std::path::Path) -> Result<crate::screen::Screen, String> {
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

/// Exit code recorded in the .jsonl sibling of a .cast log, if the session
/// has exited.
pub fn exit_code_for(cast: &std::path::Path) -> Option<i64> {
    let text = std::fs::read_to_string(cast.with_extension("jsonl")).ok()?;
    text.lines().rev().find_map(|line| {
        let ev = serde_json::from_str::<Value>(line).ok()?;
        if ev["type"].as_str() == Some("exit") {
            ev["exitCode"].as_i64()
        } else {
            None
        }
    })
}

pub struct EventLog {
    t0: Instant,
    cast: Mutex<File>,
    jsonl: Mutex<File>,
}

impl EventLog {
    pub fn new(
        name: &str,
        command: &str,
        cols: u16,
        rows: u16,
        logging: &Logging,
    ) -> std::io::Result<EventLog> {
        let dir = logs_dir();
        prune(&dir, logging);
        let ts = chrono::Utc::now()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            .replace([':', '.'], "-");
        let cast_path = dir.join(format!("{name}-{ts}.cast"));
        let jsonl_path = dir.join(format!("{name}-{ts}.jsonl"));
        let mut cast = File::create(&cast_path)?;
        let jsonl = File::create(&jsonl_path)?;
        let header = json!({
            "version": 2,
            "width": cols,
            "height": rows,
            "timestamp": chrono::Utc::now().timestamp(),
            "title": command,
        });
        writeln!(cast, "{header}")?;
        let log = EventLog {
            t0: Instant::now(),
            cast: Mutex::new(cast),
            jsonl: Mutex::new(jsonl),
        };
        log.event(
            "start",
            json!({ "command": command, "cols": cols, "rows": rows }),
        );
        Ok(log)
    }

    fn t(&self) -> f64 {
        (self.t0.elapsed().as_millis() as f64) / 1000.0
    }

    pub fn output(&self, data: &[u8]) {
        let line = json!([self.t(), "o", String::from_utf8_lossy(data)]);
        if let Ok(mut f) = self.cast.lock() {
            let _ = writeln!(f, "{line}");
        }
    }

    pub fn event(&self, kind: &str, detail: Value) {
        let mut obj = json!({
            "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "t": self.t(),
            "type": kind,
        });
        if let (Some(a), Some(b)) = (obj.as_object_mut(), detail.as_object()) {
            for (k, v) in b {
                a.insert(k.clone(), v.clone());
            }
        }
        if let Ok(mut f) = self.jsonl.lock() {
            let _ = writeln!(f, "{obj}");
        }
    }

    pub fn close(&self, exit_code: i32) {
        self.event("exit", json!({ "exitCode": exit_code }));
    }
}

#[cfg(test)]
mod tests {
    use super::latest_cast_in;

    #[test]
    fn latest_cast_matches_exact_session_name_and_picks_newest() {
        let dir = std::env::temp_dir().join(format!("puppetty-cast-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in [
            "codex-2026-07-06T01-00-00-000Z.cast",
            "codex-2026-07-06T02-00-00-000Z.cast",
            "codex-2-2026-07-06T03-00-00-000Z.cast", // session "codex-2", not "codex"
            "codex-2026-07-06T02-00-00-000Z.jsonl",  // sibling, not a .cast
        ] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let hit = latest_cast_in(&dir, "codex").unwrap();
        assert_eq!(
            hit.file_name().unwrap().to_str().unwrap(),
            "codex-2026-07-06T02-00-00-000Z.cast"
        );
        let hit2 = latest_cast_in(&dir, "codex-2").unwrap();
        assert_eq!(
            hit2.file_name().unwrap().to_str().unwrap(),
            "codex-2-2026-07-06T03-00-00-000Z.cast"
        );
        assert!(latest_cast_in(&dir, "nosuch").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
