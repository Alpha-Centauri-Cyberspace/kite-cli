use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::platform::{self, OpenClaw, OrchestrationPlatform};

pub fn install(name: String) -> Result<()> {
    install_for_platform(name, &OpenClaw)
}

pub fn install_for_platform(name: String, platform: &dyn OrchestrationPlatform) -> Result<()> {
    let config = SkillConfig::from_platform(platform);
    let outcome = install_with_config(&name, &config, platform)?;

    println!("Installed skill `{name}` for {}.", platform.display_name());
    println!("Install location: {}", outcome.install_location.display());
    println!("Tip: run `kite skill list` to view installed skills.");

    Ok(())
}

pub fn list() -> Result<()> {
    list_for_platform(&OpenClaw)
}

pub fn list_for_platform(platform: &dyn OrchestrationPlatform) -> Result<()> {
    let config = SkillConfig::from_platform(platform);
    let mut skills = list_installed_skills(&config.skills_dir)?;
    skills.sort();

    if skills.is_empty() {
        println!(
            "No skills are installed yet for {}. Install location: {}",
            platform.display_name(),
            config.skills_dir.display()
        );
        println!("Install one with: kite skill install <name>");
        return Ok(());
    }

    println!(
        "Installed skills for {} ({}):",
        platform.display_name(),
        skills.len()
    );
    for skill in skills {
        println!("- {skill}");
    }
    println!("Install location: {}", config.skills_dir.display());

    Ok(())
}

#[derive(Debug, Clone)]
struct SkillConfig {
    installer_bin: String,
    skills_dir: PathBuf,
}

impl SkillConfig {
    fn from_platform(p: &dyn OrchestrationPlatform) -> Self {
        Self {
            installer_bin: platform::resolve_installer_bin(p),
            skills_dir: platform::resolve_skills_dir(p),
        }
    }
}

#[derive(Debug)]
struct InstallOutcome {
    install_location: PathBuf,
}

fn install_with_config(
    name: &str,
    config: &SkillConfig,
    platform: &dyn OrchestrationPlatform,
) -> Result<InstallOutcome> {
    validate_skill_name(name)?;

    std::fs::create_dir_all(&config.skills_dir).with_context(|| {
        format!(
            "failed to create skill install directory at {}",
            config.skills_dir.display()
        )
    })?;

    let attempts = vec![
        vec![
            "skills".to_string(),
            "install".to_string(),
            name.to_string(),
        ],
        vec!["skill".to_string(), "install".to_string(), name.to_string()],
    ];

    let mut attempt_labels = Vec::new();
    let mut last_failure = None;

    for args in attempts {
        let command_line = format!("{} {}", config.installer_bin, args.join(" "));
        attempt_labels.push(command_line.clone());

        let output = Command::new(&config.installer_bin)
            .args(&args)
            .output()
            .with_context(|| format!("failed to run installer command: {command_line}"));

        match output {
            Ok(output) if output.status.success() => {
                let expected_location = config.skills_dir.join(name);
                let install_location = if expected_location.exists() {
                    expected_location
                } else {
                    config.skills_dir.clone()
                };
                return Ok(InstallOutcome { install_location });
            }
            Ok(output) => {
                let detail = summarize_output(&output.stdout, &output.stderr);

                if is_command_shape_error(&detail) {
                    continue;
                }

                let status = output.status.code().unwrap_or(1);
                let message = format_install_failure(
                    name,
                    &config.skills_dir,
                    &attempt_labels,
                    platform,
                    Some(format!(
                        "{command_line} exited with code {status}: {detail}"
                    )),
                );
                bail!(message);
            }
            Err(err) => {
                last_failure = Some(err.to_string());
                continue;
            }
        }
    }

    let message = format_install_failure(
        name,
        &config.skills_dir,
        &attempt_labels,
        platform,
        last_failure.map(|msg| format!("installer invocation failed: {msg}")),
    );
    bail!(message)
}

fn list_installed_skills(skills_dir: &Path) -> Result<Vec<String>> {
    if !skills_dir.exists() {
        return Ok(Vec::new());
    }

    let mut installed = Vec::new();
    for entry in std::fs::read_dir(skills_dir)
        .with_context(|| format!("failed to read skills dir {}", skills_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        if let Some(name) = entry.file_name().to_str()
            && !name.starts_with('.')
        {
            installed.push(name.to_string());
        }
    }

    Ok(installed)
}

fn summarize_output(stdout: &[u8], stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }

    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }

    "no output".to_string()
}

fn is_command_shape_error(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    lower.contains("unrecognized subcommand")
        || lower.contains("unknown command")
        || lower.contains("did you mean")
}

fn format_install_failure(
    name: &str,
    install_dir: &Path,
    attempted_commands: &[String],
    platform: &dyn OrchestrationPlatform,
    detail: Option<String>,
) -> String {
    let mut message = format!(
        "Failed to install skill `{name}`.\nInstall location: {}\nAttempted installer commands:\n{}\n",
        install_dir.display(),
        attempted_commands
            .iter()
            .map(|line| format!("  - {line}"))
            .collect::<Vec<String>>()
            .join("\n")
    );

    if let Some(detail) = detail {
        message.push_str(&format!("Failure detail: {detail}\n"));
    }

    message.push_str(&format!(
        "Remediation:\n  1) Ensure {} CLI is installed and available on PATH.\n  2) Verify available installer commands with `{} --help`.\n  3) Retry with `kite skill install <name>`.",
        platform.display_name(),
        platform.installer_bin(),
    ));

    message
}

fn validate_skill_name(name: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("Skill name cannot be empty. Example: `kite skill install weather`");
    }

    let is_valid = trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
    if !is_valid || trimmed.starts_with('.') || trimmed.contains("..") {
        bail!(
            "`{name}` is not a valid skill name. Use only letters, numbers, '-', '_' or '.'. Example: `kite skill install weather`"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn make_temp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn install_succeeds_for_valid_name() {
        let root = make_temp_dir("kite-skill-install-success");
        let installer_path = root.join("fake-installer.sh");
        let skills_dir = root.join("skills");

        let script = r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" != "skills" || "${2:-}" != "install" ]]; then
  echo "unexpected args: $*" >&2
  exit 64
fi
name="${3:-}"
skills_root="$(cd "$(dirname "$0")" && pwd)/skills"
mkdir -p "$skills_root/$name"
"#;

        fs::write(&installer_path, script).expect("write fake installer");
        let mut perms = fs::metadata(&installer_path)
            .expect("stat installer")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&installer_path, perms).expect("chmod installer");

        let config = SkillConfig {
            installer_bin: installer_path.to_string_lossy().to_string(),
            skills_dir: skills_dir.clone(),
        };

        let result =
            install_with_config("weather", &config, &OpenClaw).expect("install should succeed");

        assert_eq!(result.install_location, skills_dir.join("weather"));
        assert!(result.install_location.exists());
    }

    #[test]
    fn install_rejects_invalid_name() {
        let config = SkillConfig {
            installer_bin: "openclaw".to_string(),
            skills_dir: PathBuf::from("/tmp/skills"),
        };

        let err =
            install_with_config("../oops", &config, &OpenClaw).expect_err("name should be invalid");
        let message = err.to_string();

        assert!(message.contains("not a valid skill name"));
        assert!(message.contains("kite skill install weather"));
    }
}
