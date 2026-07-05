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

/// Bind the Unix control socket up front so a failure (path too long, bad
/// permissions) is fatal at session start instead of leaving a silently
/// unreachable session behind.
#[cfg(not(windows))]
pub fn bind(name: &str) -> std::io::Result<tokio::net::UnixListener> {
    use std::os::unix::fs::PermissionsExt;
    let path = pipe_path(name);
    let _ = std::fs::remove_file(&path);
    let listener = tokio::net::UnixListener::bind(&path)?;
    // Only the owner may drive the session (umask-independent).
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    Ok(listener)
}

#[cfg(not(windows))]
pub async fn serve(
    listener: tokio::net::UnixListener,
    session: Arc<Session>,
) -> std::io::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client;
    use crate::session::SpawnOptions;
    use std::time::Duration;

    /// Round-trip through the real control endpoint — named pipe on Windows,
    /// Unix domain socket elsewhere: info, send, wait-for-pattern, kill.
    #[tokio::test(flavor = "multi_thread")]
    async fn control_endpoint_round_trip() {
        let name = format!("rs-socktest-{}", std::process::id());
        #[cfg(windows)]
        let (command, args) = ("cmd", Vec::<String>::new());
        #[cfg(unix)]
        let (command, args) = ("sh", Vec::<String>::new());
        let session = Session::spawn(SpawnOptions {
            name: name.clone(),
            command: command.into(),
            args,
            cols: 80,
            rows: 24,
            cwd: ".".into(),
            exit_grace: Duration::from_millis(100),
            logger: None,
            policy: None,
        })
        .unwrap();
        let srv = session.clone();
        #[cfg(not(windows))]
        {
            let listener = bind(&name).expect("bind control endpoint");
            tokio::spawn(async move {
                let _ = serve(listener, srv).await;
            });
        }
        #[cfg(windows)]
        tokio::spawn(async move {
            let _ = serve(srv).await;
        });
        tokio::time::sleep(Duration::from_millis(300)).await;

        let info = client::request(&name, &json!({ "op": "info" }), 5_000)
            .await
            .expect("info over the control endpoint");
        assert_eq!(info["ok"].as_bool(), Some(true));
        assert_eq!(info["name"].as_str(), Some(name.as_str()));

        client::request(
            &name,
            &json!({ "op": "send", "data": "echo sock-roundtrip-99", "enter": true }),
            5_000,
        )
        .await
        .expect("send");
        let res = client::request(
            &name,
            &json!({ "op": "wait", "pattern": "sock-roundtrip-99", "timeoutMs": 10_000 }),
            15_000,
        )
        .await
        .expect("wait");
        assert_eq!(res["reason"].as_str(), Some("pattern"), "res: {res}");

        client::request(&name, &json!({ "op": "kill" }), 5_000)
            .await
            .expect("kill");
        let mut shutdown = session.shutdown.subscribe();
        tokio::time::timeout(Duration::from_secs(15), async {
            while !*shutdown.borrow() {
                shutdown.changed().await.unwrap();
            }
        })
        .await
        .expect("session shut down after kill");
    }
}
