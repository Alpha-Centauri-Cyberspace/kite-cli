//! Orchestration platform abstraction.
//!
//! Each platform (OpenClaw, Claude Code, Agents, …) implements
//! [`OrchestrationPlatform`] so the CLI can install, export, and discover
//! skills without hard-coding any single runtime.

use anyhow::Result;
use directories::BaseDirs;
use std::path::PathBuf;

// ── Trait ────────────────────────────────────────────────────────────────────

/// A pluggable orchestration runtime that Kite can export skills to and
/// install skills from.
pub trait OrchestrationPlatform {
    /// Short machine-readable key used in CLI flags (e.g. `"openclaw"`).
    fn key(&self) -> &str;

    /// Human-readable display name (e.g. `"OpenClaw"`).
    fn display_name(&self) -> &str;

    /// The binary name used to invoke this platform's CLI installer.
    fn installer_bin(&self) -> &str;

    /// Default directory where skills are stored.
    fn default_skills_dir(&self) -> PathBuf;

    /// Whether this platform stores skills relative to the user's home
    /// directory (true) or relative to the working directory (false).
    fn is_home_relative(&self) -> bool;

    /// Environment variable that overrides the installer binary.
    fn installer_bin_env(&self) -> Option<&str> {
        None
    }

    /// Environment variable that overrides the skills directory.
    fn skills_dir_env(&self) -> Option<&str> {
        None
    }
}

// ── Built-in platforms ───────────────────────────────────────────────────────

pub struct OpenClaw;
pub struct ClaudeCode;
pub struct AgentsFramework;

impl OrchestrationPlatform for OpenClaw {
    fn key(&self) -> &str {
        "openclaw"
    }
    fn display_name(&self) -> &str {
        "OpenClaw"
    }
    fn installer_bin(&self) -> &str {
        "openclaw"
    }
    fn default_skills_dir(&self) -> PathBuf {
        home_dir().join(".openclaw/skills")
    }
    fn is_home_relative(&self) -> bool {
        true
    }
    fn installer_bin_env(&self) -> Option<&str> {
        Some("KITE_SKILL_INSTALLER_BIN")
    }
    fn skills_dir_env(&self) -> Option<&str> {
        Some("KITE_SKILLS_DIR")
    }
}

impl OrchestrationPlatform for ClaudeCode {
    fn key(&self) -> &str {
        "claude"
    }
    fn display_name(&self) -> &str {
        "Claude Code"
    }
    fn installer_bin(&self) -> &str {
        "claude"
    }
    fn default_skills_dir(&self) -> PathBuf {
        PathBuf::from(".claude/skills")
    }
    fn is_home_relative(&self) -> bool {
        false
    }
}

impl OrchestrationPlatform for AgentsFramework {
    fn key(&self) -> &str {
        "agents"
    }
    fn display_name(&self) -> &str {
        "Agents"
    }
    fn installer_bin(&self) -> &str {
        "agents"
    }
    fn default_skills_dir(&self) -> PathBuf {
        PathBuf::from(".agents/skills")
    }
    fn is_home_relative(&self) -> bool {
        false
    }
}

// ── Registry ─────────────────────────────────────────────────────────────────

/// Returns all built-in platforms in discovery order.
pub fn all_platforms() -> Vec<Box<dyn OrchestrationPlatform>> {
    vec![
        Box::new(ClaudeCode),
        Box::new(AgentsFramework),
        Box::new(OpenClaw),
    ]
}

/// Look up a platform by its CLI key.
pub fn platform_by_key(key: &str) -> Result<Box<dyn OrchestrationPlatform>> {
    all_platforms()
        .into_iter()
        .find(|p| p.key() == key)
        .ok_or_else(|| {
            let valid: Vec<_> = all_platforms()
                .iter()
                .map(|p| p.key().to_string())
                .collect();
            anyhow::anyhow!("unknown platform `{key}`. Supported: {}", valid.join(", "))
        })
}

/// Resolve the effective skills directory for a platform, checking env overrides.
pub fn resolve_skills_dir(platform: &dyn OrchestrationPlatform) -> PathBuf {
    if let Some(env_key) = platform.skills_dir_env()
        && let Ok(val) = std::env::var(env_key)
        && !val.trim().is_empty()
    {
        return PathBuf::from(val);
    }
    platform.default_skills_dir()
}

/// Resolve the effective installer binary for a platform, checking env overrides.
pub fn resolve_installer_bin(platform: &dyn OrchestrationPlatform) -> String {
    if let Some(env_key) = platform.installer_bin_env()
        && let Ok(val) = std::env::var(env_key)
        && !val.trim().is_empty()
    {
        return val;
    }
    platform.installer_bin().to_string()
}

fn home_dir() -> PathBuf {
    BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("~"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_platforms_omit_obsolete_paperclip_runtime() {
        let keys = all_platforms()
            .into_iter()
            .map(|platform| platform.key().to_string())
            .collect::<Vec<_>>();

        assert!(!keys.iter().any(|key| key == "paperclip"));
    }

    #[test]
    fn platform_lookup_rejects_obsolete_paperclip_runtime() {
        let error = match platform_by_key("paperclip") {
            Ok(_) => panic!("paperclip should not be supported"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("unknown platform"));
    }
}
