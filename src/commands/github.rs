use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{commands::rpc, config::KiteConfig};

const GITHUB_API_VERSION: &str = "2026-03-10";
const DEFAULT_EVENTS: &[&str] = &[
    "push",
    "pull_request",
    "issues",
    "issue_comment",
    "pull_request_review",
    "pull_request_review_comment",
];

#[derive(Debug, Deserialize)]
struct RepoWebhookConfig {
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RepoWebhook {
    id: u64,
    active: bool,
    events: Vec<String>,
    config: RepoWebhookConfig,
}

#[derive(Debug, Deserialize)]
struct PrepareInstallResponse {
    id: String,
    team_id: String,
    endpoint_with_token: String,
    github_webhook_secret: String,
    secret_rotated: bool,
}

pub async fn install(
    repo: String,
    events: Vec<String>,
    all_events: bool,
    rotate_secret: bool,
) -> Result<()> {
    ensure_gh_available()?;
    ensure_gh_auth()?;

    let (owner, repo_name) = parse_repo(&repo)?;
    let resolved_events = resolve_events(events, all_events)?;

    let config = KiteConfig::load()?;
    let hook_base_url = config.hook_base_url()?;
    let prepared: PrepareInstallResponse = serde_json::from_value(
        rpc::call(
            "hooks.github.prepare_install",
            serde_json::json!({ "rotate_secret": rotate_secret }),
        )
        .await?,
    )
    .context("failed to decode Kite GitHub install preparation response")?;

    let payload_url = format!("{}{}", hook_base_url, prepared.endpoint_with_token);
    let managed_prefix = kite_managed_url_prefix(&hook_base_url, &prepared.team_id);
    let hooks = list_repo_hooks(&owner, &repo_name)?;

    let matches: Vec<_> = hooks
        .into_iter()
        .filter(|hook| is_kite_managed_hook(hook, &managed_prefix))
        .collect();

    let final_hook = match matches.as_slice() {
        [] => create_repo_hook(
            &owner,
            &repo_name,
            &payload_url,
            &prepared.github_webhook_secret,
            &resolved_events,
        )?,
        [hook] => update_repo_hook(
            &owner,
            &repo_name,
            hook.id,
            &payload_url,
            &prepared.github_webhook_secret,
            &resolved_events,
        )?,
        _ => {
            let ids = matches
                .iter()
                .map(|hook| hook.id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "multiple Kite-managed GitHub webhooks already exist for {repo} (hook ids: {ids}). Remove duplicates in GitHub, then rerun `kite github install --repo {repo}`."
            );
        }
    };

    println!("Installed GitHub webhook for {owner}/{repo_name}");
    println!("- endpoint_id: {}", prepared.id);
    println!("- team_id: {}", prepared.team_id);
    println!("- hook_id: {}", final_hook.id);
    println!("- active: {}", final_hook.active);
    println!("- payload_url: {}", payload_url);
    println!("- events: {}", final_hook.events.join(","));
    println!("- github_secret_rotated: {}", prepared.secret_rotated);
    println!();
    println!(
        "This command used your local `gh` authentication. Re-run with `--events`, `--all-events`, or `--rotate-secret` to change the GitHub webhook configuration."
    );

    Ok(())
}

fn ensure_gh_available() -> Result<()> {
    match Command::new("gh").arg("--version").output() {
        Ok(output) if output.status.success() => Ok(()),
        Ok(_) => bail!(
            "`gh` is installed but not functioning correctly. Reinstall GitHub CLI and rerun."
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            bail!("`gh` is not installed. Install GitHub CLI, then rerun `kite github install`.")
        }
        Err(error) => Err(error).context("failed to execute `gh --version`"),
    }
}

fn ensure_gh_auth() -> Result<()> {
    let output = Command::new("gh")
        .args(["auth", "status", "--hostname", "github.com"])
        .output()
        .context("failed to execute `gh auth status`")?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
        "GitHub CLI is not authenticated for github.com. Run `gh auth login -h github.com`, then rerun. Details: {}",
        stderr.trim()
    );
}

fn parse_repo(repo: &str) -> Result<(String, String)> {
    let trimmed = repo.trim();
    let Some((owner, name)) = trimmed.split_once('/') else {
        bail!("invalid repo `{trimmed}`. Expected OWNER/REPO.");
    };
    let owner = owner.trim();
    let name = name.trim();
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        bail!("invalid repo `{trimmed}`. Expected OWNER/REPO.");
    }
    Ok((owner.to_string(), name.to_string()))
}

fn resolve_events(events: Vec<String>, all_events: bool) -> Result<Vec<String>> {
    if all_events {
        return Ok(vec!["*".to_string()]);
    }

    let candidates = if events.is_empty() {
        DEFAULT_EVENTS
            .iter()
            .map(|event| event.to_string())
            .collect()
    } else {
        events
    };

    let mut deduped = Vec::new();
    for event in candidates {
        let normalized = event.trim().to_lowercase();
        if normalized.is_empty() {
            continue;
        }
        if !is_valid_github_event_name(&normalized) {
            bail!(
                "invalid GitHub event `{normalized}`. Use event names like `push`, `pull_request`, or `issue_comment`."
            );
        }
        if !deduped.contains(&normalized) {
            deduped.push(normalized);
        }
    }

    if deduped.is_empty() {
        bail!("no GitHub events resolved. Pass `--events` with at least one event name.");
    }

    Ok(deduped)
}

fn is_valid_github_event_name(value: &str) -> bool {
    value == "*"
        || value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
}

fn kite_managed_url_prefix(hook_base_url: &str, team_id: &str) -> String {
    format!(
        "{}/hooks/{}/github",
        hook_base_url.trim_end_matches('/'),
        team_id
    )
}

fn is_kite_managed_hook(hook: &RepoWebhook, prefix: &str) -> bool {
    hook.config
        .url
        .as_deref()
        .is_some_and(|url| url == prefix || url.starts_with(&format!("{prefix}/")))
}

fn list_repo_hooks(owner: &str, repo: &str) -> Result<Vec<RepoWebhook>> {
    let endpoint = format!("repos/{owner}/{repo}/hooks?per_page=100");
    gh_api_json(vec!["api".to_string(), endpoint])
        .context("failed to list GitHub repository webhooks")
}

fn create_repo_hook(
    owner: &str,
    repo: &str,
    payload_url: &str,
    secret: &str,
    events: &[String],
) -> Result<RepoWebhook> {
    let endpoint = format!("repos/{owner}/{repo}/hooks");
    let args = hook_mutation_args(&endpoint, "POST", payload_url, secret, events);
    gh_api_json(args).context("failed to create GitHub repository webhook")
}

fn update_repo_hook(
    owner: &str,
    repo: &str,
    hook_id: u64,
    payload_url: &str,
    secret: &str,
    events: &[String],
) -> Result<RepoWebhook> {
    let endpoint = format!("repos/{owner}/{repo}/hooks/{hook_id}");
    let args = hook_mutation_args(&endpoint, "PATCH", payload_url, secret, events);
    gh_api_json(args).context("failed to update GitHub repository webhook")
}

fn hook_mutation_args(
    endpoint: &str,
    method: &str,
    payload_url: &str,
    secret: &str,
    events: &[String],
) -> Vec<String> {
    let mut args = vec![
        "api".to_string(),
        endpoint.to_string(),
        "--method".to_string(),
        method.to_string(),
        "-f".to_string(),
        "name=web".to_string(),
        "-F".to_string(),
        "active=true".to_string(),
        "-f".to_string(),
        "config[content_type]=json".to_string(),
        "-f".to_string(),
        "config[insecure_ssl]=0".to_string(),
    ];

    args.push("-f".to_string());
    args.push(format!("config[url]={payload_url}"));
    args.push("-f".to_string());
    args.push(format!("config[secret]={secret}"));

    for event in events {
        args.push("-f".to_string());
        args.push(format!("events[]={event}"));
    }

    args
}

fn gh_api_json<T>(args: Vec<String>) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let mut command = Command::new("gh");
    if matches!(args.first().map(|value| value.as_str()), Some("api")) {
        command.arg("api");
        command.arg("--hostname");
        command.arg("github.com");
        command.arg("-H");
        command.arg("Accept: application/vnd.github+json");
        command.arg("-H");
        command.arg(format!("X-GitHub-Api-Version: {GITHUB_API_VERSION}"));
        command.args(args.into_iter().skip(1));
    } else {
        command.args(args);
    }

    let output = command.output().context("failed to execute `gh api`")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{}", stderr.trim());
    }

    serde_json::from_slice(&output.stdout).context("failed to parse GitHub API response")
}

#[cfg(test)]
mod tests {
    use super::{RepoWebhook, RepoWebhookConfig, is_kite_managed_hook, parse_repo, resolve_events};

    #[test]
    fn parse_repo_requires_owner_and_name() {
        let parsed = parse_repo("lommaj/kite").expect("repo");
        assert_eq!(parsed.0, "lommaj");
        assert_eq!(parsed.1, "kite");
        assert!(parse_repo("kite").is_err());
        assert!(parse_repo("lommaj/").is_err());
    }

    #[test]
    fn resolve_events_uses_default_bundle() {
        let events = resolve_events(Vec::new(), false).expect("events");
        assert_eq!(
            events,
            vec![
                "push",
                "pull_request",
                "issues",
                "issue_comment",
                "pull_request_review",
                "pull_request_review_comment"
            ]
        );
    }

    #[test]
    fn resolve_events_replaces_defaults_and_dedupes() {
        let events = resolve_events(
            vec![
                "push".to_string(),
                "issue_comment".to_string(),
                "push".to_string(),
            ],
            false,
        )
        .expect("events");
        assert_eq!(events, vec!["push", "issue_comment"]);
    }

    #[test]
    fn resolve_events_supports_all_events() {
        let events = resolve_events(vec!["push".to_string()], true).expect("events");
        assert_eq!(events, vec!["*"]);
    }

    #[test]
    fn managed_hook_match_uses_kite_url_prefix() {
        let hook = RepoWebhook {
            id: 1,
            active: true,
            events: vec!["push".to_string()],
            config: RepoWebhookConfig {
                url: Some(
                    "https://api.getkite.sh/hooks/team_123/github/khk_abc_secret".to_string(),
                ),
            },
        };
        assert!(is_kite_managed_hook(
            &hook,
            "https://api.getkite.sh/hooks/team_123/github"
        ));
        assert!(!is_kite_managed_hook(
            &hook,
            "https://api.getkite.sh/hooks/team_other/github"
        ));
    }
}
