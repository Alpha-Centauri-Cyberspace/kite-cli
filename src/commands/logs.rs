use anyhow::Result;

use crate::commands::rpc;

pub async fn recent(limit: u32) -> Result<()> {
    let payload = rpc::call("events.recent", serde_json::json!({ "limit": limit })).await?;
    let events = payload
        .get("events")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if events.is_empty() {
        println!("No events found.");
        return Ok(());
    }

    for row in events {
        let seq = row.get("seq").and_then(|v| v.as_i64()).unwrap_or(0);
        let created_at = row
            .get("created_at")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let event = row
            .get("event")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let event_type = event
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let summary = event
            .get("kitesummary")
            .and_then(|v| v.as_str())
            .unwrap_or("No summary");
        println!("#{seq} [{created_at}] {event_type} - {summary}");
    }

    Ok(())
}
