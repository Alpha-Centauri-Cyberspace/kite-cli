use anyhow::{Result, bail};

use crate::config::KiteConfig;
use crate::ws_client;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiPermission {
    Read,
    Write,
    Admin,
}

impl ApiPermission {
    pub const fn as_str(self) -> &'static str {
        match self {
            ApiPermission::Read => "read",
            ApiPermission::Write => "write",
            ApiPermission::Admin => "admin",
        }
    }

    const fn rank(self) -> u8 {
        match self {
            ApiPermission::Read => 1,
            ApiPermission::Write => 2,
            ApiPermission::Admin => 3,
        }
    }
}

pub async fn call(method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
    let config = KiteConfig::load()?;
    let ws_url = config.ws_url();
    let (api_key, team_id) = config.require_auth()?;
    ws_client::rpc_request(&ws_url, &api_key, &team_id, method, params).await
}

pub async fn ensure_permission(required: ApiPermission, command_hint: &str) -> Result<()> {
    let stats = call("dashboard.stats", serde_json::json!({})).await?;
    let current_permissions = extract_current_key_permissions(&stats);

    // Backward compatibility with older servers that do not report key permissions yet.
    if current_permissions.is_empty() {
        return Ok(());
    }

    validate_current_permissions(required, command_hint, &current_permissions)
}

fn extract_current_key_permissions(payload: &serde_json::Value) -> Vec<String> {
    payload
        .get("current_key_permissions")
        .and_then(|v| v.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn permission_rank(raw: &str) -> Option<u8> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "read" => Some(ApiPermission::Read.rank()),
        "write" => Some(ApiPermission::Write.rank()),
        "admin" | "*" => Some(ApiPermission::Admin.rank()),
        _ => None,
    }
}

fn has_required_permission(permissions: &[String], required: ApiPermission) -> bool {
    let required_rank = required.rank();
    permissions
        .iter()
        .filter_map(|permission| permission_rank(permission))
        .any(|rank| rank >= required_rank)
}

fn validate_current_permissions(
    required: ApiPermission,
    command_hint: &str,
    current_permissions: &[String],
) -> Result<()> {
    if has_required_permission(current_permissions, required) {
        return Ok(());
    }

    bail!(insufficient_permission_message(
        required,
        command_hint,
        current_permissions
    ))
}

fn insufficient_permission_message(
    required: ApiPermission,
    command_hint: &str,
    current_permissions: &[String],
) -> String {
    format!(
        "`{command_hint}` requires `{required}` permission, but your current key has `{current}`.\nRun `kite login` to refresh your local CLI key with write/admin access, then retry.\nIf you intentionally use a custom key, set `KITE_API_KEY` to a key with `{required}` permission.\nTip: run `kite status` to inspect current key permissions.",
        required = required.as_str(),
        current = format_permissions(current_permissions),
    )
}

fn format_permissions(permissions: &[String]) -> String {
    if permissions.is_empty() {
        return "none".to_string();
    }

    permissions.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_ranking_matches_server_rules() {
        assert!(has_required_permission(
            &["read".to_string()],
            ApiPermission::Read
        ));
        assert!(!has_required_permission(
            &["read".to_string()],
            ApiPermission::Write
        ));
        assert!(has_required_permission(
            &["write".to_string()],
            ApiPermission::Read
        ));
        assert!(has_required_permission(
            &["admin".to_string()],
            ApiPermission::Write
        ));
        assert!(has_required_permission(
            &["*".to_string()],
            ApiPermission::Admin
        ));
    }

    #[test]
    fn extract_current_permissions_from_dashboard_stats_payload() {
        let payload = serde_json::json!({
            "current_key_permissions": ["read", "write"]
        });
        let permissions = extract_current_key_permissions(&payload);
        assert_eq!(permissions, vec!["read".to_string(), "write".to_string()]);
    }

    #[test]
    fn insufficient_permission_message_is_actionable() {
        let message = insufficient_permission_message(
            ApiPermission::Write,
            "kite endpoints create --source github",
            &["read".to_string()],
        );

        assert!(message.contains("kite endpoints create --source github"));
        assert!(message.contains("requires `write`"));
        assert!(message.contains("kite login"));
        assert!(message.contains("KITE_API_KEY"));
    }

    #[test]
    fn read_only_key_fails_fast_for_mutating_command() {
        let err = validate_current_permissions(
            ApiPermission::Write,
            "kite endpoints create",
            &["read".to_string()],
        )
        .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("kite endpoints create"));
        assert!(message.contains("requires `write`"));
        assert!(message.contains("kite login"));
    }
}
