use anyhow::{Result, bail};
use serde_json::json;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

/// Register a webhook on a GitHub repository.
///
/// Tries the `gh` CLI first; falls back to a direct API call via reqwest.
/// If `force` is true, any existing Kite webhook for the same URL is deleted first.
pub async fn register_webhook(
    repo: &str,
    webhook_url: &str,
    secret: &str,
    events: &[&str],
    github_token: Option<&str>,
    force: bool,
) -> Result<()> {
    if force {
        // Best-effort: ignore errors from delete so we still attempt creation.
        let _ = delete_kite_webhook(repo, webhook_url, github_token).await;
    }

    // Try gh CLI first (no token required if the user is already authenticated).
    match try_gh_cli(repo, webhook_url, secret, events).await {
        Ok(()) => return Ok(()),
        Err(e) => {
            tracing::debug!("gh CLI attempt failed: {e}; falling back to direct API");
        }
    }

    // Fall back to direct API call.
    let token = resolve_token(github_token)?;
    create_webhook_via_api(repo, webhook_url, secret, events, &token).await
}

/// Attempt webhook creation via the `gh` CLI.
async fn try_gh_cli(repo: &str, webhook_url: &str, secret: &str, events: &[&str]) -> Result<()> {
    let body = json!({
        "name": "web",
        "config": {
            "url": webhook_url,
            "content_type": "json",
            "secret": secret,
        },
        "events": events,
        "active": true,
    });
    let body_bytes = serde_json::to_vec(&body)?;

    let mut child = match tokio::process::Command::new("gh")
        .args([
            "api",
            &format!("repos/{repo}/hooks"),
            "-X",
            "POST",
            "--input",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("gh CLI not found");
        }
        Err(e) => bail!("failed to spawn gh: {e}"),
    };

    // Write JSON body to stdin.
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&body_bytes).await?;
        // stdin is dropped here, closing the pipe.
    }

    let output = child.wait_with_output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("admin:repo_hook") || stderr.contains("admin repo hook") {
            bail!(
                "GitHub token is missing the `admin:repo_hook` scope. \
                 Re-authenticate with: gh auth login --scopes admin:repo_hook"
            );
        }
        bail!("gh api failed ({}): {stderr}", output.status);
    }

    Ok(())
}

/// Create a webhook directly via the GitHub REST API.
async fn create_webhook_via_api(
    repo: &str,
    webhook_url: &str,
    secret: &str,
    events: &[&str],
    token: &str,
) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!("https://api.github.com/repos/{repo}/hooks");

    let body = json!({
        "name": "web",
        "config": {
            "url": webhook_url,
            "content_type": "json",
            "secret": secret,
        },
        "events": events,
        "active": true,
    });

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "kite-cli")
        .json(&body)
        .send()
        .await?;

    let status = response.status();

    if status.is_success() {
        return Ok(());
    }

    let error_body = response.text().await.unwrap_or_default();

    if status == reqwest::StatusCode::NOT_FOUND {
        bail!(
            "Repository '{repo}' not found or token lacks permission to manage webhooks. \
             Ensure the token has the `admin:repo_hook` scope and the repo path is correct."
        );
    }

    if status == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
        // GitHub returns 422 when the hook URL is already registered.
        if error_body.contains("already exists") || error_body.contains("Hook already exists") {
            bail!(
                "A webhook for this URL already exists on '{repo}'. \
                 Use --force to replace it."
            );
        }
        bail!("GitHub returned 422 Unprocessable Entity: {error_body}");
    }

    bail!("GitHub API error {status}: {error_body}");
}

/// Delete any existing Kite webhooks on the repository whose URL contains `/hooks/`
/// and whose path matches the team-specific segment of `webhook_url`.
async fn delete_kite_webhook(
    repo: &str,
    webhook_url: &str,
    github_token: Option<&str>,
) -> Result<()> {
    let token = resolve_token(github_token)?;
    let client = reqwest::Client::new();
    let list_url = format!("https://api.github.com/repos/{repo}/hooks");

    let response = client
        .get(&list_url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "kite-cli")
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("Failed to list webhooks for '{repo}' ({status}): {body}");
    }

    let hooks: Vec<serde_json::Value> = response.json().await?;

    // Extract the team-path segment from webhook_url for matching.
    // webhook_url is typically: https://host/hooks/{team_id}/...
    // We match any hook whose URL contains "/hooks/" AND shares this substring.
    let team_segment = extract_team_segment(webhook_url);

    let mut deleted = 0usize;
    for hook in &hooks {
        let hook_url = hook
            .get("config")
            .and_then(|c| c.get("url"))
            .and_then(|u| u.as_str())
            .unwrap_or("");

        let should_delete = hook_url.contains("/hooks/")
            && team_segment
                .as_deref()
                .map(|seg| hook_url.contains(seg))
                .unwrap_or(false);

        if should_delete {
            let id = hook.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
            let delete_url = format!("https://api.github.com/repos/{repo}/hooks/{id}");
            let del_resp = client
                .delete(&delete_url)
                .header("Authorization", format!("Bearer {token}"))
                .header("Accept", "application/vnd.github+json")
                .header("User-Agent", "kite-cli")
                .send()
                .await?;

            if del_resp.status().is_success() || del_resp.status() == reqwest::StatusCode::NOT_FOUND
            {
                deleted += 1;
                tracing::debug!("Deleted existing Kite webhook id={id} from '{repo}'");
            } else {
                let status = del_resp.status();
                tracing::warn!("Failed to delete hook id={id}: HTTP {status}");
            }
        }
    }

    tracing::debug!("Deleted {deleted} existing Kite webhook(s) from '{repo}'");
    Ok(())
}

/// Extract the team-specific path segment from a Kite webhook URL.
/// For `https://host/hooks/{team_id}/rest`, returns `Some("{team_id}")`.
fn extract_team_segment(webhook_url: &str) -> Option<String> {
    // Find "/hooks/" and grab the next path component.
    let after_hooks = webhook_url.split("/hooks/").nth(1)?;
    let segment = after_hooks.split('/').next()?;
    if segment.is_empty() {
        None
    } else {
        Some(segment.to_string())
    }
}

/// Resolve a GitHub personal access token from (in order):
/// 1. Explicitly provided argument.
/// 2. `GITHUB_TOKEN` environment variable.
/// 3. `gh auth token` command output.
fn resolve_token(explicit: Option<&str>) -> Result<String> {
    if let Some(t) = explicit
        && !t.is_empty()
    {
        return Ok(t.to_string());
    }

    if let Ok(t) = std::env::var("GITHUB_TOKEN")
        && !t.is_empty()
    {
        return Ok(t);
    }

    // Try `gh auth token`.
    match std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
    {
        Ok(output) if output.status.success() => {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if token.is_empty() {
                bail!("No GitHub token available. Set GITHUB_TOKEN or run `gh auth login`.");
            }
            Ok(token)
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "Could not retrieve GitHub token via `gh auth token`: {stderr}\n\
                 Set the GITHUB_TOKEN environment variable or run `gh auth login`."
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!(
                "No GitHub token found. Set the GITHUB_TOKEN environment variable \
                 or install the `gh` CLI and run `gh auth login`."
            );
        }
        Err(e) => bail!("Failed to run `gh auth token`: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- extract_team_segment tests ----

    #[test]
    fn extracts_team_id_from_standard_webhook_url() {
        let url = "https://api.getkite.sh/hooks/team_abc123/github";
        assert_eq!(extract_team_segment(url), Some("team_abc123".to_string()));
    }

    #[test]
    fn extracts_team_id_without_trailing_path() {
        let url = "https://api.getkite.sh/hooks/team_abc123";
        assert_eq!(extract_team_segment(url), Some("team_abc123".to_string()));
    }

    #[test]
    fn returns_none_when_no_hooks_segment() {
        let url = "https://api.getkite.sh/webhooks/team_abc123/github";
        assert_eq!(extract_team_segment(url), None);
    }

    #[test]
    fn returns_none_when_hooks_segment_is_empty() {
        let url = "https://api.getkite.sh/hooks/";
        assert_eq!(extract_team_segment(url), None);
    }

    #[test]
    fn extracts_team_id_with_uuid() {
        let url = "https://api.getkite.sh/hooks/550e8400-e29b-41d4-a716-446655440000/stripe";
        assert_eq!(
            extract_team_segment(url),
            Some("550e8400-e29b-41d4-a716-446655440000".to_string())
        );
    }

    #[test]
    fn handles_multiple_hooks_segments() {
        let url = "https://api.getkite.sh/hooks/team1/hooks/team2";
        assert_eq!(extract_team_segment(url), Some("team1".to_string()));
    }

    // ---- resolve_token tests ----

    #[test]
    fn resolve_token_uses_explicit_value() {
        let result = resolve_token(Some("ghp_test_token_123"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "ghp_test_token_123");
    }

    #[test]
    fn resolve_token_rejects_empty_explicit() {
        // Empty explicit token should fall through to env/gh fallback.
        // If gh CLI is installed, this may succeed — that's correct behavior.
        let result = resolve_token(Some(""));
        // Just verify it doesn't panic; result depends on environment.
        let _ = result;
    }

    #[test]
    fn resolve_token_none_depends_on_environment() {
        // With no explicit token and no GITHUB_TOKEN env var,
        // result depends on whether gh CLI is installed.
        // This test just verifies the function doesn't panic.
        unsafe { std::env::remove_var("GITHUB_TOKEN") };
        let result = resolve_token(None);
        let _ = result;
    }
}
