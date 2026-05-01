#![allow(dead_code)]

use anyhow::{Result, anyhow};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KiteConfig {
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Stable identifier the local agent uses as its CloudEvent `from` and as
    /// the scope for incoming `agent_to:<id>` subscriptions.
    pub id: String,
    /// Optional human-readable label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthConfig {
    pub api_key: Option<String>,
    pub team_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_url")]
    pub url: String,
    #[serde(default)]
    pub http_base: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            url: default_url(),
            http_base: None,
        }
    }
}

fn default_url() -> String {
    "ws://localhost:7700/ws".to_string()
}

/// Derive an HTTP base URL from a WebSocket URL.
/// Replaces "wss://" with "https://" and "ws://" with "http://", then trims a trailing "/ws".
pub fn derive_http_base(ws_url: &str) -> String {
    let http = if let Some(rest) = ws_url.strip_prefix("wss://") {
        format!("https://{}", rest)
    } else if let Some(rest) = ws_url.strip_prefix("ws://") {
        format!("http://{}", rest)
    } else {
        ws_url.to_string()
    };
    http.strip_suffix("/ws").unwrap_or(&http).to_string()
}

impl KiteConfig {
    /// Load config from ~/.config/kite/config.toml
    pub fn load() -> Result<Self> {
        let path = config_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let config: KiteConfig = toml::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// Save config to ~/.config/kite/config.toml
    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Get the HTTP base URL (e.g., https://api.getkite.sh).
    /// Uses server.http_base if set, otherwise derives it from ws_url().
    pub fn http_base(&self) -> String {
        if let Some(ref base) = self.server.http_base {
            return base.clone();
        }
        derive_http_base(&self.ws_url())
    }

    /// Get the WebSocket URL to connect to.
    pub fn ws_url(&self) -> String {
        // Allow override via env var
        std::env::var("KITE_WS_URL").unwrap_or_else(|_| self.server.url.clone())
    }

    /// Get the public HTTP(S) base URL for Kite hook ingress.
    pub fn hook_base_url(&self) -> Result<String> {
        if let Ok(url) = std::env::var("KITE_HOOK_BASE_URL") {
            let trimmed = url.trim().trim_end_matches('/').to_string();
            if !trimmed.is_empty() {
                return Ok(trimmed);
            }
        }

        derive_hook_base_url(&self.ws_url())
    }

    /// Get the API key for authentication.
    pub fn api_key(&self) -> Option<String> {
        // Allow override via env var
        std::env::var("KITE_API_KEY")
            .ok()
            .or_else(|| self.auth.api_key.clone())
    }

    /// Get the team ID for authentication scope.
    pub fn team_id(&self) -> Option<String> {
        std::env::var("KITE_TEAM_ID")
            .ok()
            .or_else(|| self.auth.team_id.clone())
    }

    /// Require both API key and team ID to be configured.
    pub fn require_auth(&self) -> Result<(String, String)> {
        let api_key = self
            .api_key()
            .ok_or_else(|| anyhow!("No API key configured. Run `kite login` first."))?;
        let team_id = self
            .team_id()
            .ok_or_else(|| anyhow!("No team ID configured. Run `kite login` first."))?;
        Ok((api_key, team_id))
    }
}

/// Get the config file path.
pub fn config_path() -> PathBuf {
    if let Some(proj_dirs) = ProjectDirs::from("dev", "kite", "kite") {
        proj_dirs.config_dir().join("config.toml")
    } else {
        PathBuf::from(".kite/config.toml")
    }
}

/// Get the data directory path (for DLQ, logs, etc.)
pub fn data_dir() -> PathBuf {
    if let Some(proj_dirs) = ProjectDirs::from("dev", "kite", "kite") {
        proj_dirs.data_dir().to_path_buf()
    } else {
        PathBuf::from(".kite/data")
    }
}

/// Get the DLQ directory path.
pub fn dlq_dir() -> PathBuf {
    data_dir().join("dlq")
}

/// Get the queue database path.
pub fn queue_db_path() -> PathBuf {
    data_dir().join("queue.db")
}

fn derive_hook_base_url(ws_url: &str) -> Result<String> {
    let trimmed = ws_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(anyhow!("Kite server URL is empty. Run `kite login` first."));
    }

    let without_ws_suffix = trimmed
        .strip_suffix("/ws")
        .or_else(|| trimmed.strip_suffix("/ws/"))
        .unwrap_or(trimmed);

    if let Some(rest) = without_ws_suffix.strip_prefix("wss://") {
        return Ok(format!("https://{rest}"));
    }
    if let Some(rest) = without_ws_suffix.strip_prefix("ws://") {
        return Ok(format!("http://{rest}"));
    }
    if without_ws_suffix.starts_with("https://") || without_ws_suffix.starts_with("http://") {
        return Ok(without_ws_suffix.to_string());
    }

    Err(anyhow!(
        "Unsupported Kite server URL `{}`. Expected ws:// or wss://. Run `kite login` again or set KITE_HOOK_BASE_URL.",
        ws_url
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_http_base_wss_with_ws_suffix() {
        assert_eq!(
            derive_http_base("wss://api.getkite.sh/ws"),
            "https://api.getkite.sh"
        );
    }

    #[test]
    fn test_derive_http_base_ws_localhost() {
        assert_eq!(
            derive_http_base("ws://localhost:7700/ws"),
            "http://localhost:7700"
        );
    }

    #[test]
    fn test_derive_http_base_wss_no_ws_suffix() {
        assert_eq!(
            derive_http_base("wss://api.getkite.sh"),
            "https://api.getkite.sh"
        );
    }

    #[test]
    fn derives_https_base_from_wss_url() {
        let base = derive_hook_base_url("wss://api.getkite.sh/ws").expect("base url");
        assert_eq!(base, "https://api.getkite.sh");
    }

    #[test]
    fn derives_http_base_from_ws_url() {
        let base = derive_hook_base_url("ws://localhost:7700/ws").expect("base url");
        assert_eq!(base, "http://localhost:7700");
    }

    #[test]
    fn accepts_http_url_without_ws_suffix() {
        let base = derive_hook_base_url("https://api.getkite.sh").expect("base url");
        assert_eq!(base, "https://api.getkite.sh");
    }
}
