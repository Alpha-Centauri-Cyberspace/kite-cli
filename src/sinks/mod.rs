pub mod exec;
pub mod mcp;
pub mod proxy;
pub mod socket;
pub mod stdout;

use anyhow::Result;
use cloudevents::Event;

/// Result of processing a single event through a sink.
pub enum SinkResult {
    /// Event processed successfully.
    Ok,
    /// Event should be retried (saved to DLQ).
    Retry,
}

/// Trait for event sinks — destinations where CloudEvents are routed.
///
/// Note: We use `async fn in trait` which requires that concrete types
/// are used rather than `dyn Sink` trait objects.
#[allow(async_fn_in_trait)]
pub trait Sink: Send {
    /// Called once when the sink starts.
    async fn start(&mut self) -> Result<()>;
    /// Called for each incoming CloudEvent.
    async fn handle(&mut self, event: &Event) -> Result<SinkResult>;
}
