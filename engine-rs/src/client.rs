use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::protocol::{meta_path, pipe_path, sessions_dir};

/// One-shot request against a session's control endpoint.
pub async fn request(name: &str, req: &Value, timeout_ms: u64) -> Result<Value, String> {
    let fut = async {
        #[cfg(windows)]
        let stream = {
            use tokio::net::windows::named_pipe::ClientOptions;
            ClientOptions::new()
                .open(pipe_path(name))
                .map_err(|e| format!("cannot reach session \"{name}\": {e}"))?
        };
        #[cfg(not(windows))]
        let stream = tokio::net::UnixStream::connect(pipe_path(name))
            .await
            .map_err(|e| format!("cannot reach session \"{name}\": {e}"))?;

        let (read_half, mut write_half) = tokio::io::split(stream);
        write_half
            .write_all(format!("{req}\n").as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        let mut line = String::new();
        BufReader::new(read_half)
            .read_line(&mut line)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::from_str::<Value>(&line).map_err(|e| e.to_string())
    };
    tokio::time::timeout(Duration::from_millis(timeout_ms), fut)
        .await
        .map_err(|_| format!("session \"{name}\" did not respond"))?
}

/// Registry sessions merged with a live info ping; stale entries removed.
pub async fn list_sessions() -> Vec<Value> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(sessions_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        let Ok(meta) = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()))
        else {
            continue;
        };
        match request(&name, &json!({ "op": "info" }), 2_000).await {
            Ok(info) if info["ok"].as_bool() == Some(true) => {
                let mut merged = meta;
                if let (Some(a), Some(b)) = (merged.as_object_mut(), info.as_object()) {
                    for (k, v) in b {
                        a.insert(k.clone(), v.clone());
                    }
                }
                out.push(merged);
            }
            _ => {
                let _ = std::fs::remove_file(meta_path(&name));
            }
        }
    }
    out
}

/// Pick a session name that is not currently live.
pub async fn free_name(base: &str) -> String {
    let live: std::collections::HashSet<String> = list_sessions()
        .await
        .iter()
        .filter_map(|s| s["name"].as_str().map(String::from))
        .collect();
    if !live.contains(base) {
        return base.to_string();
    }
    for i in 2.. {
        let candidate = format!("{base}-{i}");
        if !live.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}
