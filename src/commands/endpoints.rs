use std::io::{self, Read};

use anyhow::{Context, Result};

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
    signing_secret: Option<String>,
) -> Result<()> {
    rpc::ensure_permission(ApiPermission::Write, "kite endpoints create").await?;

    let resolved_secret = resolve_signing_secret(signing_secret.as_deref(), &mut io::stdin())?;

    let mut params = serde_json::Map::new();
    params.insert(
        "source".to_string(),
        serde_json::Value::String(source.clone()),
    );
    if let Some(secret) = resolved_secret.as_ref() {
        params.insert(
            "webhook_secret".to_string(),
            serde_json::Value::String(secret.clone()),
        );
    }
    let payload = rpc::call("hooks.create", serde_json::Value::Object(params)).await?;
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
    let webhook_secret_configured = payload
        .get("webhook_secret_configured")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let id = payload
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("-")
        .to_string();
    let config = crate::config::KiteConfig::load()?;
    let base = config.http_base();

    let provider_signature_configured =
        webhook_secret_configured || github_webhook_secret.is_some();
    let urls = endpoint_urls(
        &base,
        &endpoint,
        endpoint_with_token.as_deref(),
        provider_signature_configured,
    );
    let webhook_url = urls.primary_webhook_url.as_str();

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
                webhook_url,
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
                    println!("- webhook_url: {webhook_url}");
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
    println!("- webhook_url: {webhook_url}");
    if provider_signature_configured {
        println!(
            "- bearer_webhook_url (shown once): {}",
            urls.bearer_webhook_url
        );
    }
    println!("- hook_token (shown once): {hook_token}");
    if resolved_secret.is_some() {
        println!(
            "- signing_secret: {}",
            if webhook_secret_configured {
                "stored (signature verification enabled)"
            } else {
                "WARNING: server did not confirm storage"
            }
        );
    }

    if let Some(secret) = &github_webhook_secret {
        println!();
        println!("GitHub webhook secret (shown once): {secret}");
        println!("Copy this value now and paste it into GitHub webhook settings > Secret.");
        println!("Kite stores the secret server-side and will not show it again.");
    }

    Ok(())
}

fn resolve_signing_secret(
    signing_secret: Option<&str>,
    reader: &mut dyn Read,
) -> Result<Option<String>> {
    match signing_secret {
        Some("-") => {
            let mut buf = String::new();
            reader
                .read_to_string(&mut buf)
                .context("failed to read signing secret from stdin")?;
            let trimmed = buf.trim().to_string();
            if trimmed.is_empty() {
                anyhow::bail!("--signing-secret - was provided but stdin was empty");
            }
            Ok(Some(trimmed))
        }
        Some(other) => {
            let trimmed = other.trim().to_string();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed))
            }
        }
        None => Ok(None),
    }
}

pub async fn rotate_secret(source: String, signing_secret: Option<String>) -> Result<()> {
    rpc::ensure_permission(ApiPermission::Write, "kite endpoints rotate-secret").await?;

    let Some(signing_secret) = signing_secret else {
        anyhow::bail!("--signing-secret is required; use --signing-secret - to read from stdin");
    };
    let resolved_secret = resolve_signing_secret(Some(&signing_secret), &mut io::stdin())?
        .context("--signing-secret is required; use --signing-secret - to read from stdin")?;

    let payload = rpc::call(
        "hooks.rotate_secret",
        serde_json::json!({
            "source": source,
            "webhook_secret": resolved_secret,
        }),
    )
    .await?;
    let hook_id = payload.get("id").and_then(|v| v.as_str()).unwrap_or("-");
    let source = payload
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("-");

    println!("Updated signing secret:");
    println!("- id: {hook_id}");
    println!("- source: {source}");
    println!("- signing_secret: stored (signature verification enabled)");
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct EndpointUrls {
    primary_webhook_url: String,
    bearer_webhook_url: String,
}

fn endpoint_urls(
    base: &str,
    endpoint: &str,
    endpoint_with_token: Option<&str>,
    provider_signature_configured: bool,
) -> EndpointUrls {
    let provider_webhook_url = format!("{base}{endpoint}");
    let bearer_webhook_url = endpoint_with_token
        .map(|path| format!("{base}{path}"))
        .unwrap_or_else(|| provider_webhook_url.clone());
    let primary_webhook_url = if provider_signature_configured {
        provider_webhook_url
    } else {
        bearer_webhook_url.clone()
    };

    EndpointUrls {
        primary_webhook_url,
        bearer_webhook_url,
    }
}

pub async fn deactivate(id: String) -> Result<()> {
    rpc::ensure_permission(ApiPermission::Write, "kite endpoints deactivate").await?;
    let payload = rpc::call("hooks.deactivate", serde_json::json!({ "id": id })).await?;
    let hook_id = payload.get("id").and_then(|v| v.as_str()).unwrap_or("-");
    println!("Deactivated endpoint: {hook_id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_signed_endpoint_uses_bare_webhook_url_as_primary() {
        let urls = endpoint_urls(
            "https://api.getkite.sh",
            "/hooks/team/linear",
            Some("/hooks/team/linear/khk_secret"),
            true,
        );

        assert_eq!(
            urls.primary_webhook_url,
            "https://api.getkite.sh/hooks/team/linear"
        );
        assert_eq!(
            urls.bearer_webhook_url,
            "https://api.getkite.sh/hooks/team/linear/khk_secret"
        );
    }

    #[test]
    fn unsigned_endpoint_uses_bearer_webhook_url_as_primary() {
        let urls = endpoint_urls(
            "https://api.getkite.sh",
            "/hooks/team/generic",
            Some("/hooks/team/generic/khk_secret"),
            false,
        );

        assert_eq!(
            urls.primary_webhook_url,
            "https://api.getkite.sh/hooks/team/generic/khk_secret"
        );
    }

    #[test]
    fn signing_secret_can_be_read_from_stdin() {
        let mut input = " lin_wh_new_secret \n".as_bytes();
        let secret = resolve_signing_secret(Some("-"), &mut input)
            .expect("secret resolved")
            .expect("secret present");

        assert_eq!(secret, "lin_wh_new_secret");
    }
}
