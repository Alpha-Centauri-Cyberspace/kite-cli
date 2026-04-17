use anyhow::Result;

use crate::commands::rpc;
use crate::config::KiteConfig;

pub async fn run() -> Result<()> {
    let config = KiteConfig::load()?;
    let ws_url = config.ws_url();
    let payload = rpc::call("dashboard.stats", serde_json::json!({})).await?;

    let team_id = payload
        .get("team_id")
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    let webhooks_today = payload
        .get("webhooks_today")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let active_endpoints = payload
        .get("active_endpoints")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let active_api_keys = payload
        .get("active_api_keys")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let current_key_permissions = payload
        .get("current_key_permissions")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .collect::<Vec<_>>()
                .join(",")
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "-".to_string());
    let plan_name = payload
        .get("plan_name")
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    let active_sources_limit = payload
        .get("active_sources_limit")
        .and_then(|v| v.as_i64())
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());

    println!("Kite status");
    println!("- team_id: {team_id}");
    println!("- ws_url: {ws_url}");
    println!("- plan: {plan_name}");
    println!("- webhooks_today: {webhooks_today}");
    println!("- active_endpoints: {active_endpoints}");
    println!("- active_sources_limit: {active_sources_limit}");
    println!("- active_api_keys: {active_api_keys}");
    println!("- current_key_permissions: {current_key_permissions}");
    Ok(())
}
