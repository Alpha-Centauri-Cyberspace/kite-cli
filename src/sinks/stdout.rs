use std::io::Write;

use anyhow::Result;
use cloudevents::AttributesReader;
use cloudevents::Event;

use kite_protocol::extensions::{get_kiteseq, get_kitesummary};

use super::{Sink, SinkResult};

pub enum OutputMode {
    Pretty,
    Json,
    Compact,
}

pub struct StdoutSink {
    mode: OutputMode,
}

impl StdoutSink {
    pub fn new(json: bool, compact: bool) -> Self {
        let mode = if json {
            OutputMode::Json
        } else if compact {
            OutputMode::Compact
        } else {
            OutputMode::Pretty
        };
        Self { mode }
    }
}

impl Sink for StdoutSink {
    async fn start(&mut self) -> Result<()> {
        Ok(())
    }

    async fn handle(&mut self, event: &Event) -> Result<SinkResult> {
        // Flush after each event so downstream pipes (test harnesses, log
        // shippers, `tee`) observe events in real time. Rust block-buffers
        // stdout when it's a pipe — without this, low-volume streams sit in
        // the buffer indefinitely.
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        match self.mode {
            OutputMode::Json => {
                let json = serde_json::to_string(event)?;
                writeln!(out, "{json}")?;
            }
            OutputMode::Compact => {
                let summary = get_kitesummary(event).unwrap_or_else(|| event.ty().to_string());
                writeln!(out, "{summary}")?;
            }
            OutputMode::Pretty => {
                let now = chrono::Local::now().format("%H:%M:%S");
                let event_type = event.ty();
                let summary = get_kitesummary(event).unwrap_or_default();
                let seq = get_kiteseq(event)
                    .map(|s| format!("#{s}"))
                    .unwrap_or_default();

                writeln!(out, "[{now}] {seq} {event_type}  {summary}")?;
            }
        }
        out.flush()?;

        Ok(SinkResult::Ok)
    }
}
