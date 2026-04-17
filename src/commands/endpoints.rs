use anyhow::Result;

use crate::commands::rpc;
use crate::commands::rpc::ApiPermission;

pub async fn list() -> Result<()> {
    let payload = rpc::call("hooks.list", serde_json::json!({})).await?;
    let config = crate::config::KiteConfig::load()?;
    let base = config.http_base();
    let team_id = payload
        .get("team_id")
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    println!("Team: {team_id}");
    let hooks = payload
        .get("hooks")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if hooks.is_empty() {
        println!("No endpoints configured.");
        return Ok(());
    }
    for hook in hooks {
        let id = hook.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        let source = hook.get("source").and_then(|v| v.as_str()).unwrap_or("-");
        let active = hook
            .get("active")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let webhook_secret_configured = hook
            .get("webhook_secret_configured")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let endpoint_url = format!("{base}/hooks/{team_id}/{source}");
        println!(
            "- id={id} source={source} active={active} url={endpoint_url} webhook_secret_configured={webhook_secret_configured}"
        );
    }
    Ok(())
}

pub async fn create(
    source: String,
    repo: Option<String>,
    events: Option<Vec<String>>,
    force: bool,
    github_token: Option<String>,
) -> Result<()> {
    rpc::ensure_permission(ApiPermission::Write, "kite endpoints create").await?;
    let payload = rpc::call("hooks.create", serde_json::json!({ "source": source })).await?;
    let endpoint = payload
        .get("endpoint")
        .and_then(|v| v.as_str())
        .unwrap_or("-")
        .to_string();
    let hook_token = payload
        .get("hook_token")
        .and_then(|v| v.as_str())
        .unwrap_or("-")
        .to_string();
    let endpoint_with_token = payload
        .get("endpoint_with_token")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            if endpoint != "-" && hook_token != "-" {
                Some(format!("{endpoint}/{hook_token}"))
            } else {
                None
            }
        });
    let github_webhook_secret = payload
        .get("github_webhook_secret")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let id = payload
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("-")
        .to_string();
    let config = crate::config::KiteConfig::load()?;
    let base = config.http_base();

    let full_url = endpoint_with_token
        .as_ref()
        .map(|p| format!("{base}{p}"))
        .unwrap_or_else(|| format!("{base}{endpoint}"));

    // If --repo is provided, auto-register on GitHub
    if let Some(ref repo_name) = repo {
        let secret = github_webhook_secret.as_deref().unwrap_or("");
        if secret.is_empty() {
            eprintln!("Warning: no webhook secret returned (source may not be 'github').");
            eprintln!("Falling back to manual setup.");
        } else {
            let event_list = events
                .unwrap_or_else(|| vec!["push".into(), "pull_request".into(), "issues".into()]);

            let event_refs: Vec<&str> = event_list.iter().map(String::as_str).collect();

            match crate::github::register_webhook(
                repo_name,
                &full_url,
                secret,
                &event_refs,
                github_token.as_deref(),
                force,
            )
            .await
            {
                Ok(()) => {
                    println!("Webhook configured on {repo_name}");
                    println!("- endpoint_id: {id}");
                    println!("- webhook_url: {full_url}");
                    println!("- events: {}", event_list.join(", "));
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("Failed to auto-register webhook on GitHub: {e}");
                    eprintln!("Falling back to manual setup.\n");
                }
            }
        }
    }

    println!("Created endpoint:");
    println!("- id: {id}");
    println!("- webhook_url: {full_url}");
    println!("- hook_token (shown once): {hook_token}");

    if let Some(secret) = &github_webhook_secret {
        println!();
        println!("GitHub webhook secret (shown once): {secret}");
        println!("Copy this value now and paste it into GitHub webhook settings > Secret.");
        println!("Kite stores the secret server-side and will not show it again.");
    }

    Ok(())
}

pub async fn deactivate(id: String) -> Result<()> {
    rpc::ensure_permission(ApiPermission::Write, "kite endpoints deactivate").await?;
    let payload = rpc::call("hooks.deactivate", serde_json::json!({ "id": id })).await?;
    let hook_id = payload.get("id").and_then(|v| v.as_str()).unwrap_or("-");
    println!("Deactivated endpoint: {hook_id}");
    Ok(())
}
