use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::pipeline::filter::{FilterConfig, FilterRule};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
    pub sink: SinkConfig,
    #[serde(default)]
    pub filters: Option<FilterConfig>,
    #[serde(default)]
    pub enrichment: Option<Vec<EnrichmentHook>>,
    #[serde(default)]
    pub scoring: Option<ScoringConfig>,
    #[serde(default)]
    pub queue: Option<QueueConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub source: Option<String>,
    #[serde(rename = "type")]
    pub event_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SinkConfig {
    Stdout {
        #[serde(default)]
        json: bool,
    },
    Proxy {
        target: String,
    },
    Socket {
        path: String,
    },
    Exec {
        command: String,
        #[serde(default)]
        importance: Option<String>,
        #[serde(default)]
        batch: Option<String>,
    },
    McpServer {
        #[serde(default = "default_buffer_size")]
        buffer_size: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrichmentHook {
    #[serde(rename = "match")]
    pub match_rule: FilterRule,
    pub run: String,
    #[serde(default = "default_timeout")]
    pub timeout: String,
}

fn default_buffer_size() -> usize {
    100
}

fn default_timeout() -> String {
    "30s".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoringConfig {
    #[serde(default)]
    pub rules: Vec<ScoringRule>,
    #[serde(default)]
    pub dedup: Option<DedupConfig>,
    #[serde(default = "default_importance")]
    pub default_importance: String,
}

fn default_importance() -> String {
    "normal".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoringRule {
    #[serde(rename = "match")]
    pub match_rule: FilterRule,
    pub importance: String,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DedupConfig {
    pub window: String,
    pub key: Vec<String>,
    #[serde(default = "default_strategy")]
    pub strategy: String,
}

fn default_strategy() -> String {
    "keep_last".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueConfig {
    #[serde(default = "default_retention")]
    pub retention: String,
    #[serde(default)]
    pub max_size: Option<String>,
}

fn default_retention() -> String {
    "7d".to_string()
}

impl DedupConfig {
    /// Return the deduplication window in seconds.
    pub fn window_seconds(&self) -> Result<i64> {
        parse_duration_seconds(&self.window)
    }

    /// Build a stable deduplication key from queued metadata and event JSON.
    pub fn key_for(
        &self,
        source: &str,
        event_type: &str,
        event_id: &str,
        payload: &serde_json::Value,
    ) -> Result<String> {
        let mut parts = Vec::with_capacity(self.key.len());
        for field in &self.key {
            let value = dedup_field_value(field, source, event_type, event_id, payload)
                .ok_or_else(|| anyhow!("dedup key field {:?} is unavailable", field))?;
            parts.push(format!("{field}={value}"));
        }
        Ok(parts.join("|"))
    }
}

impl QueueConfig {
    /// Return the queue retention period in seconds.
    pub fn retention_seconds(&self) -> Result<i64> {
        parse_duration_seconds(&self.retention)
    }

    /// Return the configured maximum retained event count.
    pub fn max_event_count(&self) -> Result<Option<usize>> {
        self.max_size.as_deref().map(parse_count).transpose()
    }
}

pub fn parse_duration_seconds(input: &str) -> Result<i64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("duration string is empty"));
    }

    let (number, unit) = trimmed.split_at(trimmed.len() - 1);
    let amount = parse_count_i64(number)?;
    let seconds = match unit {
        "s" => amount,
        "m" => amount * 60,
        "h" => amount * 3_600,
        "d" => amount * 86_400,
        other => return Err(anyhow!("unknown duration unit {:?} (use s/m/h/d)", other)),
    };

    Ok(seconds)
}

pub fn parse_count(input: &str) -> Result<usize> {
    let count = parse_count_i64(input)?;
    usize::try_from(count).map_err(|_| anyhow!("count {:?} is too large", input))
}

fn parse_count_i64(input: &str) -> Result<i64> {
    let normalized = input.trim().replace('_', "");
    if normalized.is_empty() {
        return Err(anyhow!("count string is empty"));
    }
    let count: i64 = normalized
        .parse()
        .map_err(|_| anyhow!("invalid count {:?}", input))?;
    if count < 0 {
        return Err(anyhow!("count {:?} must be non-negative", input));
    }
    Ok(count)
}

fn dedup_field_value(
    field: &str,
    source: &str,
    event_type: &str,
    event_id: &str,
    payload: &serde_json::Value,
) -> Option<String> {
    match field {
        "source" => Some(source.to_string()),
        "type" | "event_type" => Some(event_type.to_string()),
        "event_id" | "id" => Some(event_id.to_string()),
        "ref" | "git_ref" => first_string_at(
            payload,
            &[
                &["ref"],
                &["git_ref"],
                &["data", "ref"],
                &["data", "git_ref"],
                &["pull_request", "head", "ref"],
                &["data", "pull_request", "head", "ref"],
            ],
        ),
        "actor" => first_string_at(
            payload,
            &[
                &["actor"],
                &["sender", "login"],
                &["user", "login"],
                &["author", "login"],
                &["data", "actor"],
                &["data", "sender", "login"],
                &["data", "user", "login"],
                &["data", "author", "login"],
            ],
        ),
        other => first_string_at(payload, &[&[other], &["data", other]]),
    }
}

fn first_string_at(payload: &serde_json::Value, paths: &[&[&str]]) -> Option<String> {
    paths.iter().find_map(|path| string_at(payload, path))
}

fn string_at(payload: &serde_json::Value, path: &[&str]) -> Option<String> {
    let mut current = payload;
    for segment in path {
        current = current.get(*segment)?;
    }
    current.as_str().map(ToOwned::to_owned)
}

impl Manifest {
    /// Load a manifest from a file path.
    pub fn load(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let manifest: Manifest = serde_json::from_str(&content)?;
        Ok(manifest)
    }

    /// Build subscription scope strings for the WS connect message.
    pub fn scopes(&self) -> Vec<String> {
        if self.subscriptions.is_empty() {
            return vec!["*".to_string()];
        }

        let mut scopes = Vec::new();
        for sub in &self.subscriptions {
            if let Some(ref source) = sub.source {
                scopes.push(format!("source:{source}"));
            }
            if let Some(ref event_type) = sub.event_type {
                scopes.push(format!("type:{event_type}"));
            }
        }

        if scopes.is_empty() {
            vec!["*".to_string()]
        } else {
            scopes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_old_manifest_still_parses() {
        let json = r#"{"name":"my-app","subscriptions":[{"source":"github"}],"sink":{"type":"proxy","target":"http://localhost:3000"}}"#;
        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "my-app");
        assert!(manifest.filters.is_none());
        assert!(manifest.enrichment.is_none());
        assert!(manifest.scoring.is_none());
        assert!(manifest.queue.is_none());
    }

    #[test]
    fn test_full_manifest_parses() {
        let json = r#"{
            "name": "my-app",
            "subscriptions": [{"source": "github"}],
            "filters": {"drop": [{"source": "github", "type": "com.github.ping"}]},
            "enrichment": [{"match": {"source": "github", "type": "com.github.pull_request.*"}, "run": "scripts/enrich-pr.sh", "timeout": "30s"}],
            "scoring": {"rules": [{"match": {"source": "github"}, "importance": "high", "paths": ["src/auth/*"]}], "default_importance": "normal"},
            "sink": {"type": "exec", "command": "./handle.sh", "importance": "high"},
            "queue": {"retention": "7d"}
        }"#;
        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert!(manifest.filters.is_some());
        assert_eq!(manifest.enrichment.unwrap().len(), 1);
        assert_eq!(manifest.scoring.unwrap().rules.len(), 1);
        match manifest.sink {
            SinkConfig::Exec { command, .. } => assert_eq!(command, "./handle.sh"),
            _ => panic!("Expected Exec sink"),
        }
    }

    #[test]
    fn obsolete_paperclip_sink_is_rejected() {
        let json = r#"{
            "name": "old-paperclip-app",
            "subscriptions": [{"source": "github"}],
            "sink": {
                "type": "paperclip",
                "api_url": "https://api.paperclip.ing",
                "company_id": "company-123"
            }
        }"#;

        let error =
            serde_json::from_str::<Manifest>(json).expect_err("paperclip sink should not parse");

        assert!(error.to_string().contains("unknown variant"));
    }

    #[test]
    fn queue_config_parses_retention_and_max_size() {
        let config = QueueConfig {
            retention: "24h".to_string(),
            max_size: Some("1_500".to_string()),
        };

        assert_eq!(config.retention_seconds().unwrap(), 86_400);
        assert_eq!(config.max_event_count().unwrap(), Some(1_500));
    }

    #[test]
    fn dedup_config_builds_key_from_metadata_and_payload() {
        let config = DedupConfig {
            window: "10m".to_string(),
            key: vec![
                "source".to_string(),
                "type".to_string(),
                "event_id".to_string(),
                "ref".to_string(),
                "actor".to_string(),
            ],
            strategy: "keep_first".to_string(),
        };
        let payload = serde_json::json!({
            "ref": "refs/heads/main",
            "sender": {"login": "octocat"}
        });

        assert_eq!(config.window_seconds().unwrap(), 600);
        assert_eq!(
            config
                .key_for("github", "com.github.push", "evt-1", &payload)
                .unwrap(),
            "source=github|type=com.github.push|event_id=evt-1|ref=refs/heads/main|actor=octocat"
        );
    }
}
