use anyhow::{Context, Result};

use crate::platform::{self, OrchestrationPlatform};
use crate::skill_content::SKILL_MD;

const SKILL_FILENAME: &str = "SKILL.md";
const SKILL_NAME: &str = "kite";

pub fn run(format: Option<String>, auto: bool) -> Result<()> {
    if auto {
        run_auto()
    } else {
        let fmt = format.unwrap_or_else(|| "claude".to_string());
        let p = platform::platform_by_key(&fmt)?;
        run_for_platform(p.as_ref())
    }
}

fn run_auto() -> Result<()> {
    let platforms = platform::all_platforms();
    let mut exported = false;

    for p in &platforms {
        let dir = platform::resolve_skills_dir(p.as_ref());

        // Write to this dir if it exists or its parent exists.
        let should_write =
            dir.exists() || dir.parent().map(|parent| parent.exists()).unwrap_or(false);

        if should_write {
            let dest = dir.join(SKILL_NAME).join(SKILL_FILENAME);
            write_skill(&dest).with_context(|| {
                format!(
                    "failed to write skill to {} ({})",
                    dest.display(),
                    p.display_name()
                )
            })?;
            println!("Exported: {} ({})", dest.display(), p.display_name());
            exported = true;
        }
    }

    if !exported {
        // Fall back to default claude path.
        let claude = platform::platform_by_key("claude")?;
        run_for_platform(claude.as_ref())?;
    }

    Ok(())
}

fn run_for_platform(p: &dyn OrchestrationPlatform) -> Result<()> {
    let dir = platform::resolve_skills_dir(p);
    let dest = dir.join(SKILL_NAME).join(SKILL_FILENAME);
    write_skill(&dest).with_context(|| format!("failed to write skill to {}", dest.display()))?;
    println!("Exported: {}", dest.display());
    Ok(())
}

fn write_skill(dest: &std::path::Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    std::fs::write(dest, SKILL_MD)
        .with_context(|| format!("failed to write {}", dest.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn make_temp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn write_skill_creates_parent_dirs_and_writes_content() {
        let root = make_temp_dir("kite-export-test");
        let dest = root.join("kite").join(SKILL_FILENAME);

        write_skill(&dest).expect("write_skill should succeed");

        assert!(dest.exists(), "SKILL.md should exist");
        let content = fs::read_to_string(&dest).expect("read SKILL.md");
        assert!(
            content.contains("name: kite"),
            "should contain frontmatter name"
        );
        assert!(
            content.contains("Quick Start"),
            "should contain Quick Start section"
        );
    }

    #[test]
    fn run_with_claude_format_writes_to_claude_skills() {
        let root = make_temp_dir("kite-export-claude");
        let original_dir = std::env::current_dir().expect("current dir");
        std::env::set_current_dir(&root).expect("chdir");

        let result = run(Some("claude".to_string()), false);

        std::env::set_current_dir(original_dir).expect("restore dir");

        assert!(result.is_ok(), "export should succeed: {:?}", result);
        let dest = root.join(".claude/skills/kite").join(SKILL_FILENAME);
        assert!(dest.exists(), "SKILL.md should exist at {}", dest.display());
    }

    #[test]
    fn run_with_unknown_format_returns_error() {
        let err = run(Some("bogus".to_string()), false).expect_err("should fail");
        assert!(err.to_string().contains("unknown platform"));
    }
}
