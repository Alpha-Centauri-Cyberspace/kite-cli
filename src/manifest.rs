use anyhow::Result;
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
    Paperclip {
        /// Paperclip API base URL.
        api_url: String,
        /// Paperclip company ID.
        company_id: String,
        /// Optional agent ID for targeted heartbeat triggers.
        #[serde(default)]
        agent_id: Option<String>,
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
}
