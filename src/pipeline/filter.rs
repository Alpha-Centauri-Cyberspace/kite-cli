use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FilterConfig {
    #[serde(default)]
    pub drop: Vec<FilterRule>,
    #[serde(default)]
    pub keep_only: Vec<FilterRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterRule {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(rename = "type", default)]
    pub event_type: Option<String>,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(rename = "ref", default)]
    pub git_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FilterResult {
    Pass,
    Dropped,
}

impl FilterConfig {
    /// Evaluate filter rules against an incoming event.
    ///
    /// - Drop rules are evaluated first; if any match, the event is dropped.
    /// - keep_only rules are evaluated next; if any are defined and none match, the event is dropped.
    /// - Otherwise the event passes.
    pub fn evaluate(&self, source: &str, event_type: &str, payload: &Value) -> FilterResult {
        // Evaluate drop rules first
        for rule in &self.drop {
            if rule.matches(source, event_type, payload) {
                return FilterResult::Dropped;
            }
        }

        // Evaluate keep_only rules — if any are defined and none match, drop
        if !self.keep_only.is_empty() {
            let any_match = self
                .keep_only
                .iter()
                .any(|rule| rule.matches(source, event_type, payload));
            if !any_match {
                return FilterResult::Dropped;
            }
        }

        FilterResult::Pass
    }
}

impl FilterRule {
    /// Returns true if this rule matches the given event.
    /// All specified fields must match (AND logic). Unspecified fields always match.
    pub fn matches(&self, source: &str, event_type: &str, payload: &Value) -> bool {
        if let Some(pattern) = &self.source
            && !glob_match::glob_match(pattern, source)
        {
            return false;
        }

        if let Some(pattern) = &self.event_type
            && !glob_match::glob_match(pattern, event_type)
        {
            return false;
        }

        if let Some(pattern) = &self.actor {
            let actor = payload
                .get("sender")
                .and_then(|s| s.get("login"))
                .and_then(|l| l.as_str())
                .unwrap_or("");
            if !glob_match::glob_match(pattern, actor) {
                return false;
            }
        }

        if let Some(pattern) = &self.git_ref {
            let git_ref = payload.get("ref").and_then(|r| r.as_str()).unwrap_or("");
            if !glob_match::glob_match(pattern, git_ref) {
                return false;
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_empty_filter_passes_all() {
        let config = FilterConfig::default();
        let payload = json!({});
        assert_eq!(
            config.evaluate("com.github", "push", &payload),
            FilterResult::Pass
        );
    }

    #[test]
    fn test_drop_rule_exact_match() {
        let config = FilterConfig {
            drop: vec![FilterRule {
                source: Some("com.github".to_string()),
                event_type: Some("push".to_string()),
                actor: None,
                git_ref: None,
            }],
            keep_only: vec![],
        };
        let payload = json!({});
        assert_eq!(
            config.evaluate("com.github", "push", &payload),
            FilterResult::Dropped
        );
        // Different event type should pass
        assert_eq!(
            config.evaluate("com.github", "pull_request", &payload),
            FilterResult::Pass
        );
    }

    #[test]
    fn test_drop_rule_glob_match() {
        let config = FilterConfig {
            drop: vec![FilterRule {
                source: None,
                event_type: Some("com.github.pull_request.*".to_string()),
                actor: None,
                git_ref: None,
            }],
            keep_only: vec![],
        };
        let payload = json!({});
        assert_eq!(
            config.evaluate("any.source", "com.github.pull_request.opened", &payload),
            FilterResult::Dropped
        );
        assert_eq!(
            config.evaluate("any.source", "com.github.pull_request.closed", &payload),
            FilterResult::Dropped
        );
        assert_eq!(
            config.evaluate("any.source", "com.github.push", &payload),
            FilterResult::Pass
        );
    }

    #[test]
    fn test_drop_rule_actor_match() {
        let config = FilterConfig {
            drop: vec![FilterRule {
                source: None,
                event_type: None,
                // GitHub bot names contain literal brackets; escape them for glob matching
                actor: Some("dependabot\\[bot\\]".to_string()),
                git_ref: None,
            }],
            keep_only: vec![],
        };
        let dependabot_payload = json!({ "sender": { "login": "dependabot[bot]" } });
        let human_payload = json!({ "sender": { "login": "octocat" } });

        assert_eq!(
            config.evaluate("com.github", "push", &dependabot_payload),
            FilterResult::Dropped
        );
        assert_eq!(
            config.evaluate("com.github", "push", &human_payload),
            FilterResult::Pass
        );
    }

    #[test]
    fn test_drop_rule_actor_glob() {
        let config = FilterConfig {
            drop: vec![FilterRule {
                source: None,
                event_type: None,
                // Match any GitHub-style bot: *\[bot\] matches "dependabot[bot]", "renovate[bot]", etc.
                actor: Some("*\\[bot\\]".to_string()),
                git_ref: None,
            }],
            keep_only: vec![],
        };
        let bot_payload = json!({ "sender": { "login": "some-bot[bot]" } });
        let human_payload = json!({ "sender": { "login": "octocat" } });

        assert_eq!(
            config.evaluate("com.github", "push", &bot_payload),
            FilterResult::Dropped
        );
        assert_eq!(
            config.evaluate("com.github", "push", &human_payload),
            FilterResult::Pass
        );
    }

    #[test]
    fn test_keep_only_whitelist() {
        let config = FilterConfig {
            drop: vec![],
            keep_only: vec![FilterRule {
                source: Some("com.github".to_string()),
                event_type: Some("push".to_string()),
                actor: None,
                git_ref: None,
            }],
        };
        let payload = json!({});
        // Matching event passes
        assert_eq!(
            config.evaluate("com.github", "push", &payload),
            FilterResult::Pass
        );
        // Non-matching event is dropped
        assert_eq!(
            config.evaluate("com.github", "pull_request", &payload),
            FilterResult::Dropped
        );
        // Different source is dropped
        assert_eq!(
            config.evaluate("com.gitlab", "push", &payload),
            FilterResult::Dropped
        );
    }

    #[test]
    fn test_drop_takes_priority_over_keep() {
        let config = FilterConfig {
            drop: vec![FilterRule {
                source: Some("com.github".to_string()),
                event_type: None,
                actor: None,
                git_ref: None,
            }],
            keep_only: vec![FilterRule {
                source: Some("com.github".to_string()),
                event_type: Some("push".to_string()),
                actor: None,
                git_ref: None,
            }],
        };
        let payload = json!({});
        // Even though keep_only matches, drop rule takes priority
        assert_eq!(
            config.evaluate("com.github", "push", &payload),
            FilterResult::Dropped
        );
    }

    #[test]
    fn test_ref_filter() {
        let config = FilterConfig {
            drop: vec![FilterRule {
                source: None,
                event_type: None,
                actor: None,
                git_ref: Some("refs/heads/main".to_string()),
            }],
            keep_only: vec![],
        };
        let main_payload = json!({ "ref": "refs/heads/main" });
        let feature_payload = json!({ "ref": "refs/heads/feature-branch" });

        assert_eq!(
            config.evaluate("com.github", "push", &main_payload),
            FilterResult::Dropped
        );
        assert_eq!(
            config.evaluate("com.github", "push", &feature_payload),
            FilterResult::Pass
        );
    }
}
