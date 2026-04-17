use anyhow::Result;
use cloudevents::Event;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;
use tokio::sync::broadcast;

use super::{Sink, SinkResult};

pub struct SocketSink {
    path: PathBuf,
    event_tx: broadcast::Sender<String>,
}

impl SocketSink {
    pub fn new(path: String) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            path: PathBuf::from(path),
            event_tx,
        }
    }

    /// Get a clone of the broadcast sender so external code (e.g. event loops)
    /// can push events into the socket sink without owning it.
    pub fn sender(&self) -> broadcast::Sender<String> {
        self.event_tx.clone()
    }
}

impl Sink for SocketSink {
    async fn start(&mut self) -> Result<()> {
        // Remove existing socket file if present
        if self.path.exists() {
            std::fs::remove_file(&self.path)?;
        }

        // Create parent directory if needed
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let listener = UnixListener::bind(&self.path)?;

        // Set restrictive permissions (owner only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
        }

        eprintln!("Listening on socket: {}", self.path.display());

        let event_tx = self.event_tx.clone();

        // Spawn a task to accept connections
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        eprintln!("Socket client connected");
                        let mut rx = event_tx.subscribe();

                        tokio::spawn(async move {
                            let mut stream = stream;
                            loop {
                                match rx.recv().await {
                                    Ok(json_line) => {
                                        if stream.write_all(json_line.as_bytes()).await.is_err() {
                                            break;
                                        }
                                        if stream.write_all(b"\n").await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(broadcast::error::RecvError::Lagged(n)) => {
                                        tracing::warn!("Socket client lagged, skipped {n} events");
                                    }
                                    Err(broadcast::error::RecvError::Closed) => break,
                                }
                            }
                            eprintln!("Socket client disconnected");
                        });
                    }
                    Err(e) => {
                        tracing::error!("Socket accept error: {e}");
                    }
                }
            }
        });

        Ok(())
    }

    async fn handle(&mut self, event: &Event) -> Result<SinkResult> {
        let json = serde_json::to_string(event)?;
        // Broadcast to all connected socket clients
        let _ = self.event_tx.send(json);
        Ok(SinkResult::Ok)
    }
}
