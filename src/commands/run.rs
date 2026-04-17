use anyhow::Result;
use cloudevents::AttributesReader;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};

use crate::config::KiteConfig;
use crate::manifest::{Manifest, SinkConfig};
use crate::queue::EventStatus;
use crate::sinks::Sink;
use crate::sinks::exec::ExecSink;
use crate::sinks::mcp::{McpSink, McpSinkHandle};
use crate::sinks::paperclip::{PaperclipSink, PaperclipSinkConfig};
use crate::sinks::proxy::ProxySink;
use crate::sinks::socket::SocketSink;
use crate::sinks::stdout::StdoutSink;
use crate::ws_client;

/// A handle to a long-lived sink that can be cloned into the event loop.
#[derive(Clone)]
enum SinkHandle {
    Stdout { json: bool },
    Proxy(Arc<Mutex<ProxySink>>),
    Socket(broadcast::Sender<String>),
    Exec(Arc<Mutex<ExecSink>>),
    Mcp(McpSinkHandle),
    Paperclip(Arc<Mutex<PaperclipSink>>),
}

pub async fn run(manifest_path: String) -> Result<()> {
    let manifest = Manifest::load(&manifest_path)?;
    eprintln!("Loaded manifest: {}", manifest.name);

    let config = KiteConfig::load()?;
    let ws_url = config.ws_url();
    let (api_key, team_id) = config.require_auth()?;

    let scopes = manifest.scopes();

    // Open queue once — durable across reconnections
    let queue = Arc::new(std::sync::Mutex::new(crate::queue::Queue::open(
        &crate::config::queue_db_path(),
    )?));

    // Extract pipeline config from manifest
    let filter_config = manifest.filters.clone();
    let enrichment_hooks = manifest.enrichment.clone();
    let scoring_config = manifest.scoring.clone();
    let min_importance: Option<crate::queue::Importance> = match &manifest.sink {
        SinkConfig::Exec { importance, .. } => importance
            .as_ref()
            .and_then(|i| crate::queue::Importance::from_str(i).ok()),
        _ => None,
    };

    // Create long-lived sink based on manifest config
    let sink_handle = match &manifest.sink {
        SinkConfig::Stdout { json } => SinkHandle::Stdout { json: *json },
        SinkConfig::Proxy { target } => {
            SinkHandle::Proxy(Arc::new(Mutex::new(ProxySink::new(target.clone()))))
        }
        SinkConfig::Socket { path } => {
            let mut socket_sink = SocketSink::new(path.clone());
            socket_sink.start().await?;
            SinkHandle::Socket(socket_sink.sender())
        }
        SinkConfig::Exec { command, .. } => {
            let mut exec_sink = ExecSink::new(command.clone());
            exec_sink.start().await?;
            SinkHandle::Exec(Arc::new(Mutex::new(exec_sink)))
        }
        SinkConfig::McpServer { buffer_size } => {
            let mut mcp_sink = McpSink::new(*buffer_size);
            let handle = mcp_sink.handle_pair();
            mcp_sink.start().await?;
            SinkHandle::Mcp(handle)
        }
        SinkConfig::Paperclip {
            api_url,
            company_id,
            agent_id,
        } => {
            let paperclip_api_key =
                std::env::var("PAPERCLIP_API_KEY").unwrap_or_else(|_| api_key.clone());
            let mut paperclip_sink = PaperclipSink::new(PaperclipSinkConfig {
                api_url: api_url.clone(),
                api_key: paperclip_api_key,
                company_id: company_id.clone(),
                agent_id: agent_id.clone(),
            });
            paperclip_sink.start().await?;
            SinkHandle::Paperclip(Arc::new(Mutex::new(paperclip_sink)))
        }
    };

    eprintln!("Connecting to {}...", ws_url);

    let mut backoff = 1u64;
    let max_backoff = 30u64;

    loop {
        match ws_client::connect(&ws_url, &api_key, &team_id, scopes.clone(), None).await {
            Ok((_sink_ws, stream, last_seq, _client_id)) => {
                backoff = 1;
                eprintln!("Connected (last_seq: {last_seq})");

                let manifest = manifest.clone();
                let sink_handle = sink_handle.clone();
                let queue = Arc::clone(&queue);
                let team_id = team_id.clone();
                let filter_config = filter_config.clone();
                let enrichment_hooks = enrichment_hooks.clone();
                let scoring_config = scoring_config.clone();
                let min_importance = min_importance.clone();

                let result = ws_client::event_loop(stream, |_seq, event| {
                    let manifest = manifest.clone();
                    let sink_handle = sink_handle.clone();
                    let queue = Arc::clone(&queue);
                    let team_id = team_id.clone();
                    let filter_config = filter_config.clone();
                    let enrichment_hooks = enrichment_hooks.clone();
                    let scoring_config = scoring_config.clone();
                    let min_importance = min_importance.clone();

                    async move {
                        // Filter by manifest subscriptions
                        let event_type = event.ty().to_string();
                        let event_source = event.source().to_string();

                        if !manifest.subscriptions.is_empty() {
                            let matches = manifest.subscriptions.iter().any(|sub| {
                                let source_match = sub
                                    .source
                                    .as_ref()
                                    .map(|s| event_source.contains(s) || event_type.contains(s))
                                    .unwrap_or(true);
                                let type_match = sub
                                    .event_type
                                    .as_ref()
                                    .map(|t| event_type.contains(t))
                                    .unwrap_or(true);
                                source_match && type_match
                            });

                            if !matches {
                                return Ok(());
                            }
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

                        // 2. Filter
                        if let Some(ref filters) = filter_config {
                            let data = serde_json::from_str::<serde_json::Value>(&raw_json)
                                .ok()
                                .and_then(|v| v.get("data").cloned())
                                .unwrap_or_default();
                            if filters.evaluate(&source, event.ty(), &data)
                                == crate::pipeline::filter::FilterResult::Dropped
                            {
                                queue.lock().unwrap().update_status(
                                    seq,
                                    &EventStatus::Filtered,
                                    None,
                                )?;
                                return Ok(());
                            }
                        }

                        // 3. Enrichment
                        if let Some(ref hooks) = enrichment_hooks {
                            let data = serde_json::from_str::<serde_json::Value>(&raw_json)
                                .ok()
                                .and_then(|v| v.get("data").cloned())
                                .unwrap_or_default();
                            if let Some(enriched) = crate::pipeline::enrich::run_enrichment(
                                hooks,
                                &source,
                                event.ty(),
                                &data,
                                &raw_json,
                            )
                            .await
                            {
                                queue.lock().unwrap().set_enriched(seq, &enriched)?;
                            } else {
                                queue.lock().unwrap().update_status(
                                    seq,
                                    &EventStatus::Ready,
                                    None,
                                )?;
                            }
                        } else {
                            queue
                                .lock()
                                .unwrap()
                                .update_status(seq, &EventStatus::Ready, None)?;
                        }

                        // 4. Scoring
                        if let Some(ref scoring) = scoring_config {
                            let data = serde_json::from_str::<serde_json::Value>(&raw_json)
                                .ok()
                                .and_then(|v| v.get("data").cloned())
                                .unwrap_or_default();

                            // Get changed_files from enrichment payload if available
                            let queued = queue.lock().unwrap().get(seq)?;
                            let changed_files: Option<Vec<String>> = queued
                                .and_then(|e| e.enriched_payload)
                                .and_then(|ep| serde_json::from_str::<serde_json::Value>(&ep).ok())
                                .and_then(|v| v.get("changed_files").cloned())
                                .and_then(|v| serde_json::from_value(v).ok());

                            let importance = crate::pipeline::score::score_event(
                                scoring,
                                &source,
                                event.ty(),
                                &data,
                                changed_files.as_deref(),
                            );
                            queue.lock().unwrap().set_importance(seq, &importance)?;

                            // Check importance filter
                            if let Some(ref min_imp) = min_importance
                                && importance.rank() < min_imp.rank()
                            {
                                return Ok(());
                            }
                        }

                        // 5. Route to sink
                        match &sink_handle {
                            SinkHandle::Stdout { json } => {
                                let mut s = StdoutSink::new(*json, false);
                                s.handle(&event).await?;
                            }
                            SinkHandle::Proxy(sink) => {
                                let mut sink = sink.lock().await;
                                sink.handle(&event).await?;
                            }
                            SinkHandle::Socket(tx) => {
                                let json = serde_json::to_string(&event)?;
                                let _ = tx.send(json);
                            }
                            SinkHandle::Exec(sink) => {
                                let mut sink = sink.lock().await;
                                sink.handle(&event).await?;
                            }
                            SinkHandle::Mcp(handle) => {
                                handle.push_event(&event).await;
                            }
                            SinkHandle::Paperclip(sink) => {
                                let mut sink = sink.lock().await;
                                sink.handle(&event).await?;
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
                eprintln!("Connection failed: {e}");
            }
        }

        eprintln!("Reconnecting in {backoff}s...");
        tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}
