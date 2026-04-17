use anyhow::{Context, Result, bail};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::path::Path;
use std::process::Command;
use uuid::Uuid;

const DEFAULT_UPDATE_SERVER: &str = "https://downloads.getkite.sh";
const FALLBACK_UPDATE_SERVER: &str = "https://pub-8c89023eee8443d0acfbb4cdc0d65494.r2.dev";

pub async fn run(server: Option<String>, check: bool, force: bool) -> Result<()> {
    let base = server
        .unwrap_or_else(|| DEFAULT_UPDATE_SERVER.to_string())
        .trim()
        .trim_end_matches('/')
        .to_string();
    let current_version = env!("CARGO_PKG_VERSION").to_string();

    let (latest, effective_base) = fetch_latest_release(&base).await?;
    let latest_tag = latest
        .get("tag_name")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let latest_version = latest_tag.trim_start_matches('v');

    let cmp = compare_versions(&current_version, latest_version);
    if check {
        match cmp {
            Ordering::Less => {
                println!(
                    "Update available: current={} latest={}",
                    current_version, latest_tag
                );
            }
            Ordering::Equal => {
                println!("Up to date: {}", current_version);
            }
            Ordering::Greater => {
                println!(
                    "Current version ({}) is newer than latest release ({})",
                    current_version, latest_tag
                );
            }
        }
        return Ok(());
    }

    if cmp != Ordering::Less && !force {
        println!("Already up to date ({})", current_version);
        return Ok(());
    }

    let asset = detect_asset_name()?;
    let download_url = resolve_asset_download_url(&latest, &effective_base, latest_tag, &asset);
    let expected_sha256 =
        resolve_asset_sha256(&latest, &asset).or(fetch_sha256_sidecar(&download_url).await);
    eprintln!("Downloading {latest_tag} ({asset})...");

    let tmp_dir = std::env::temp_dir().join(format!("kite-update-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&tmp_dir)?;
    let tar_path = tmp_dir.join("kite.tar.gz");
    let unpacked_path = tmp_dir.join("kite");

    let bytes = reqwest::get(&download_url)
        .await
        .with_context(|| format!("failed to request {download_url}"))?
        .error_for_status()
        .with_context(|| format!("failed to download {download_url}"))?
        .bytes()
        .await
        .context("failed reading update payload")?;

    if let Some(expected) = expected_sha256 {
        let actual = sha256_hex(&bytes);
        if actual != expected {
            bail!(
                "download checksum mismatch for {}: expected={} actual={}",
                asset,
                expected,
                actual
            );
        }
    }

    std::fs::write(&tar_path, &bytes).context("failed writing downloaded archive")?;

    let tar_status = Command::new("tar")
        .arg("-xzf")
        .arg(&tar_path)
        .arg("-C")
        .arg(&tmp_dir)
        .status()
        .context("failed to execute tar")?;
    if !tar_status.success() {
        bail!("failed to extract update archive");
    }
    if !unpacked_path.exists() {
        bail!("extracted archive did not contain kite binary");
    }

    let target = std::env::current_exe().context("failed to resolve current executable")?;
    install_binary(&unpacked_path, &target)?;

    // Verify the installed binary reports an expected version.
    let installed_version = read_binary_version(&target)
        .with_context(|| format!("failed to read installed version from {}", target.display()))?;
    let installed_cmp = compare_versions(&installed_version, latest_version);
    if installed_cmp == Ordering::Less {
        bail!(
            "self-update verification failed: installed binary at {} reports {}, expected at least {}",
            target.display(),
            installed_version,
            latest_tag
        );
    }

    // Best-effort PATH diagnostic (helps when another older `kite` shadows current_exe).
    if let Ok(path_version) = read_command_version("kite")
        && compare_versions(&path_version, latest_version) == Ordering::Less
    {
        eprintln!(
            "Warning: `kite` in PATH reports {} while updated binary at {} reports {}. You may have multiple installs.",
            path_version,
            target.display(),
            installed_version
        );
    }

    eprintln!(
        "Updated kite to {latest_tag} at {} (reported {})",
        target.display(),
        installed_version
    );
    let _ = std::fs::remove_dir_all(&tmp_dir);
    Ok(())
}

async fn fetch_latest_release(base: &str) -> Result<(Value, String)> {
    let manifest_url = format!("{base}/releases/latest.json");
    match fetch_json(&manifest_url).await {
        Ok(json) => Ok((json, base.to_string())),
        Err(primary_error) => {
            if base == FALLBACK_UPDATE_SERVER {
                return Err(primary_error);
            }

            let fallback_manifest_url = format!("{FALLBACK_UPDATE_SERVER}/releases/latest.json");
            match fetch_json(&fallback_manifest_url).await {
                Ok(json) => Ok((json, FALLBACK_UPDATE_SERVER.to_string())),
                Err(_) => Err(primary_error),
            }
        }
    }
}

async fn fetch_json(url: &str) -> Result<Value> {
    let response = reqwest::get(url)
        .await
        .with_context(|| format!("failed to request {url}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read update metadata response body")?;
    if !status.is_success() {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            bail!("update metadata request failed: {} ({})", url, status);
        }
        bail!(
            "update metadata request failed: {} ({}) - {}",
            url,
            status,
            trimmed
        );
    }
    serde_json::from_str::<Value>(&body).context("failed to parse update metadata JSON")
}

fn resolve_asset_download_url(latest: &Value, base: &str, latest_tag: &str, asset: &str) -> String {
    if let Some(asset_obj) = latest
        .get("assets")
        .and_then(Value::as_array)
        .and_then(|assets| {
            assets
                .iter()
                .find(|item| item.get("name").and_then(Value::as_str) == Some(asset))
        })
    {
        for key in [
            "download_url",
            "website_download_url",
            "url",
            "github_download_url",
        ] {
            if let Some(value) = asset_obj.get(key).and_then(Value::as_str)
                && !value.trim().is_empty()
            {
                return value.to_string();
            }
        }
    }

    format!("{base}/releases/{latest_tag}/{asset}")
}

fn resolve_asset_sha256(latest: &Value, asset: &str) -> Option<String> {
    let value = latest
        .get("assets")
        .and_then(Value::as_array)
        .and_then(|assets| {
            assets
                .iter()
                .find(|item| item.get("name").and_then(Value::as_str) == Some(asset))
        })
        .and_then(|item| item.get("sha256").and_then(Value::as_str))?;

    normalize_sha256(value)
}

async fn fetch_sha256_sidecar(download_url: &str) -> Option<String> {
    let sidecar_url = format!("{download_url}.sha256");
    let response = reqwest::get(&sidecar_url).await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = response.text().await.ok()?;
    let token = body.split_whitespace().next()?;
    normalize_sha256(token)
}

fn normalize_sha256(value: &str) -> Option<String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.len() == 64 && normalized.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(normalized);
    }
    None
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn detect_asset_name() -> Result<String> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        other => bail!("unsupported OS for self-update: {other}"),
    };

    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "arm64",
        "arm64" => "arm64",
        other => bail!("unsupported architecture for self-update: {other}"),
    };

    Ok(format!("kite-{os}-{arch}.tar.gz"))
}

fn install_binary(source: &Path, target: &Path) -> Result<()> {
    let target_str = target
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("invalid target executable path"))?;
    let source_str = source
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("invalid source executable path"))?;

    let direct_install = Command::new("install")
        .args(["-m", "0755", source_str, target_str])
        .status();

    match direct_install {
        Ok(status) if status.success() => return Ok(()),
        Ok(_) | Err(_) => {}
    }

    eprintln!("Need elevated permissions to replace {}", target.display());
    let sudo_status = Command::new("sudo")
        .args(["install", "-m", "0755", source_str, target_str])
        .status()
        .context("failed to execute sudo install")?;
    if sudo_status.success() {
        return Ok(());
    }

    bail!(
        "self-update failed: could not install binary at {}",
        target.display()
    )
}

fn read_binary_version(path: &Path) -> Result<String> {
    let output = Command::new(path)
        .arg("--version")
        .output()
        .with_context(|| format!("failed to run {} --version", path.display()))?;

    if !output.status.success() {
        bail!("{} --version exited with {}", path.display(), output.status);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    extract_version_from_output(&stdout)
        .ok_or_else(|| anyhow::anyhow!("could not parse version from output: {}", stdout.trim()))
}

fn read_command_version(cmd: &str) -> Result<String> {
    let output = Command::new(cmd)
        .arg("--version")
        .output()
        .with_context(|| format!("failed to run {cmd} --version"))?;

    if !output.status.success() {
        bail!("{cmd} --version exited with {}", output.status);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    extract_version_from_output(&stdout)
        .ok_or_else(|| anyhow::anyhow!("could not parse version from output: {}", stdout.trim()))
}

fn extract_version_from_output(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .map(|token| token.trim().trim_start_matches('v'))
        .find(|token| token.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(|token| token.to_string())
}

fn compare_versions(current: &str, latest: &str) -> Ordering {
    let current_parts = parse_semver_like(current);
    let latest_parts = parse_semver_like(latest);
    for i in 0..3 {
        let c = *current_parts.get(i).unwrap_or(&0);
        let l = *latest_parts.get(i).unwrap_or(&0);
        match c.cmp(&l) {
            Ordering::Equal => continue,
            ord => return ord,
        }
    }
    Ordering::Equal
}

fn parse_semver_like(value: &str) -> Vec<u64> {
    value
        .split('.')
        .map(|part| {
            part.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
        })
        .map(|digits| digits.parse::<u64>().unwrap_or(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::extract_version_from_output;

    #[test]
    fn parses_standard_version_output() {
        assert_eq!(
            extract_version_from_output("kite 0.1.4\n"),
            Some("0.1.4".to_string())
        );
    }

    #[test]
    fn strips_v_prefix_if_present() {
        assert_eq!(
            extract_version_from_output("kite v0.1.5\n"),
            Some("0.1.5".to_string())
        );
    }
}
