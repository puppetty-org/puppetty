use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::unbounded_channel;

use crate::protocol::pipe_path;
use crate::session::Session;

/// Accept loop for the session control endpoint (named pipe on Windows,
/// Unix socket elsewhere). One JSON object per line in, one out; an `attach`
/// op upgrades the connection to a persistent event stream.
#[cfg(windows)]
pub async fn serve(session: Arc<Session>) -> std::io::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let path = pipe_path(&session.name);
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&path)?;
    loop {
        server.connect().await?;
        let conn = server;
        server = ServerOptions::new().create(&path)?;
        let session = session.clone();
        tokio::spawn(async move {
            handle_connection(conn, session).await;
        });
    }
}

#[cfg(not(windows))]
pub async fn serve(session: Arc<Session>) -> std::io::Result<()> {
    use tokio::net::UnixListener;
    let path = pipe_path(&session.name);
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    loop {
        let (conn, _) = listener.accept().await?;
        let session = session.clone();
        tokio::spawn(async move {
            handle_connection(conn, session).await;
        });
    }
}

async fn handle_connection<S>(conn: S, session: Arc<Session>)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (read_half, mut write_half) = tokio::io::split(conn);

    // Single writer task: request replies and attach events share one
    // ordered channel so they never interleave mid-line.
    let (out_tx, mut out_rx) = unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(line) = out_rx.recv().await {
            if write_half.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if write_half.write_all(b"\n").await.is_err() {
                break;
            }
        }
    });

    let mut lines = BufReader::new(read_half).lines();
    let mut attach_id: Option<u64> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let _ = out_tx.send(json!({ "ok": false, "error": e.to_string() }).to_string());
                continue;
            }
        };
        if attach_id.is_some() {
            // Attached connections accept input/resize/detach only.
            match req["op"].as_str().unwrap_or("") {
                "input" => {
                    let data = req["data"].as_str().unwrap_or("");
                    // Content is never logged: a human may be typing a secret.
                    session.log_event(
                        "stdin",
                        json!({ "bytes": data.len(), "source": req["source"].as_str().unwrap_or("attach") }),
                    );
                    session.write(data);
                }
                "resize" => {
                    let cols = req["cols"].as_u64().unwrap_or(120) as u16;
                    let rows = req["rows"].as_u64().unwrap_or(30) as u16;
                    session.resize(cols, rows);
                }
                "detach" => break,
                _ => {}
            }
        } else if req["op"].as_str() == Some("attach") {
            attach_id = Some(session.attach_client(out_tx.clone(), &req));
        } else {
            let reply = session.handle_request(req).await;
            let _ = out_tx.send(reply.to_string());
        }
    }
    if let Some(id) = attach_id {
        session.detach_client(id);
    }
    drop(out_tx);
    let _ = writer.await;
}
