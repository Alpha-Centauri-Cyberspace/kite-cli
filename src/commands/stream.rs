use anyhow::Result;
use cloudevents::AttributesReader;
use std::sync::Arc;

use crate::config::KiteConfig;
use crate::queue::EventStatus;
use crate::sinks::Sink;
use crate::sinks::SinkResult;
use crate::sinks::exec::ExecSink;
use crate::sinks::stdout::StdoutSink;
use crate::ws_client;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    source: Option<String>,
    event_type: Option<String>,
    json: bool,
    compact: bool,
    exec: Option<String>,
    client_id: Option<String>,
    importance_filter: Option<String>,
) -> Result<()> {
    let config = KiteConfig::load()?;
    let ws_url = config.ws_url();
    let (api_key, team_id) = config.require_auth()?;

    // Open queue once — durable across reconnections
    let queue = Arc::new(std::sync::Mutex::new(crate::queue::Queue::open(
        &crate::config::queue_db_path(),
    )?));

    // Build scopes from filters
    let mut scopes = Vec::new();
    if let Some(ref src) = source {
        scopes.push(format!("source:{src}"));
    }
    if let Some(ref ty) = event_type {
        scopes.push(format!("type:{ty}"));
    }
    if scopes.is_empty() {
        scopes.push("*".to_string());
    }

    eprintln!("Connecting to {}...", ws_url);

    let source_filter = source.clone();
    let type_filter = event_type.clone();

    // Connect with auto-reconnect
    let mut backoff = 1u64;
    let max_backoff = 30u64;

    loop {
        match ws_client::connect(
            &ws_url,
            &api_key,
            &team_id,
            scopes.clone(),
            client_id.clone(),
        )
        .await
        {
            Ok((_sink_ws, stream, last_seq, _assigned_client_id)) => {
                backoff = 1;
                eprintln!("Connected (last_seq: {last_seq})");

                let source_filter = source_filter.clone();
                let type_filter = type_filter.clone();
                let exec_cmd = exec.clone();
                let queue = Arc::clone(&queue);
                let team_id = team_id.clone();
                let importance_filter = importance_filter.clone();

                let result = ws_client::event_loop(stream, |_seq, event| {
                    let source_filter = source_filter.clone();
                    let type_filter = type_filter.clone();
                    let exec_cmd = exec_cmd.clone();
                    let queue = Arc::clone(&queue);
                    let team_id = team_id.clone();
                    let importance_filter = importance_filter.clone();

                    async move {
                        // Client-side filtering
                        if let Some(ref src) = source_filter {
                            let event_type = event.ty();
                            let event_source = event.source().to_string();
                            if !event_type.contains(src) && !event_source.contains(src) {
                                return Ok(());
                            }
                        }
                        if let Some(ref ty) = type_filter
                            && !event.ty().contains(ty)
                        {
                            return Ok(());
                        }

                        // Derive metadata for queue insertion
                        let source = crate::sinks::proxy::derive_source_from_event_type(event.ty())
                            .unwrap_or_else(|| "unknown".to_string());
                        let seq =
                            kite_protocol::extensions::get_kiteseq(&event).unwrap_or(0) as i64;
                        let now_ts = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64;

                        // 1. Queue the raw event
                        let raw_json = serde_json::to_string(&event)?;
                        queue.lock().unwrap().insert(
                            seq,
                            event.id(),
                            &team_id,
                            &source,
                            event.ty(),
                            &raw_json,
                            now_ts,
                        )?;

                        // 2. Importance filter (if --importance flag set)
                        if let Some(ref min_imp_str) = importance_filter
                            && let Ok(min_imp) = crate::queue::Importance::from_str(min_imp_str)
                        {
                            // For stream without scoring config, default importance is "normal"
                            // Only filter if min importance is above normal
                            if crate::queue::Importance::Normal.rank() < min_imp.rank() {
                                queue.lock().unwrap().update_status(
                                    seq,
                                    &crate::queue::EventStatus::Filtered,
                                    None,
                                )?;
                                return Ok(());
                            }
                        }

                        // Mark ready
                        queue
                            .lock()
                            .unwrap()
                            .update_status(seq, &EventStatus::Ready, None)?;

                        // 3. Deliver to sink
                        let result = if let Some(ref cmd) = exec_cmd {
                            let mut exec_sink = ExecSink::new(cmd.clone());
                            exec_sink.handle(&event).await?
                        } else {
                            let json_mode = json;
                            let compact_mode = compact;
                            let mut stdout_sink = StdoutSink::new(json_mode, compact_mode);
                            stdout_sink.handle(&event).await?
                        };

                        // 4. Update final status
                        {
                            let q = queue.lock().unwrap();
                            match result {
                                SinkResult::Ok => {
                                    q.update_status(seq, &EventStatus::Delivered, None)?
                                }
                                SinkResult::Retry => q.update_status(
                                    seq,
                                    &EventStatus::Failed,
                                    Some("sink returned retry"),
                                )?,
                            }
                        }

                        Ok(())
                    }
                })
                .await;

                if let Err(e) = result {
                    eprintln!("Disconnected: {e}");
                }
            }
            Err(e) => {
                let err_str = e.to_string();
                // Don't retry on auth failures
                if err_str.contains("Server error (4001)") || err_str.contains("Invalid token") {
                    eprintln!("Connection failed: {e}");
                    return Err(e);
                }
                eprintln!("Connection failed: {e}");
            }
        }

        eprintln!("Reconnecting in {backoff}s...");
        tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}
