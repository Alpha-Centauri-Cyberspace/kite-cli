use crate::manifest::EnrichmentHook;
use anyhow::Result;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Run matching enrichment hooks against an event.
/// Returns the enrichment payload JSON string, or None if no hooks matched or all failed.
pub async fn run_enrichment(
    hooks: &[EnrichmentHook],
    source: &str,
    event_type: &str,
    payload: &serde_json::Value,
    raw_event_json: &str,
) -> Option<String> {
    for hook in hooks {
        if !hook.match_rule.matches(source, event_type, payload) {
            continue;
        }

        let timeout = parse_timeout(&hook.timeout).unwrap_or(std::time::Duration::from_secs(30));

        match run_hook(&hook.run, raw_event_json, timeout).await {
            Ok(output) => {
                if serde_json::from_str::<serde_json::Value>(&output).is_ok() {
                    return Some(output);
                } else {
                    tracing::warn!(
                        "Enrichment hook `{}` returned invalid JSON, skipping",
                        hook.run
                    );
                }
            }
            Err(e) => {
                tracing::warn!("Enrichment hook `{}` failed: {e}", hook.run);
            }
        }
    }
    None
}

async fn run_hook(command: &str, stdin_data: &str, timeout: std::time::Duration) -> Result<String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        // Ignore broken-pipe errors: the child may exit before reading all stdin
        // (e.g., a command that only writes stdout without consuming input).
        let _ = stdin.write_all(stdin_data.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }

    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| anyhow::anyhow!("Enrichment hook timed out after {:?}", timeout))??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Hook exited with {}: {}", output.status, stderr.trim());
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn parse_timeout(s: &str) -> Option<std::time::Duration> {
    let s = s.trim();
    if s.len() < 2 {
        return None;
    }
    let (num_str, unit) = s.split_at(s.len() - 1);
    let num: u64 = num_str.parse().ok()?;
    match unit {
        "s" => Some(std::time::Duration::from_secs(num)),
        "m" => Some(std::time::Duration::from_secs(num * 60)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_hook_echo() {
        // Use cat to consume stdin, then printf to produce output.
        // This avoids timing issues where the command exits before stdin is fully written.
        let result = run_hook(
            r#"cat >/dev/null; printf '{"enriched":true}\n'"#,
            "{}",
            std::time::Duration::from_secs(5),
        )
        .await;
        assert!(result.is_ok(), "run_hook failed: {:?}", result.err());
        let parsed: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(parsed["enriched"], true);
    }

    #[tokio::test]
    async fn test_run_hook_reads_stdin() {
        let result = run_hook(
            "cat",
            r#"{"input":"hello"}"#,
            std::time::Duration::from_secs(5),
        )
        .await;
        assert!(result.is_ok());
        let parsed: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(parsed["input"], "hello");
    }

    #[tokio::test]
    async fn test_run_hook_failure() {
        let result = run_hook("exit 1", "{}", std::time::Duration::from_secs(5)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_run_hook_timeout() {
        let result = run_hook("sleep 10", "{}", std::time::Duration::from_millis(100)).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timed out"));
    }

    #[test]
    fn test_parse_timeout() {
        assert_eq!(
            parse_timeout("30s"),
            Some(std::time::Duration::from_secs(30))
        );
        assert_eq!(
            parse_timeout("2m"),
            Some(std::time::Duration::from_secs(120))
        );
        assert_eq!(parse_timeout("bad"), None);
    }
}
