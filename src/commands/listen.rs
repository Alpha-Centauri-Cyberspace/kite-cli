use anyhow::Result;
use cloudevents::AttributesReader;

use crate::config::KiteConfig;
use crate::sinks::Sink;
use crate::sinks::socket::SocketSink;
use crate::ws_client;

pub async fn run(socket_path: String, source: Option<String>) -> Result<()> {
    let config = KiteConfig::load()?;
    let ws_url = config.ws_url();
    let (api_key, team_id) = config.require_auth()?;

    let mut scopes = Vec::new();
    if let Some(ref src) = source {
        scopes.push(format!("source:{src}"));
    }
    if scopes.is_empty() {
        scopes.push("*".to_string());
    }

    let mut socket_sink = SocketSink::new(socket_path.clone());
    socket_sink.start().await?;

    // Get a sender handle so the event loop closure can push events
    // through the socket sink's broadcast channel.
    let socket_tx = socket_sink.sender();

    eprintln!("Connecting to {}...", ws_url);

    let mut backoff = 1u64;
    let max_backoff = 30u64;

    loop {
        match ws_client::connect(&ws_url, &api_key, &team_id, scopes.clone(), None).await {
            Ok((_sink_ws, stream, last_seq, _client_id)) => {
                backoff = 1;
                eprintln!("Connected (last_seq: {last_seq})");

                let source_filter = source.clone();
                let socket_tx = socket_tx.clone();

                let result = ws_client::event_loop(stream, |_seq, event| {
                    let source_filter = source_filter.clone();
                    let socket_tx = socket_tx.clone();

                    async move {
                        if let Some(ref src) = source_filter {
                            let event_type = event.ty();
                            let event_source = event.source().to_string();
                            if !event_type.contains(src) && !event_source.contains(src) {
                                return Ok(());
                            }
                        }

                        // Push event through the socket sink's broadcast channel
                        let json = serde_json::to_string(&event)?;
                        let _ = socket_tx.send(json);
                        Ok(())
                    }
                })
                .await;

                if let Err(e) = result {
                    eprintln!("Disconnected: {e}");
                }
            }
            Err(e) => {
                eprintln!("Connection failed: {e}");
            }
        }

        eprintln!("Reconnecting in {backoff}s...");
        tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}
