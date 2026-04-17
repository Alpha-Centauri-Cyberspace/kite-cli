use anyhow::{Result, bail};
use cloudevents::AttributesReader;
use std::sync::Arc;

use crate::config::KiteConfig;

/// Validates that `--federation-target` is safe to connect to.
///
/// Allowed:
/// - Any `https://` URL (remote or local)
/// - `http://` only when the host is `localhost` or `127.x.x.x`
///
/// Rejected with a clear error message for everything else (e.g. `http://remote.example.com`).
///
/// # Examples
/// ```rust
/// assert!(validate_federation_target("https://relay.example.com/hooks/team/kite").is_ok());
/// assert!(validate_federation_target("http://localhost:9000/hooks/team/kite").is_ok());
/// assert!(validate_federation_target("http://remote.example.com/kite").is_err());
/// ```
pub fn validate_federation_target(url: &str) -> Result<()> {
    if url.is_empty() {
        bail!("--federation-target must use HTTPS for non-localhost targets (got: {url})");
    }
    let parsed = url::Url::parse(url)
        .map_err(|e| anyhow::anyhow!("--federation-target is not a valid URL: {e} (got: {url})"))?;
    match parsed.scheme() {
        "https" => Ok(()),
        "http" => {
            let host = parsed.host_str().unwrap_or("");
            if host == "localhost" || is_loopback_ipv4(host) {
                Ok(())
            } else {
                bail!("--federation-target must use HTTPS for non-localhost targets (got: {url})");
            }
        }
        _ => bail!("--federation-target must use HTTPS for non-localhost targets (got: {url})"),
    }
}

/// Returns true for IPv4 loopback addresses: 127.0.0.0/8 (i.e. 127.x.x.x).
fn is_loopback_ipv4(host: &str) -> bool {
    host.starts_with("127.")
}
use crate::queue::EventStatus;
use crate::sinks::Sink;
use crate::sinks::SinkResult;
use crate::sinks::exec::ExecSink;
use crate::sinks::kite::{KiteSink, KiteSinkConfig};
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
    federation_target: Option<String>,
    federation_token: Option<String>,
    instance_id: Option<String>,
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

    // Validate federation target URL: must be HTTPS for non-localhost targets.
    if let Some(ref url) = federation_target {
        validate_federation_target(url)?;
    }

    // Resolve stable origin ID for federation — must be constant across reconnects
    // so loop detection works correctly across the session.
    let effective_instance_id: Option<String> = federation_target.as_ref().map(|_| {
        instance_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    });

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
                let federation_target = federation_target.clone();
                let federation_token = federation_token.clone();
                let effective_instance_id = effective_instance_id.clone();

                let result = ws_client::event_loop(stream, |_seq, event| {
                    let source_filter = source_filter.clone();
                    let type_filter = type_filter.clone();
                    let exec_cmd = exec_cmd.clone();
                    let queue = Arc::clone(&queue);
                    let team_id = team_id.clone();
                    let importance_filter = importance_filter.clone();
                    let federation_target = federation_target.clone();
                    let federation_token = federation_token.clone();
                    let effective_instance_id = effective_instance_id.clone();

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
                        let result = if let Some(ref target_url) = federation_target {
                            let token = federation_token.clone().unwrap_or_default();
                            // effective_instance_id is always Some when federation_target is Some
                            let origin = effective_instance_id.clone().unwrap_or_default();
                            let mut kite_sink = KiteSink::new(KiteSinkConfig {
                                target_url: target_url.clone(),
                                target_token: token,
                                origin_instance_id: origin,
                            });
                            kite_sink.handle(&event).await?
                        } else if let Some(ref cmd) = exec_cmd {
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

#[cfg(test)]
mod tests {
    use super::validate_federation_target;

    #[test]
    fn https_remote_is_allowed() {
        assert!(validate_federation_target("https://remote.example.com/hooks/team/kite").is_ok());
    }

    #[test]
    fn http_localhost_is_allowed() {
        assert!(validate_federation_target("http://localhost/hooks/team/kite").is_ok());
    }

    #[test]
    fn http_localhost_with_port_is_allowed() {
        assert!(validate_federation_target("http://localhost:3000/hooks/team/kite").is_ok());
    }

    #[test]
    fn http_127_loopback_is_allowed() {
        assert!(validate_federation_target("http://127.0.0.1:8080/hooks/team/kite").is_ok());
    }

    #[test]
    fn http_127_other_loopback_is_allowed() {
        assert!(validate_federation_target("http://127.1.2.3/hooks/team/kite").is_ok());
    }

    #[test]
    fn http_remote_is_rejected() {
        let err =
            validate_federation_target("http://remote.example.com/hooks/team/kite").unwrap_err();
        assert!(
            err.to_string()
                .contains("--federation-target must use HTTPS")
        );
    }

    #[test]
    fn invalid_url_is_rejected() {
        let err = validate_federation_target("not a url").unwrap_err();
        assert!(err.to_string().contains("not a valid URL"));
    }

    #[test]
    fn empty_string_is_rejected() {
        let err = validate_federation_target("").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not a valid URL") || msg.contains("must use HTTPS"),
            "unexpected error: {msg}"
        );
    }
}
