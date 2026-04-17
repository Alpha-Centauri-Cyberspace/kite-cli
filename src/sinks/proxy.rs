use anyhow::Result;
use cloudevents::{AttributesReader, Event};
use reqwest::Client;
use std::collections::HashMap;
use std::time::Instant;

use kite_protocol::extensions::get_kiteoriginalheaders;

use super::{Sink, SinkResult};

pub struct ProxySink {
    default_target: Option<String>,
    routes: HashMap<String, String>,
    client: Client,
}

pub(crate) fn derive_source_from_event_type(event_type: &str) -> Option<String> {
    event_type
        .strip_prefix("com.")
        .and_then(|s| s.split('.').next())
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn derive_team_id_from_ce_source(ce_source: &str) -> Option<String> {
    let path = reqwest::Url::parse(ce_source)
        .ok()
        .map(|url| url.path().to_string())
        .unwrap_or_else(|| ce_source.to_string());

    let segments: Vec<_> = path.split('/').filter(|s| !s.is_empty()).collect();
    for (idx, segment) in segments.iter().enumerate() {
        if *segment == "hooks" {
            let team = segments.get(idx + 1)?;
            let source = segments.get(idx + 2)?;
            if !team.is_empty() && !source.is_empty() {
                return Some((*team).to_string());
            }
        }
    }

    None
}

fn event_body(event: &Event) -> Vec<u8> {
    event
        .data()
        .map(|d| match d {
            cloudevents::Data::Json(v) => serde_json::to_vec(v).unwrap_or_default(),
            cloudevents::Data::String(s) => s.as_bytes().to_vec(),
            cloudevents::Data::Binary(b) => b.clone(),
        })
        .unwrap_or_default()
}

fn build_forward_request(client: &Client, target: &str, event: &Event) -> reqwest::RequestBuilder {
    let original_headers = get_kiteoriginalheaders(event).unwrap_or_default();
    let mut request = client.post(target);

    for (key, value) in &original_headers {
        // Skip authorization header from original webhook — don't leak tokens to local server
        if key.eq_ignore_ascii_case("authorization") {
            continue;
        }
        request = request.header(key.as_str(), value.as_str());
    }

    // Ensure content-type is set
    if !original_headers
        .keys()
        .any(|k| k.eq_ignore_ascii_case("content-type"))
    {
        request = request.header("content-type", "application/json");
    }

    let source_name =
        derive_source_from_event_type(event.ty()).unwrap_or_else(|| "unknown".to_string());

    request = request
        .header("x-kite-source", source_name)
        .header("x-kite-event-id", event.id())
        .header("x-kite-event-type", event.ty())
        .header("ce-id", event.id())
        .header("ce-source", event.source().to_string())
        .header("ce-type", event.ty())
        .header("ce-specversion", event.specversion().to_string());

    if let Some(team_id) = derive_team_id_from_ce_source(&event.source().to_string()) {
        request = request.header("x-kite-team-id", team_id);
    }

    if let Some(time) = event.time() {
        request = request.header("ce-time", time.to_rfc3339());
    }

    request.body(event_body(event))
}

impl ProxySink {
    pub fn new(target: String) -> Self {
        Self {
            default_target: Some(target),
            routes: HashMap::new(),
            client: Client::new(),
        }
    }

    pub fn with_routes(default_target: Option<String>, routes: HashMap<String, String>) -> Self {
        Self {
            default_target,
            routes,
            client: Client::new(),
        }
    }

    fn resolve_target_for_source(&self, source: &str) -> Option<&str> {
        self.routes
            .get(source)
            .map(String::as_str)
            .or(self.default_target.as_deref())
    }
}

impl Sink for ProxySink {
    async fn start(&mut self) -> Result<()> {
        if let Some(target) = self.default_target.as_ref() {
            eprintln!("Proxying events to {target}");
        }
        Ok(())
    }

    async fn handle(&mut self, event: &Event) -> Result<SinkResult> {
        let source_name =
            derive_source_from_event_type(event.ty()).unwrap_or_else(|| "unknown".to_string());
        let source_key = source_name.to_ascii_lowercase();

        let target = if let Some(target) = self.resolve_target_for_source(&source_key) {
            target
        } else {
            let message = format!(
                "No proxy route configured for source '{source_name}' (event type: {})",
                event.ty()
            );
            eprintln!("{message}");
            return Ok(SinkResult::Retry);
        };

        let request = build_forward_request(&self.client, target, event);

        let start = Instant::now();
        match request.send().await {
            Ok(response) => {
                let status = response.status();
                let elapsed = start.elapsed();
                let now = chrono::Local::now().format("%H:%M:%S");

                eprintln!(
                    "[{now}] -> POST {} ({} {}, {}ms)",
                    target,
                    status.as_u16(),
                    status.canonical_reason().unwrap_or(""),
                    elapsed.as_millis()
                );

                if status.is_success() || status.is_redirection() {
                    Ok(SinkResult::Ok)
                } else {
                    eprintln!("  Non-success status — event queued for retry");
                    Ok(SinkResult::Retry)
                }
            }
            Err(e) => {
                let now = chrono::Local::now().format("%H:%M:%S");
                if e.is_connect() {
                    eprintln!(
                        "[{now}] -> POST {} (connection refused — event queued for retry)",
                        target
                    );
                } else {
                    eprintln!(
                        "[{now}] -> POST {} (error: {e} — event queued for retry)",
                        target
                    );
                }
                Ok(SinkResult::Retry)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cloudevents::{EventBuilder, EventBuilderV10};
    use kite_protocol::extensions::set_kiteoriginalheaders;
    use std::collections::HashMap;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    #[derive(Debug)]
    struct CapturedRequest {
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    async fn spawn_capture_server(
        status_line: &str,
    ) -> (String, oneshot::Receiver<CapturedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let status_line = status_line.to_string();
        let (tx, rx) = oneshot::channel();

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();

            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            let headers_end = loop {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    return;
                }
                raw.extend_from_slice(&buf[..n]);

                if let Some(pos) = find_bytes(&raw, b"\r\n\r\n") {
                    break pos + 4;
                }
            };

            let header_text = String::from_utf8_lossy(&raw[..headers_end]);
            let mut headers = HashMap::new();
            let mut content_length = 0usize;

            for line in header_text.lines().skip(1) {
                if line.trim().is_empty() {
                    continue;
                }
                if let Some((name, value)) = line.split_once(':') {
                    let key = name.trim().to_ascii_lowercase();
                    let val = value.trim().to_string();
                    if key == "content-length" {
                        content_length = val.parse().unwrap_or(0);
                    }
                    headers.insert(key, val);
                }
            }

            let mut body = raw[headers_end..].to_vec();
            while body.len() < content_length {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&buf[..n]);
            }
            body.truncate(content_length);

            let _ = tx.send(CapturedRequest { headers, body });

            let response =
                format!("HTTP/1.1 {status_line}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            let _ = socket.write_all(response.as_bytes()).await;
        });

        (format!("http://{addr}/webhooks"), rx)
    }

    fn sample_event() -> Event {
        let payload = serde_json::json!({ "hello": "world" });
        let event_time = chrono::DateTime::parse_from_rfc3339("2026-03-02T16:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let mut event = EventBuilderV10::new()
            .id("evt_123")
            .ty("com.github.push")
            .source("https://getkite.sh/hooks/team_abc/github")
            .time(event_time)
            .data("application/json", payload)
            .build()
            .unwrap();

        let mut headers = HashMap::new();
        headers.insert("x-original-header".to_string(), "keep-me".to_string());
        headers.insert("authorization".to_string(), "Bearer secret".to_string());
        set_kiteoriginalheaders(&mut event, &headers);

        event
    }

    #[test]
    fn derive_source_from_event_type_works() {
        assert_eq!(
            derive_source_from_event_type("com.github.push"),
            Some("github".to_string())
        );
        assert_eq!(
            derive_source_from_event_type("com.stripe.payment_intent.succeeded"),
            Some("stripe".to_string())
        );
        assert_eq!(derive_source_from_event_type("github.push"), None);
    }

    #[test]
    fn derive_team_id_from_ce_source_works() {
        assert_eq!(
            derive_team_id_from_ce_source("https://getkite.sh/hooks/team-123/github"),
            Some("team-123".to_string())
        );
        assert_eq!(
            derive_team_id_from_ce_source("/hooks/dev-team/stripe"),
            Some("dev-team".to_string())
        );
        assert_eq!(
            derive_team_id_from_ce_source("https://example.com/other/path"),
            None
        );
    }

    #[test]
    fn resolve_target_prefers_explicit_route_then_default() {
        let mut routes = HashMap::new();
        routes.insert("github".to_string(), "http://localhost:3001".to_string());
        let sink = ProxySink::with_routes(Some("http://localhost:3000".to_string()), routes);

        assert_eq!(
            sink.resolve_target_for_source("github"),
            Some("http://localhost:3001")
        );
        assert_eq!(
            sink.resolve_target_for_source("stripe"),
            Some("http://localhost:3000")
        );
    }

    #[tokio::test]
    async fn proxy_sink_injects_metadata_headers() {
        let event = sample_event();
        let expected_body = serde_json::json!({ "hello": "world" });

        let (target, rx) = spawn_capture_server("200 OK").await;
        let mut sink = ProxySink::new(target);
        let result = sink.handle(&event).await.unwrap();
        assert!(matches!(result, SinkResult::Ok));

        let captured = rx.await.unwrap();

        assert_eq!(
            captured.headers.get("x-original-header"),
            Some(&"keep-me".to_string())
        );
        assert!(!captured.headers.contains_key("authorization"));

        assert_eq!(
            captured.headers.get("x-kite-source"),
            Some(&"github".to_string())
        );
        assert_eq!(
            captured.headers.get("x-kite-event-id"),
            Some(&"evt_123".to_string())
        );
        assert_eq!(
            captured.headers.get("x-kite-event-type"),
            Some(&"com.github.push".to_string())
        );
        assert_eq!(
            captured.headers.get("x-kite-team-id"),
            Some(&"team_abc".to_string())
        );

        assert_eq!(captured.headers.get("ce-id"), Some(&"evt_123".to_string()));
        assert_eq!(
            captured.headers.get("ce-source"),
            Some(&"https://getkite.sh/hooks/team_abc/github".to_string())
        );
        assert_eq!(
            captured.headers.get("ce-type"),
            Some(&"com.github.push".to_string())
        );
        assert_eq!(
            captured.headers.get("ce-specversion"),
            Some(&"1.0".to_string())
        );
        assert_eq!(
            captured.headers.get("ce-time"),
            Some(&"2026-03-02T16:00:00+00:00".to_string())
        );

        let body_json: serde_json::Value = serde_json::from_slice(&captured.body).unwrap();
        assert_eq!(body_json, expected_body);
    }
}
