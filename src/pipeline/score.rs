use crate::manifest::ScoringConfig;
use crate::queue::Importance;

/// Evaluate scoring rules and return the importance level for an event.
pub fn score_event(
    config: &ScoringConfig,
    source: &str,
    event_type: &str,
    payload: &serde_json::Value,
    changed_files: Option<&[String]>,
) -> Importance {
    for rule in &config.rules {
        if !rule.match_rule.matches(source, event_type, payload) {
            continue;
        }

        // If rule has paths, check against changed files from enrichment
        if !rule.paths.is_empty() {
            if let Some(files) = changed_files {
                let path_match = rule.paths.iter().any(|pattern| {
                    files
                        .iter()
                        .any(|file| glob_match::glob_match(pattern, file))
                });
                if !path_match {
                    continue;
                }
            } else {
                continue; // No files available, skip path-based rules
            }
        }

        if let Ok(imp) = Importance::from_str(&rule.importance) {
            return imp;
        }
    }

    Importance::from_str(&config.default_importance).unwrap_or(Importance::Normal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ScoringRule;
    use crate::pipeline::filter::FilterRule;
    use serde_json::json;

    fn make_config(rules: Vec<ScoringRule>) -> ScoringConfig {
        ScoringConfig {
            rules,
            dedup: None,
            default_importance: "normal".to_string(),
        }
    }

    #[test]
    fn test_default_importance() {
        let config = make_config(vec![]);
        assert_eq!(
            score_event(&config, "github", "com.github.push", &json!({}), None).as_str(),
            "normal"
        );
    }

    #[test]
    fn test_simple_rule_match() {
        let config = make_config(vec![ScoringRule {
            match_rule: FilterRule {
                source: Some("github".into()),
                event_type: Some("com.github.push".into()),
                actor: None,
                git_ref: None,
            },
            importance: "high".into(),
            paths: vec![],
            reason: None,
        }]);
        assert_eq!(
            score_event(&config, "github", "com.github.push", &json!({}), None).as_str(),
            "high"
        );
    }

    #[test]
    fn test_first_match_wins() {
        let config = make_config(vec![
            ScoringRule {
                match_rule: FilterRule {
                    source: Some("github".into()),
                    event_type: None,
                    actor: None,
                    git_ref: None,
                },
                importance: "critical".into(),
                paths: vec![],
                reason: None,
            },
            ScoringRule {
                match_rule: FilterRule {
                    source: Some("github".into()),
                    event_type: None,
                    actor: None,
                    git_ref: None,
                },
                importance: "low".into(),
                paths: vec![],
                reason: None,
            },
        ]);
        assert_eq!(
            score_event(&config, "github", "com.github.push", &json!({}), None).as_str(),
            "critical"
        );
    }

    #[test]
    fn test_path_based_rule() {
        let config = make_config(vec![ScoringRule {
            match_rule: FilterRule {
                source: Some("github".into()),
                event_type: None,
                actor: None,
                git_ref: None,
            },
            importance: "critical".into(),
            paths: vec!["src/auth/*".into()],
            reason: Some("touches auth".into()),
        }]);

        let files = vec!["src/auth/login.rs".to_string()];
        assert_eq!(
            score_event(
                &config,
                "github",
                "com.github.push",
                &json!({}),
                Some(&files)
            )
            .as_str(),
            "critical"
        );

        let files = vec!["src/utils/helpers.rs".to_string()];
        assert_eq!(
            score_event(
                &config,
                "github",
                "com.github.push",
                &json!({}),
                Some(&files)
            )
            .as_str(),
            "normal"
        );

        assert_eq!(
            score_event(&config, "github", "com.github.push", &json!({}), None).as_str(),
            "normal"
        );
    }

    #[test]
    fn test_bot_actor_low_importance() {
        // Note: glob_match treats [bot] as character class. Use escaped form.
        let config = make_config(vec![ScoringRule {
            match_rule: FilterRule {
                source: None,
                event_type: None,
                actor: Some("*\\[bot\\]".into()),
                git_ref: None,
            },
            importance: "low".into(),
            paths: vec![],
            reason: None,
        }]);
        let payload = json!({"sender": {"login": "dependabot[bot]"}});
        assert_eq!(
            score_event(&config, "github", "com.github.push", &payload, None).as_str(),
            "low"
        );
    }
}
