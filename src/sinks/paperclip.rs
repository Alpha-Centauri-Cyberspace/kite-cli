use anyhow::Result;
use cloudevents::{AttributesReader, Event};
use reqwest::Client;
use serde::Serialize;
use std::time::Instant;

use super::{Sink, SinkResult};

/// Configuration for the Paperclip sink.
pub struct PaperclipSinkConfig {
    /// Paperclip API base URL (e.g. `https://api.paperclip.ing`).
    pub api_url: String,
    /// Bearer token for Paperclip API authentication.
    pub api_key: String,
    /// Paperclip company ID to scope deliveries.
    pub company_id: String,
    /// Optional agent ID — when set, deliveries target this specific agent.
    pub agent_id: Option<String>,
}

pub struct PaperclipSink {
    config: PaperclipSinkConfig,
    client: Client,
}

/// Maps a Kite CloudEvent type to a Paperclip wake reason.
///
/// The wake reason tells the receiving agent *why* it was woken so it can
/// prioritise work accordingly.
fn map_wake_reason(event_type: &str) -> &str {
    match event_type {
        "kite.subscription.payment_failed" => "payment_failed",
        "kite.subscription.quota_exhausted" => "quota_exhausted",
        "kite.subscription.quota_restored" => "quota_restored",
        t if t.starts_with("com.github.") => "webhook_github",
        t if t.starts_with("com.stripe.") => "webhook_stripe",
        t if t.starts_with("com.clerk.") => "webhook_clerk",
        t if t.starts_with("com.slack.") => "webhook_slack",
        _ => "webhook_event",
    }
}

/// Payload sent to the Paperclip webhook ingestion endpoint.
#[derive(Debug, Serialize)]
struct PaperclipWebhookPayload {
    /// The original CloudEvent type (e.g. `com.github.push`).
    event_type: String,
    /// Mapped wake reason for agent routing.
    wake_reason: String,
    /// CloudEvent source URI.
    source: String,
    /// CloudEvent ID for deduplication.
    event_id: String,
    /// ISO-8601 timestamp from the CloudEvent.
    #[serde(skip_serializing_if = "Option::is_none")]
    event_time: Option<String>,
    /// The event data payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

impl PaperclipSink {
    pub fn new(config: PaperclipSinkConfig) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("failed to build HTTP client for Paperclip sink");

        Self { config, client }
    }

    /// Build the delivery URL.
    ///
    /// When an agent_id is configured, events are posted to the agent-scoped
    /// webhook endpoint which triggers a heartbeat for that specific agent.
    /// Otherwise events go to the company-scoped endpoint for fan-out routing.
    fn delivery_url(&self) -> String {
        let base = self.config.api_url.trim_end_matches('/');
        if let Some(ref agent_id) = self.config.agent_id {
            format!(
                "{base}/api/companies/{}/agents/{}/webhooks/kite",
                self.config.company_id, agent_id
            )
        } else {
            format!(
                "{base}/api/companies/{}/webhooks/kite",
                self.config.company_id
            )
        }
    }
}

impl Sink for PaperclipSink {
    async fn start(&mut self) -> Result<()> {
        let url = self.delivery_url();
        eprintln!("Paperclip sink: delivering events to {url}");
        Ok(())
    }

    async fn handle(&mut self, event: &Event) -> Result<SinkResult> {
        let event_type = event.ty().to_string();
        let wake_reason = map_wake_reason(&event_type).to_string();
        let source = event.source().to_string();
        let event_id = event.id().to_string();
        let event_time = event.time().map(|t| t.to_rfc3339());

        let data = event.data().map(|d| match d {
            cloudevents::Data::Json(v) => v.clone(),
            cloudevents::Data::String(s) => serde_json::Value::String(s.clone()),
            cloudevents::Data::Binary(b) => serde_json::Value::String(base64_encode(b)),
        });

        let payload = PaperclipWebhookPayload {
            event_type: event_type.clone(),
            wake_reason: wake_reason.clone(),
            source,
            event_id: event_id.clone(),
            event_time,
            data,
        };

        let url = self.delivery_url();
        let start = Instant::now();

        let result = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("X-Kite-Event-Id", &event_id)
            .header("X-Kite-Event-Type", &event_type)
            .header("X-Kite-Wake-Reason", &wake_reason)
            .json(&payload)
            .send()
            .await;

        let elapsed = start.elapsed();
        let now = chrono::Local::now().format("%H:%M:%S");

        match result {
            Ok(response) => {
                let status = response.status();
                eprintln!(
                    "[{now}] -> POST {url} ({} {}, {}ms)",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or(""),
                    elapsed.as_millis()
                );

                if status.is_success() {
                    Ok(SinkResult::Ok)
                } else {
                    eprintln!("  Non-success status -- event queued for retry");
                    Ok(SinkResult::Retry)
                }
            }
            Err(e) => {
                if e.is_connect() {
                    eprintln!(
                        "[{now}] -> POST {url} (connection refused -- event queued for retry)"
                    );
                } else {
                    eprintln!("[{now}] -> POST {url} (error: {e} -- event queued for retry)");
                }
                Ok(SinkResult::Retry)
            }
        }
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            let _ = result.write_char('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            let _ = result.write_char('=');
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_github_event_to_wake_reason() {
        assert_eq!(map_wake_reason("com.github.push"), "webhook_github");
        assert_eq!(
            map_wake_reason("com.github.pull_request.opened"),
            "webhook_github"
        );
    }

    #[test]
    fn maps_stripe_event_to_wake_reason() {
        assert_eq!(
            map_wake_reason("com.stripe.payment_intent.succeeded"),
            "webhook_stripe"
        );
    }

    #[test]
    fn maps_kite_internal_events() {
        assert_eq!(
            map_wake_reason("kite.subscription.payment_failed"),
            "payment_failed"
        );
        assert_eq!(
            map_wake_reason("kite.subscription.quota_exhausted"),
            "quota_exhausted"
        );
        assert_eq!(
            map_wake_reason("kite.subscription.quota_restored"),
            "quota_restored"
        );
    }

    #[test]
    fn unknown_event_maps_to_generic_reason() {
        assert_eq!(map_wake_reason("com.custom.event"), "webhook_event");
        assert_eq!(map_wake_reason("something.else"), "webhook_event");
    }

    #[test]
    fn delivery_url_company_scoped() {
        let sink = PaperclipSink::new(PaperclipSinkConfig {
            api_url: "https://api.paperclip.ing".to_string(),
            api_key: "test-key".to_string(),
            company_id: "company-123".to_string(),
            agent_id: None,
        });
        assert_eq!(
            sink.delivery_url(),
            "https://api.paperclip.ing/api/companies/company-123/webhooks/kite"
        );
    }

    #[test]
    fn delivery_url_agent_scoped() {
        let sink = PaperclipSink::new(PaperclipSinkConfig {
            api_url: "https://api.paperclip.ing".to_string(),
            api_key: "test-key".to_string(),
            company_id: "company-123".to_string(),
            agent_id: Some("agent-456".to_string()),
        });
        assert_eq!(
            sink.delivery_url(),
            "https://api.paperclip.ing/api/companies/company-123/agents/agent-456/webhooks/kite"
        );
    }

    #[test]
    fn delivery_url_strips_trailing_slash() {
        let sink = PaperclipSink::new(PaperclipSinkConfig {
            api_url: "https://api.paperclip.ing/".to_string(),
            api_key: "test-key".to_string(),
            company_id: "c1".to_string(),
            agent_id: None,
        });
        assert_eq!(
            sink.delivery_url(),
            "https://api.paperclip.ing/api/companies/c1/webhooks/kite"
        );
    }
}
