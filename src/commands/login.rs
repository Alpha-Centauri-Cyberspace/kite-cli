use anyhow::Result;

use crate::config::KiteConfig;

pub async fn run(server: String) -> Result<()> {
    eprintln!("Kite Login");
    eprintln!();

    // Step 1: Request a device code from the Next.js app
    let client = reqwest::Client::new();
    let device_code_url = format!("{server}/api/auth/device-code");

    eprintln!("Requesting device code from {device_code_url}...");

    let response = client
        .post(&device_code_url)
        .json(&serde_json::json!({}))
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!(
            "Failed to request device code: {} {}",
            response.status(),
            response.text().await.unwrap_or_default()
        );
    }

    let body: serde_json::Value = response.json().await?;
    let device_code = body["device_code"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing device_code in response"))?;
    let user_code = body["user_code"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing user_code in response"))?;
    let default_url = format!("{server}/activate");
    let verification_url_raw = body["verification_url"].as_str().unwrap_or(&default_url);
    let verification_url = sanitize_url(verification_url_raw);
    let preferred_ws_url = body
        .get("ws_url")
        .and_then(|v| v.as_str())
        .map(sanitize_url)
        .filter(|v| !v.is_empty());

    // Step 2: Show the user code and open browser
    eprintln!();
    eprintln!("Open {} and enter code:", verification_url);
    eprintln!();
    eprintln!("  {user_code}");
    eprintln!();

    // Try to open browser automatically
    if open_browser(&verification_url).is_err() {
        eprintln!("(Could not open browser automatically — please open the URL manually)");
    }

    // Step 3: Poll for approval
    let device_token_url = format!("{server}/api/auth/device-token");
    let poll_interval = body["interval"].as_u64().unwrap_or(5);

    eprintln!("Waiting for authorization...");

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(poll_interval)).await;

        let response = client
            .post(&device_token_url)
            .json(&serde_json::json!({ "device_code": device_code }))
            .send()
            .await?;

        let status = response.status();
        let token_body: serde_json::Value = response.json().await?;

        if status.is_success()
            && let (Some(api_key), Some(team_id)) = (
                token_body["api_key"].as_str(),
                token_body["team_id"].as_str(),
            )
        {
            // Save to config
            let mut config = KiteConfig::load().unwrap_or_default();
            config.auth.api_key = Some(api_key.to_string());
            config.auth.team_id = Some(team_id.to_string());

            // Prefer explicit ws_url from server response, fallback to derivation.
            let ws_url = preferred_ws_url
                .clone()
                .unwrap_or_else(|| derive_ws_url(&server));
            config.server.url = ws_url;
            config.server.http_base = Some(crate::config::derive_http_base(&config.server.url));

            config.save()?;

            eprintln!();
            eprintln!("Logged in successfully!");
            eprintln!("API key saved to {:?}", crate::config::config_path());
            return Ok(());
        }

        let error = token_body["error"].as_str().unwrap_or("");
        match error {
            "authorization_pending" => {
                // Keep polling
                eprint!(".");
            }
            "slow_down" => {
                // Back off
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
            "expired_token" => {
                anyhow::bail!("Device code expired. Please run `kite login` again.");
            }
            "access_denied" => {
                anyhow::bail!("Authorization denied.");
            }
            _ => {
                if !status.is_success() {
                    // Keep polling on other errors
                    eprint!(".");
                }
            }
        }
    }
}

fn sanitize_url(value: &str) -> String {
    value.chars().filter(|c| !c.is_whitespace()).collect()
}

fn derive_ws_url(server: &str) -> String {
    let base = sanitize_url(server)
        .trim_end_matches('/')
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    if base.ends_with("/ws") {
        base
    } else {
        format!("{base}/ws")
    }
}

fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", url])
            .spawn()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_url_strips_all_whitespace_variants() {
        // Spaces, tabs, CR, LF that a careless copy/paste could introduce.
        assert_eq!(
            sanitize_url(" https://example.com/path "),
            "https://example.com/path"
        );
        assert_eq!(
            sanitize_url("https://example.com\t/path\n"),
            "https://example.com/path"
        );
        assert_eq!(
            sanitize_url("https://example.com /path\r\n"),
            "https://example.com/path"
        );
    }

    #[test]
    fn sanitize_url_preserves_query_params_and_fragments() {
        let input = "https://example.com/activate?user_code=ABCD-1234#verify";
        assert_eq!(sanitize_url(input), input);
    }

    #[test]
    fn derive_ws_url_converts_https_to_wss() {
        assert_eq!(
            derive_ws_url("https://api.getkite.sh"),
            "wss://api.getkite.sh/ws"
        );
    }

    #[test]
    fn derive_ws_url_converts_http_to_ws() {
        assert_eq!(
            derive_ws_url("http://localhost:3000"),
            "ws://localhost:3000/ws"
        );
    }

    #[test]
    fn derive_ws_url_preserves_existing_ws_suffix() {
        // If the server URL already ends with /ws, don't double-append.
        assert_eq!(
            derive_ws_url("https://api.getkite.sh/ws"),
            "wss://api.getkite.sh/ws"
        );
    }

    #[test]
    fn derive_ws_url_strips_trailing_slashes_before_appending() {
        assert_eq!(
            derive_ws_url("https://api.getkite.sh/"),
            "wss://api.getkite.sh/ws"
        );
        assert_eq!(
            derive_ws_url("https://api.getkite.sh///"),
            "wss://api.getkite.sh/ws"
        );
    }

    #[test]
    fn derive_ws_url_strips_whitespace_before_converting() {
        // sanitize_url is called first, so whitespace in the source URL doesn't
        // leak into the derived WebSocket URL.
        assert_eq!(
            derive_ws_url("  https://api.getkite.sh\n"),
            "wss://api.getkite.sh/ws"
        );
    }
}
