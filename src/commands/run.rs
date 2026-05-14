use anyhow::{Result, anyhow};
use cloudevents::AttributesReader;
use std::collections::{HashMap, hash_map::Entry};
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};

use crate::config::KiteConfig;
use crate::manifest::{Manifest, SinkConfig};
use crate::queue::EventStatus;
use crate::sinks::Sink;
use crate::sinks::SinkResult;
use crate::sinks::exec::ExecSink;
use crate::sinks::mcp::{McpSink, McpSinkHandle};
use crate::sinks::paperclip::{PaperclipSink, PaperclipSinkConfig};
use crate::sinks::proxy::ProxySink;
use crate::sinks::socket::SocketSink;
use crate::sinks::stdout::StdoutSink;
use crate::ws_client::{self, AckDecision};

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

#[derive(Default)]
struct DedupWindow {
    seen: HashMap<String, i64>,
}

impl DedupWindow {
    fn should_deliver(
        &mut self,
        key: String,
        now_ts: i64,
        window_seconds: i64,
        strategy: &str,
    ) -> Result<bool> {
        self.seen
            .retain(|_, seen_at| now_ts.saturating_sub(*seen_at) <= window_seconds);

        match self.seen.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(now_ts);
                Ok(true)
            }
            Entry::Occupied(mut entry) => match strategy {
                "keep_first" => Ok(false),
                "keep_last" => {
                    entry.insert(now_ts);
                    Ok(false)
                }
                other => Err(anyhow!(
                    "unknown dedup strategy {:?} (use keep_first or keep_last)",
                    other
                )),
            },
        }
    }
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
    let queue_config = manifest.queue.clone();
    let dedup_config = scoring_config.as_ref().and_then(|s| s.dedup.clone());
    let dedup_window = Arc::new(std::sync::Mutex::new(DedupWindow::default()));
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
            Ok((sink_ws, stream, last_seq, _client_id)) => {
                backoff = 1;
                eprintln!("Connected (last_seq: {last_seq})");

                let manifest = manifest.clone();
                let sink_handle = sink_handle.clone();
                let queue = Arc::clone(&queue);
                let team_id = team_id.clone();
                let filter_config = filter_config.clone();
                let enrichment_hooks = enrichment_hooks.clone();
                let scoring_config = scoring_config.clone();
                let queue_config = queue_config.clone();
                let dedup_config = dedup_config.clone();
                let dedup_window = Arc::clone(&dedup_window);
                let min_importance = min_importance.clone();

                let result = ws_client::event_loop_with_ack(sink_ws, stream, |_seq, event| {
                    let manifest = manifest.clone();
                    let sink_handle = sink_handle.clone();
                    let queue = Arc::clone(&queue);
                    let team_id = team_id.clone();
                    let filter_config = filter_config.clone();
                    let enrichment_hooks = enrichment_hooks.clone();
                    let scoring_config = scoring_config.clone();
                    let queue_config = queue_config.clone();
                    let dedup_config = dedup_config.clone();
                    let dedup_window = Arc::clone(&dedup_window);
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
                                return Ok(AckDecision::Ack);
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
                        let raw_value = serde_json::from_str::<serde_json::Value>(&raw_json)
                            .unwrap_or_default();
                        let data = raw_value.get("data").cloned().unwrap_or_default();
                        queue.lock().unwrap().insert(
                            seq,
                            event.id(),
                            &team_id,
                            &source,
                            event.ty(),
                            &raw_json,
                            now_ts,
                        )?;
                        if let Some(ref queue_config) = queue_config {
                            queue
                                .lock()
                                .unwrap()
                                .apply_manifest_retention(queue_config, now_ts)?;
                        }

                        if let Some(ref dedup) = dedup_config {
                            let window_seconds = dedup.window_seconds()?;
                            let dedup_key =
                                dedup.key_for(&source, event.ty(), event.id(), &data)?;
                            let should_deliver = dedup_window.lock().unwrap().should_deliver(
                                dedup_key,
                                now_ts,
                                window_seconds,
                                &dedup.strategy,
                            )?;
                            if !should_deliver {
                                queue.lock().unwrap().update_status(
                                    seq,
                                    &EventStatus::Filtered,
                                    Some("duplicate within dedup window"),
                                )?;
                                return Ok(AckDecision::Ack);
                            }
                        }

                        // 2. Filter
                        if let Some(ref filters) = filter_config {
                            if filters.evaluate(&source, event.ty(), &data)
                                == crate::pipeline::filter::FilterResult::Dropped
                            {
                                queue.lock().unwrap().update_status(
                                    seq,
                                    &EventStatus::Filtered,
                                    None,
                                )?;
                                return Ok(AckDecision::Ack);
                            }
                        }

                        // 3. Enrichment
                        if let Some(ref hooks) = enrichment_hooks {
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
                                queue.lock().unwrap().update_status(
                                    seq,
                                    &EventStatus::Filtered,
                                    None,
                                )?;
                                return Ok(AckDecision::Ack);
                            }
                        }

                        // 5. Route to sink
                        let sink_result = match &sink_handle {
                            SinkHandle::Stdout { json } => {
                                let mut s = StdoutSink::new(*json, false);
                                s.handle(&event).await
                            }
                            SinkHandle::Proxy(sink) => {
                                let mut sink = sink.lock().await;
                                sink.handle(&event).await
                            }
                            SinkHandle::Socket(tx) => {
                                let json = serde_json::to_string(&event)?;
                                match tx.send(json) {
                                    Ok(_) => Ok(SinkResult::Ok),
                                    Err(e) => Err(anyhow!(e.to_string())),
                                }
                            }
                            SinkHandle::Exec(sink) => {
                                let mut sink = sink.lock().await;
                                sink.handle(&event).await
                            }
                            SinkHandle::Mcp(handle) => {
                                handle.push_event(&event).await;
                                Ok(SinkResult::Ok)
                            }
                            SinkHandle::Paperclip(sink) => {
                                let mut sink = sink.lock().await;
                                sink.handle(&event).await
                            }
                        };

                        {
                            let q = queue.lock().unwrap();
                            match sink_result {
                                Ok(SinkResult::Ok) => {
                                    q.update_status(seq, &EventStatus::Delivered, None)?
                                }
                                Ok(SinkResult::Retry) => q.update_status(
                                    seq,
                                    &EventStatus::Failed,
                                    Some("sink returned retry"),
                                )?,
                                Err(ref e) => q.update_status(
                                    seq,
                                    &EventStatus::Failed,
                                    Some(&e.to_string()),
                                )?,
                            }
                        }

                        match sink_result {
                            Ok(SinkResult::Ok) => Ok(AckDecision::Ack),
                            Ok(SinkResult::Retry) | Err(_) => Ok(AckDecision::NoAckStop),
                        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_window_keep_first_drops_duplicates_until_window_expires() {
        let mut window = DedupWindow::default();

        assert!(
            window
                .should_deliver("github|push".to_string(), 100, 10, "keep_first")
                .unwrap()
        );
        assert!(
            !window
                .should_deliver("github|push".to_string(), 105, 10, "keep_first")
                .unwrap()
        );
        assert!(
            window
                .should_deliver("github|push".to_string(), 111, 10, "keep_first")
                .unwrap()
        );
    }

    #[test]
    fn dedup_window_keep_last_refreshes_duplicate_window() {
        let mut window = DedupWindow::default();

        assert!(
            window
                .should_deliver("github|push".to_string(), 100, 10, "keep_last")
                .unwrap()
        );
        assert!(
            !window
                .should_deliver("github|push".to_string(), 105, 10, "keep_last")
                .unwrap()
        );
        assert!(
            !window
                .should_deliver("github|push".to_string(), 111, 10, "keep_last")
                .unwrap()
        );
        assert!(
            window
                .should_deliver("github|push".to_string(), 122, 10, "keep_last")
                .unwrap()
        );
    }

    #[test]
    fn dedup_window_rejects_unknown_strategy() {
        let mut window = DedupWindow::default();
        window
            .should_deliver("github|push".to_string(), 100, 10, "keep_first")
            .unwrap();

        let err = window
            .should_deliver("github|push".to_string(), 101, 10, "unknown")
            .unwrap_err();

        assert!(err.to_string().contains("unknown dedup strategy"));
    }
}
