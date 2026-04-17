/// KiteSink — CLI P2P federation sink.
///
/// Forwards CloudEvents to another Kite instance's `/hooks/{team_id}/kite` endpoint
/// using `Authorization: Bearer <token>` and an HMAC-SHA256 `X-Kite-Signature`.
///
/// Intended for dev/test use. For production federation, use the server-side hub
/// (outbox worker) which has Postgres-backed durability and delivery guarantees.
///
/// On failure, returns `SinkResult::Retry` so the CLI's existing DLQ handles retries.
use anyhow::Result;
use cloudevents::{AttributesReader, Event};
use hmac::{Hmac, Mac};
use reqwest::redirect::Policy;
use sha2::Sha256;
use std::time::Duration;

use super::{Sink, SinkResult};
use kite_protocol::extensions::{
    get_kitefedchain, get_kitefedhops, get_kitefedorigin, set_kitefedchain, set_kitefedhops,
    set_kitefedorigin,
};

type HmacSha256 = Hmac<Sha256>;

/// Configuration for the KiteSink.
#[derive(Debug, Clone)]
pub struct KiteSinkConfig {
    /// Full URL to the target Kite hook endpoint, e.g.
    /// `https://kite.example.com/hooks/<team_id>/kite`
    pub target_url: String,
    /// Bearer token used in the `Authorization` header when posting to the target.
    pub target_token: String,
    /// This instance's stable identifier (used in federation chain provenance).
    pub origin_instance_id: String,
}

/// CLI sink that forwards events to another Kite instance as federated CloudEvents.
pub struct KiteSink {
    config: KiteSinkConfig,
    client: reqwest::Client,
}

impl KiteSink {
    pub fn new(config: KiteSinkConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(Policy::none())
            .build()
            .expect("failed to build KiteSink HTTP client");

        Self { config, client }
    }
}

impl Sink for KiteSink {
    async fn start(&mut self) -> Result<()> {
        eprintln!(
            "KiteSink (P2P federation): forwarding to {}",
            self.config.target_url
        );
        eprintln!("  instance-id: {}", self.config.origin_instance_id);
        eprintln!("  Note: for production use, prefer server-side federation (outbox worker).");
        Ok(())
    }

    async fn handle(&mut self, event: &Event) -> Result<SinkResult> {
        let mut outbound = event.clone();

        // Set federation origin if not already set
        if get_kitefedorigin(&outbound).is_none() {
            set_kitefedorigin(&mut outbound, &self.config.origin_instance_id);
        }

        // Loop detection: if our instance ID is already in the chain, skip
        let mut chain = get_kitefedchain(&outbound);
        if chain.contains(&self.config.origin_instance_id) {
            tracing::debug!(
                event_id = event.id(),
                instance_id = %self.config.origin_instance_id,
                "KiteSink skipping: loop detected (own instance already in kitefedchain)"
            );
            return Ok(SinkResult::Ok);
        }

        // Append our instance ID and increment hops
        chain.push(self.config.origin_instance_id.clone());
        set_kitefedchain(&mut outbound, &chain);
        let hops = get_kitefedhops(&outbound).unwrap_or(0);
        set_kitefedhops(&mut outbound, hops + 1);

        let body = serde_json::to_vec(&outbound)?;
        let signature = sign_payload(&self.config.target_token, &body);

        let resp = self
            .client
            .post(&self.config.target_url)
            .header(
                "Authorization",
                format!("Bearer {}", self.config.target_token),
            )
            .header("X-Kite-Signature", &signature)
            .header("Content-Type", "application/cloudevents+json")
            .body(body)
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                tracing::debug!(
                    event_id = event.id(),
                    target = %self.config.target_url,
                    status = r.status().as_u16(),
                    "KiteSink: event forwarded"
                );
                Ok(SinkResult::Ok)
            }
            Ok(r) => {
                let status = r.status().as_u16();
                tracing::warn!(
                    event_id = event.id(),
                    target = %self.config.target_url,
                    status,
                    "KiteSink: peer returned non-2xx, queueing for retry"
                );
                Ok(SinkResult::Retry)
            }
            Err(e) => {
                tracing::warn!(
                    event_id = event.id(),
                    target = %self.config.target_url,
                    error = %e,
                    "KiteSink: delivery failed, queueing for retry"
                );
                Ok(SinkResult::Retry)
            }
        }
    }
}

fn sign_payload(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key size");
    mac.update(body);
    let result = mac.finalize().into_bytes();
    format!("sha256={}", hex_encode(&result))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::sign_payload;

    #[test]
    fn sign_payload_is_deterministic() {
        let sig1 = sign_payload("secret", b"hello world");
        let sig2 = sign_payload("secret", b"hello world");
        assert_eq!(sig1, sig2);
        assert!(sig1.starts_with("sha256="));
    }

    #[test]
    fn sign_payload_differs_on_different_input() {
        let sig1 = sign_payload("secret", b"hello");
        let sig2 = sign_payload("secret", b"world");
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn sign_payload_differs_on_different_key() {
        let sig1 = sign_payload("key1", b"payload");
        let sig2 = sign_payload("key2", b"payload");
        assert_ne!(sig1, sig2);
    }
}
