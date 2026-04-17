use super::{Sink, SinkResult};
use anyhow::Result;
use cloudevents::Event;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub struct ExecSink {
    command: String,
}

impl ExecSink {
    pub fn new(command: String) -> Self {
        Self { command }
    }
}

impl Sink for ExecSink {
    async fn start(&mut self) -> Result<()> {
        eprintln!("Exec sink: will run `{}` per event", self.command);
        Ok(())
    }

    async fn handle(&mut self, event: &Event) -> Result<SinkResult> {
        let json = serde_json::to_string(event)?;

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&self.command)
            .stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(json.as_bytes()).await?;
            stdin.shutdown().await?;
        }

        let status = child.wait().await?;
        if status.success() {
            Ok(SinkResult::Ok)
        } else {
            Ok(SinkResult::Retry)
        }
    }
}
