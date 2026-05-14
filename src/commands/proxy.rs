use anyhow::{Result, bail};
use cloudevents::AttributesReader;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::config::KiteConfig;
use crate::queue::EventStatus;
use crate::sinks::Sink;
use crate::sinks::SinkResult;
use crate::sinks::proxy::ProxySink;
use crate::ws_client::{self, AckDecision};

fn parse_routes(route_args: Vec<String>) -> Result<HashMap<String, String>> {
    let mut routes = HashMap::new();

    for raw in route_args {
        let (source, target) = raw.split_once('=').ok_or_else(|| {
            anyhow::anyhow!("Invalid --route '{raw}'. Expected <source>=<target>")
        })?;

        let source = source.trim().to_ascii_lowercase();
        let target = target.trim().to_string();

        if source.is_empty() {
            bail!("Invalid --route '{raw}': source cannot be empty");
        }
        if target.is_empty() {
            bail!("Invalid --route '{raw}': target cannot be empty");
        }

        routes.insert(source, target);
    }

    Ok(routes)
}

pub async fn run(
    source: Option<String>,
    target: Option<String>,
    route_args: Vec<String>,
    client_id: Option<String>,
) -> Result<()> {
    let routes = parse_routes(route_args)?;
    if target.is_none() && routes.is_empty() {
        bail!("Must specify --target and/or at least one --route <source>=<target>");
    }

    let config = KiteConfig::load()?;
    let ws_url = config.ws_url();
    let (api_key, team_id) = config.require_auth()?;

    // Open queue once — durable across reconnections
    let queue = Arc::new(std::sync::Mutex::new(crate::queue::Queue::open(
        &crate::config::queue_db_path(),
    )?));

    let mut scopes = Vec::new();
    if let Some(ref src) = source {
        scopes.push(format!("source:{src}"));
    }
    if scopes.is_empty() {
        scopes.push("*".to_string());
    }

    eprintln!("Connecting to {}...", ws_url);
    if routes.is_empty() {
        if let Some(default_target) = target.as_ref() {
            eprintln!("Proxying events to {default_target}");
        }
    } else {
        eprintln!("Proxy routing enabled:");
        if let Some(default_target) = target.as_ref() {
            eprintln!("  default -> {default_target}");
        } else {
            eprintln!("  default -> (none)");
        }

        let mut route_entries: Vec<_> = routes.iter().collect();
        route_entries.sort_by(|a, b| a.0.cmp(b.0));
        for (src, route_target) in route_entries {
            eprintln!("  {src} -> {route_target}");
        }
    }

    // Create one long-lived ProxySink, shared across reconnections
    let sink = Arc::new(Mutex::new(ProxySink::with_routes(target, routes)));

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
            Ok((sink_ws, stream, last_seq, _assigned_client_id)) => {
                backoff = 1;
                eprintln!("Connected (last_seq: {last_seq})");

                let source_filter = source.clone();
                let sink = Arc::clone(&sink);
                let queue = Arc::clone(&queue);
                let team_id = team_id.clone();

                let result = ws_client::event_loop_with_ack(sink_ws, stream, |_seq, event| {
                    let source_filter = source_filter.clone();
                    let sink = Arc::clone(&sink);
                    let queue = Arc::clone(&queue);
                    let team_id = team_id.clone();

                    async move {
                        // Client-side source filtering
                        if let Some(ref src) = source_filter {
                            let event_type = event.ty();
                            let event_source = event.source().to_string();
                            if !event_type.contains(src) && !event_source.contains(src) {
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
                        {
                            let q = queue.lock().unwrap();
                            q.insert(
                                seq,
                                event.id(),
                                &team_id,
                                &source,
                                event.ty(),
                                &raw_json,
                                now_ts,
                            )?;

                            // 2. Mark ready (filter/enrichment wired in Task 16)
                            q.update_status(seq, &EventStatus::Ready, None)?;
                        }

                        // 3. Deliver to sink
                        let mut sink = sink.lock().await;
                        let result = sink.handle(&event).await;

                        // 4. Update final status based on sink result
                        {
                            let q = queue.lock().unwrap();
                            match result {
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

                        match result {
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
