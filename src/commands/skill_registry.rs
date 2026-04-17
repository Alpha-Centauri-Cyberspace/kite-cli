use anyhow::{Result, bail};
use serde_json::json;

use crate::config::KiteConfig;

/// Derive the HTTP API base URL from the WebSocket URL.
///
/// ws://host:port/ws  -> http://host:port
/// wss://host:port/ws -> https://host:port
fn api_base_url(config: &KiteConfig) -> Result<String> {
    // Allow explicit override
    if let Ok(url) = std::env::var("KITE_API_URL") {
        return Ok(url.trim_end_matches('/').to_string());
    }

    let ws_url = config.ws_url();
    let base = ws_url
        .replace("wss://", "https://")
        .replace("ws://", "http://");
    // Strip /ws path suffix if present
    let base = base.trim_end_matches("/ws").to_string();
    Ok(base.trim_end_matches('/').to_string())
}

fn http_client() -> reqwest::Client {
    reqwest::Client::new()
}

/// Publish a skill to the registry (creates the skill if needed, then publishes a version).
pub async fn publish(
    name: String,
    version: String,
    description: Option<String>,
    content: Option<String>,
    public: bool,
) -> Result<()> {
    let config = KiteConfig::load()?;
    let (api_key, team_id) = config.require_auth()?;
    let base = api_base_url(&config)?;
    let client = http_client();

    // Read content from SKILL.md in current directory if not provided
    let content = match content {
        Some(c) => c,
        None => {
            let skill_md = std::path::Path::new("SKILL.md");
            if skill_md.exists() {
                std::fs::read_to_string(skill_md)?
            } else {
                String::new()
            }
        }
    };

    // Try to create the skill (ignore duplicate errors)
    let desc = description.as_deref().unwrap_or("");
    let create_resp = client
        .post(format!("{base}/api/v1/skills?team_id={team_id}"))
        .bearer_auth(&api_key)
        .json(&json!({
            "name": name,
            "description": desc,
            "public": public,
        }))
        .send()
        .await?;

    let skill_id = if create_resp.status().is_success() {
        let body: serde_json::Value = create_resp.json().await?;
        println!("Created skill `{name}`.");
        body["id"].as_str().unwrap_or_default().to_string()
    } else {
        // Skill might already exist — fetch it
        let list_resp = client
            .get(format!("{base}/api/v1/skills?team_id={team_id}"))
            .bearer_auth(&api_key)
            .send()
            .await?;

        if !list_resp.status().is_success() {
            let status = list_resp.status();
            let body = list_resp.text().await.unwrap_or_default();
            bail!("Failed to list skills (HTTP {status}): {body}");
        }

        let body: serde_json::Value = list_resp.json().await?;
        let skills = body["skills"].as_array().cloned().unwrap_or_default();
        let found = skills.iter().find(|s| s["name"].as_str() == Some(&name));
        match found {
            Some(s) => {
                println!("Skill `{name}` already exists, publishing new version.");
                s["id"].as_str().unwrap_or_default().to_string()
            }
            None => bail!("Failed to create or find skill `{name}`"),
        }
    };

    // Publish the version
    let ver_resp = client
        .post(format!(
            "{base}/api/v1/skills/{skill_id}/versions?team_id={team_id}"
        ))
        .bearer_auth(&api_key)
        .json(&json!({
            "version": version,
            "content": content,
            "metadata": {},
        }))
        .send()
        .await?;

    if !ver_resp.status().is_success() {
        let status = ver_resp.status();
        let body = ver_resp.text().await.unwrap_or_default();
        bail!("Failed to publish version {version} (HTTP {status}): {body}");
    }

    let ver_body: serde_json::Value = ver_resp.json().await?;
    let ver_id = ver_body["id"].as_str().unwrap_or("-");
    println!("Published {name}@{version} (version_id={ver_id})");

    Ok(())
}

/// Search the skill registry.
pub async fn search(query: Option<String>) -> Result<()> {
    let config = KiteConfig::load()?;
    let (api_key, team_id) = config.require_auth()?;
    let base = api_base_url(&config)?;
    let client = http_client();

    let mut url = format!("{base}/api/v1/skills/registry/search?team_id={team_id}&limit=50");
    if let Some(ref q) = query {
        // Simple percent-encode for the query parameter
        let encoded: String = q
            .chars()
            .map(|c| match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
                _ => format!("%{:02X}", c as u32),
            })
            .collect();
        url.push_str(&format!("&q={encoded}"));
    }

    let resp = client.get(&url).bearer_auth(&api_key).send().await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("Search failed (HTTP {status}): {body}");
    }

    let body: serde_json::Value = resp.json().await?;
    let skills = body["skills"].as_array().cloned().unwrap_or_default();

    if skills.is_empty() {
        println!("No skills found.");
        return Ok(());
    }

    println!("Skills ({}):", skills.len());
    for s in &skills {
        let name = s["name"].as_str().unwrap_or("-");
        let desc = s["description"].as_str().unwrap_or("");
        let public = s["public"].as_bool().unwrap_or(false);
        let visibility = if public { "public" } else { "private" };
        let id = s["id"].as_str().unwrap_or("-");
        if desc.is_empty() {
            println!("  {name} [{visibility}] (id={id})");
        } else {
            println!("  {name} [{visibility}] — {desc} (id={id})");
        }
    }

    Ok(())
}

/// Derive the HTTP API base URL from the WebSocket URL (test-visible wrapper).
#[cfg(test)]
fn api_base_url_from_ws(ws_url: &str) -> String {
    let base = ws_url
        .replace("wss://", "https://")
        .replace("ws://", "http://");
    let base = base.trim_end_matches("/ws").to_string();
    base.trim_end_matches('/').to_string()
}

/// Install a skill from the registry to the current team.
pub async fn install_from_registry(skill_id: String, version_id: Option<String>) -> Result<()> {
    let config = KiteConfig::load()?;
    let (api_key, team_id) = config.require_auth()?;
    let base = api_base_url(&config)?;
    let client = http_client();

    let mut payload = json!({ "skill_id": skill_id });
    if let Some(vid) = &version_id {
        payload["version_id"] = json!(vid);
    }

    let resp = client
        .post(format!("{base}/api/v1/skills/installed?team_id={team_id}"))
        .bearer_auth(&api_key)
        .json(&payload)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("Install failed (HTTP {status}): {body}");
    }

    let body: serde_json::Value = resp.json().await?;
    let name = body["skill_name"].as_str().unwrap_or("-");
    let version = body["version"].as_str().unwrap_or("-");
    println!("Installed {name}@{version} from registry.");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_base_from_ws_local() {
        assert_eq!(
            api_base_url_from_ws("ws://localhost:7700/ws"),
            "http://localhost:7700"
        );
    }

    #[test]
    fn api_base_from_wss_production() {
        assert_eq!(
            api_base_url_from_ws("wss://api.getkite.sh/ws"),
            "https://api.getkite.sh"
        );
    }

    #[test]
    fn api_base_strips_trailing_slash() {
        assert_eq!(
            api_base_url_from_ws("ws://localhost:7700/"),
            "http://localhost:7700"
        );
    }

    #[test]
    fn api_base_no_ws_suffix() {
        assert_eq!(
            api_base_url_from_ws("ws://localhost:7700"),
            "http://localhost:7700"
        );
    }

    #[test]
    fn api_base_from_wss_without_path() {
        assert_eq!(
            api_base_url_from_ws("wss://api.getkite.sh"),
            "https://api.getkite.sh"
        );
    }
}
