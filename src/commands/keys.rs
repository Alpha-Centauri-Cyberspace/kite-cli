use anyhow::Result;

use crate::commands::rpc;
use crate::commands::rpc::ApiPermission;

pub async fn list() -> Result<()> {
    let payload = rpc::call("keys.list", serde_json::json!({})).await?;
    let team_id = payload
        .get("team_id")
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    println!("Team: {team_id}");
    let keys = payload
        .get("keys")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if keys.is_empty() {
        println!("No API keys configured.");
        return Ok(());
    }
    for key in keys {
        let id = key.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        let name = key.get("name").and_then(|v| v.as_str()).unwrap_or("-");
        let key_prefix = key
            .get("key_prefix")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let revoked = key.get("revoked").and_then(|v| v.as_bool()).unwrap_or(true);
        let created_at = key
            .get("created_at")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        println!(
            "- id={id} name={name} revoked={revoked} key_prefix={key_prefix} created_at={created_at}"
        );
    }
    Ok(())
}

pub async fn create(
    name: Option<String>,
    scopes: Vec<String>,
    permissions: Vec<String>,
    expires_at: Option<String>,
) -> Result<()> {
    rpc::ensure_permission(ApiPermission::Admin, "kite keys create").await?;
    let payload = rpc::call(
        "keys.create",
        serde_json::json!({
            "name": name,
            "scopes": if scopes.is_empty() { vec!["*".to_string()] } else { scopes },
            "permissions": if permissions.is_empty() { vec!["read".to_string()] } else { permissions },
            "expires_at": expires_at,
        }),
    )
    .await?;

    let id = payload.get("id").and_then(|v| v.as_str()).unwrap_or("-");
    let key_prefix = payload
        .get("key_prefix")
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    let api_key = payload
        .get("api_key")
        .and_then(|v| v.as_str())
        .unwrap_or("-");

    println!("Created API key:");
    println!("- id: {id}");
    println!("- key_prefix: {key_prefix}");
    println!("- api_key (shown once): {api_key}");
    Ok(())
}

pub async fn revoke(id: String) -> Result<()> {
    rpc::ensure_permission(ApiPermission::Admin, "kite keys revoke").await?;
    let payload = rpc::call("keys.revoke", serde_json::json!({ "id": id })).await?;
    let key_id = payload.get("id").and_then(|v| v.as_str()).unwrap_or("-");
    println!("Revoked API key: {key_id}");
    Ok(())
}
