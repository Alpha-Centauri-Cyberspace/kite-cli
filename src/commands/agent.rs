use anyhow::{Result, anyhow};
use cloudevents::AttributesReader;
use serde_json::json;

use crate::config::{AgentConfig, KiteConfig};
use crate::ws_client::{self, AckDecision};
use kite_protocol::agent_message::{AgentMessage, EVENT_TYPE as AGENT_MESSAGE_EVENT_TYPE};

/// `kite agent register` — persist a stable agent id locally.
///
/// If `name` is provided it becomes the agent id (slugged), otherwise a fresh
/// UUIDv7 is generated. The id is written to the user's Kite config so later
/// `kite agent listen` and `kite agent send` invocations can default to it.
pub fn register(name: Option<String>) -> Result<()> {
    let mut config = KiteConfig::load()?;

    let id = match &name {
        Some(n) => slugify(n),
        None => uuid::Uuid::now_v7().to_string(),
    };

    if id.is_empty() {
        anyhow::bail!("Agent name must contain at least one alphanumeric character");
    }

    config.agent = Some(AgentConfig {
        id: id.clone(),
        name: name.clone(),
    });
    config.save()?;

    println!("Registered agent");
    if let Some(n) = name {
        println!("  name: {n}");
    }
    println!("  id:   {id}");
    println!();
    println!("Other agents on your team can address this agent with:");
    println!("  kite agent send --to {id} \"<message>\"");
    Ok(())
}

/// `kite agent listen` — subscribe to `com.kite.agent.message` events whose
/// `data.to` matches the local agent id (or `--as <id>` override).
pub async fn listen(as_id: Option<String>, json_mode: bool) -> Result<()> {
    let config = KiteConfig::load()?;
    let (api_key, team_id) = config.require_auth()?;
    let ws_url = config.ws_url();

    let agent_id = resolve_agent_id(&config, as_id)?;
    let scope = format!("agent_to:{agent_id}");

    eprintln!("Listening as agent `{agent_id}` (team {team_id})");
    eprintln!("Connecting to {ws_url}...");

    let mut backoff = 1u64;
    let max_backoff = 30u64;

    loop {
        match ws_client::connect(&ws_url, &api_key, &team_id, vec![scope.clone()], None).await {
            Ok((sink_ws, stream, last_seq, _client_id)) => {
                backoff = 1;
                eprintln!("Connected (last_seq: {last_seq})");

                let agent_id = agent_id.clone();
                let result = ws_client::event_loop_with_ack(sink_ws, stream, move |_seq, event| {
                    let agent_id = agent_id.clone();
                    async move {
                        if event.ty() != AGENT_MESSAGE_EVENT_TYPE {
                            return Ok(AckDecision::Ack);
                        }
                        let Some(msg) = AgentMessage::from_event(&event) else {
                            return Ok(AckDecision::Ack);
                        };
                        if msg.to != agent_id {
                            return Ok(AckDecision::Ack);
                        }

                        if json_mode {
                            let line = serde_json::to_string(&event)?;
                            println!("{line}");
                        } else {
                            print_message(&msg, event.id());
                        }
                        Ok(AckDecision::Ack)
                    }
                })
                .await;

                if let Err(e) = result {
                    eprintln!("Disconnected: {e}");
                }
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("Server error (4001)") || err_str.contains("Invalid token") {
                    eprintln!("Connection failed: {e}");
                    return Err(e);
                }
                eprintln!("Connection failed: {e}");
            }
        }

        eprintln!("Reconnecting in {backoff}s...");
        tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

/// `kite agent send` — POST a `com.kite.agent.message` CloudEvent to the
/// hosted Kite hook ingress addressed to `to`.
pub async fn send(
    to: String,
    body: String,
    from: Option<String>,
    thread: Option<String>,
    reply_to: Option<String>,
) -> Result<()> {
    let config = KiteConfig::load()?;
    let (api_key, team_id) = config.require_auth()?;
    let hook_base = config.hook_base_url()?;

    let from_id = match from {
        Some(f) => f,
        None => resolve_agent_id(&config, None).map_err(|_| {
            anyhow!(
                "No --from agent id specified and no local agent registered. Run `kite agent register` first or pass --from <id>."
            )
        })?,
    };

    let mut payload = json!({
        "from": from_id,
        "to": to,
        "body": body,
    });
    if let Some(t) = thread.as_ref() {
        payload["thread_id"] = json!(t);
    }
    if let Some(r) = reply_to.as_ref() {
        payload["reply_to_id"] = json!(r);
    }

    let url = format!("{hook_base}/api/v1/agents/messages?team_id={team_id}");
    let response = reqwest::Client::new()
        .post(&url)
        .bearer_auth(&api_key)
        .json(&payload)
        .send()
        .await?;

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("send failed ({status}): {text}");
    }

    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    let event_id = parsed
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    println!("Sent agent message");
    println!("  from: {from_id}");
    println!("  to:   {to}");
    if !event_id.is_empty() {
        println!("  id:   {event_id}");
    }
    Ok(())
}

fn resolve_agent_id(config: &KiteConfig, override_id: Option<String>) -> Result<String> {
    if let Some(id) = override_id {
        return Ok(id);
    }
    config
        .agent
        .as_ref()
        .map(|a| a.id.clone())
        .ok_or_else(|| anyhow!("No agent registered. Run `kite agent register` first."))
}

fn print_message(msg: &AgentMessage, event_id: &str) {
    let thread = msg
        .thread_id
        .as_deref()
        .map(|t| format!(" thread={t}"))
        .unwrap_or_default();
    let reply = msg
        .reply_to_id
        .as_deref()
        .map(|r| format!(" reply_to={r}"))
        .unwrap_or_default();
    println!(
        "[{event_id}] {from} → {to}{thread}{reply}: {body}",
        from = msg.from,
        to = msg.to,
        body = msg.body
    );
}

fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_dash = false;
    for ch in input.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Agent Alpha"), "agent-alpha");
        assert_eq!(slugify("  My_Agent_42  "), "my-agent-42");
        assert_eq!(slugify("agent--with___gaps"), "agent-with-gaps");
    }

    #[test]
    fn slugify_empty() {
        assert_eq!(slugify("!!!"), "");
        assert_eq!(slugify(""), "");
    }
}
