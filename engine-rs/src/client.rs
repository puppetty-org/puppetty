use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::protocol::pipe_path;

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
