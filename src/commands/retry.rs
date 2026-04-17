use anyhow::Result;

pub async fn run(source: Option<String>, target: String) -> Result<()> {
    eprintln!("Note: `kite retry` is deprecated. Use `kite queue replay` instead.");
    eprintln!("Running: kite queue replay --target {target}");
    crate::commands::queue::replay(Some("failed".to_string()), source, None, target).await
}
