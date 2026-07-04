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
