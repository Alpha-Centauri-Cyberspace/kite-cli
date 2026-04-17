use anyhow::Result;
use cloudevents::{AttributesReader, Event};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, Notify};

use super::{Sink, SinkResult};

/// A buffered webhook event stored in the MCP sink's ring buffer.
#[derive(Clone, Serialize)]
struct BufferedEvent {
    id: String,
    source: String,
    event_type: String,
    time: Option<String>,
    summary: Option<String>,
    data: serde_json::Value,
    received_at: String,
}

impl BufferedEvent {
    fn from_cloudevent(event: &Event) -> Self {
        let data = event
            .data()
            .map(|d| match d {
                cloudevents::Data::Json(v) => v.clone(),
                cloudevents::Data::String(s) => serde_json::Value::String(s.clone()),
                cloudevents::Data::Binary(b) => serde_json::Value::String(base64_encode(b)),
            })
            .unwrap_or(serde_json::Value::Null);

        let summary = kite_protocol::extensions::get_kitesummary(event);

        Self {
            id: event.id().to_string(),
            source: event.source().to_string(),
            event_type: event.ty().to_string(),
            time: event.time().map(|t| t.to_rfc3339()),
            summary,
            data,
            received_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 4 / 3 + 4);
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        let _ = write!(s, "{}", CHARS[(b0 >> 2) & 0x3f] as char);
        let _ = write!(s, "{}", CHARS[((b0 << 4) | (b1 >> 4)) & 0x3f] as char);
        if chunk.len() > 1 {
            let _ = write!(s, "{}", CHARS[((b1 << 2) | (b2 >> 6)) & 0x3f] as char);
        } else {
            s.push('=');
        }
        if chunk.len() > 2 {
            let _ = write!(s, "{}", CHARS[b2 & 0x3f] as char);
        } else {
            s.push('=');
        }
    }
    s
}

struct EventBuffer {
    events: VecDeque<BufferedEvent>,
    capacity: usize,
}

impl EventBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            events: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    fn push(&mut self, event: BufferedEvent) {
        if self.events.len() >= self.capacity {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    fn list(
        &self,
        source: Option<&str>,
        event_type: Option<&str>,
        limit: usize,
    ) -> Vec<&BufferedEvent> {
        self.events
            .iter()
            .rev()
            .filter(|e| {
                source.is_none_or(|s| e.source.contains(s) || e.event_type.contains(s))
                    && event_type.is_none_or(|t| e.event_type.contains(t))
            })
            .take(limit)
            .collect()
    }

    fn get(&self, id: &str) -> Option<&BufferedEvent> {
        self.events.iter().find(|e| e.id == id)
    }
}

pub struct McpSink {
    buffer: Arc<Mutex<EventBuffer>>,
    notify: Arc<Notify>,
}

impl McpSink {
    pub fn new(buffer_size: usize) -> Self {
        let capacity = if buffer_size == 0 { 100 } else { buffer_size };
        Self {
            buffer: Arc::new(Mutex::new(EventBuffer::new(capacity))),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Get a handle for pushing events from the event loop.
    pub fn handle_pair(&self) -> McpSinkHandle {
        McpSinkHandle {
            buffer: Arc::clone(&self.buffer),
            notify: Arc::clone(&self.notify),
        }
    }
}

/// Cloneable handle used from the event loop to push events into the MCP buffer.
#[derive(Clone)]
pub struct McpSinkHandle {
    buffer: Arc<Mutex<EventBuffer>>,
    notify: Arc<Notify>,
}

impl McpSinkHandle {
    pub async fn push_event(&self, event: &Event) {
        let buffered = BufferedEvent::from_cloudevent(event);
        self.buffer.lock().await.push(buffered);
        self.notify.notify_waiters();
    }
}

impl Sink for McpSink {
    async fn start(&mut self) -> Result<()> {
        let buffer = Arc::clone(&self.buffer);
        let notify = Arc::clone(&self.notify);

        // Spawn MCP server on stdio in background
        tokio::spawn(async move {
            if let Err(e) = run_mcp_server(buffer, notify).await {
                tracing::error!("MCP server error: {e}");
            }
        });

        eprintln!("MCP server started on stdio");
        Ok(())
    }

    async fn handle(&mut self, event: &Event) -> Result<SinkResult> {
        let buffered = BufferedEvent::from_cloudevent(event);
        self.buffer.lock().await.push(buffered);
        self.notify.notify_waiters();
        Ok(SinkResult::Ok)
    }
}

// ── JSON-RPC 2.0 types ─────────────────────────────────────────────

#[derive(Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<serde_json::Value>,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

impl JsonRpcResponse {
    fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: serde_json::Value, code: i64, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError { code, message }),
        }
    }
}

// ── MCP server implementation ───────────────────────────────────────

async fn run_mcp_server(buffer: Arc<Mutex<EventBuffer>>, _notify: Arc<Notify>) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let reader = BufReader::new(stdin);
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Invalid JSON-RPC request: {e}");
                continue;
            }
        };

        if request.jsonrpc != "2.0" {
            continue;
        }

        // Notifications (no id) don't get a response
        let id = match request.id {
            Some(id) => id,
            None => {
                // Handle notification (e.g., "notifications/initialized")
                continue;
            }
        };

        let response = match request.method.as_str() {
            "initialize" => handle_initialize(id),
            "tools/list" => handle_tools_list(id),
            "tools/call" => handle_tools_call(id, request.params, &buffer).await,
            "resources/list" => handle_resources_list(id),
            "resources/read" => handle_resources_read(id, request.params, &buffer).await,
            _ => {
                JsonRpcResponse::error(id, -32601, format!("Method not found: {}", request.method))
            }
        };

        let response_json = serde_json::to_string(&response)?;
        stdout.write_all(response_json.as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
    }

    Ok(())
}

fn handle_initialize(id: serde_json::Value) -> JsonRpcResponse {
    JsonRpcResponse::success(
        id,
        serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {},
                "resources": {}
            },
            "serverInfo": {
                "name": "kite-mcp-server",
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
    )
}

fn handle_tools_list(id: serde_json::Value) -> JsonRpcResponse {
    JsonRpcResponse::success(
        id,
        serde_json::json!({
            "tools": [
                {
                    "name": "list_events",
                    "description": "List recent webhook events received by Kite. Returns events in reverse chronological order.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "source": {
                                "type": "string",
                                "description": "Filter by event source (e.g., 'github', 'stripe'). Partial match."
                            },
                            "type": {
                                "type": "string",
                                "description": "Filter by event type (e.g., 'com.github.push'). Partial match."
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Maximum number of events to return (default: 20, max: 100).",
                                "default": 20
                            }
                        }
                    }
                },
                {
                    "name": "get_event",
                    "description": "Get a specific webhook event by its ID, including full payload data.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "The event ID to retrieve."
                            }
                        },
                        "required": ["id"]
                    }
                }
            ]
        }),
    )
}

async fn handle_tools_call(
    id: serde_json::Value,
    params: serde_json::Value,
    buffer: &Arc<Mutex<EventBuffer>>,
) -> JsonRpcResponse {
    let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or(serde_json::Value::Object(Default::default()));

    match tool_name {
        "list_events" => {
            let source = arguments.get("source").and_then(|v| v.as_str());
            let event_type = arguments.get("type").and_then(|v| v.as_str());
            let limit = arguments
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(20)
                .min(100) as usize;

            let buf = buffer.lock().await;
            let events = buf.list(source, event_type, limit);
            let summaries: Vec<serde_json::Value> = events
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "id": e.id,
                        "source": e.source,
                        "type": e.event_type,
                        "time": e.time,
                        "summary": e.summary,
                        "received_at": e.received_at,
                    })
                })
                .collect();

            JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string_pretty(&serde_json::json!({
                            "count": summaries.len(),
                            "events": summaries,
                        })).unwrap_or_default()
                    }]
                }),
            )
        }
        "get_event" => {
            let event_id = arguments.get("id").and_then(|v| v.as_str()).unwrap_or("");

            if event_id.is_empty() {
                return JsonRpcResponse::success(
                    id,
                    serde_json::json!({
                        "content": [{
                            "type": "text",
                            "text": "Error: 'id' parameter is required."
                        }],
                        "isError": true
                    }),
                );
            }

            let buf = buffer.lock().await;
            match buf.get(event_id) {
                Some(event) => JsonRpcResponse::success(
                    id,
                    serde_json::json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string_pretty(event).unwrap_or_default()
                        }]
                    }),
                ),
                None => JsonRpcResponse::success(
                    id,
                    serde_json::json!({
                        "content": [{
                            "type": "text",
                            "text": format!("Event not found: {event_id}")
                        }],
                        "isError": true
                    }),
                ),
            }
        }
        _ => JsonRpcResponse::success(
            id,
            serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": format!("Unknown tool: {tool_name}")
                }],
                "isError": true
            }),
        ),
    }
}

fn handle_resources_list(id: serde_json::Value) -> JsonRpcResponse {
    JsonRpcResponse::success(
        id,
        serde_json::json!({
            "resources": [{
                "uri": "kite://events/recent",
                "name": "Recent Webhook Events",
                "description": "List of recent webhook events received by Kite.",
                "mimeType": "application/json"
            }]
        }),
    )
}

async fn handle_resources_read(
    id: serde_json::Value,
    params: serde_json::Value,
    buffer: &Arc<Mutex<EventBuffer>>,
) -> JsonRpcResponse {
    let uri = params.get("uri").and_then(|v| v.as_str()).unwrap_or("");

    if uri == "kite://events/recent" {
        let buf = buffer.lock().await;
        let events = buf.list(None, None, 50);
        let summaries: Vec<serde_json::Value> = events
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "source": e.source,
                    "type": e.event_type,
                    "time": e.time,
                    "summary": e.summary,
                    "received_at": e.received_at,
                })
            })
            .collect();

        JsonRpcResponse::success(
            id,
            serde_json::json!({
                "contents": [{
                    "uri": "kite://events/recent",
                    "mimeType": "application/json",
                    "text": serde_json::to_string_pretty(&summaries).unwrap_or_default()
                }]
            }),
        )
    } else {
        JsonRpcResponse::error(id, -32602, format!("Unknown resource: {uri}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cloudevents::{EventBuilder, EventBuilderV10};

    fn sample_event() -> Event {
        let payload = serde_json::json!({"action": "opened", "number": 42});
        EventBuilderV10::new()
            .id("evt_test_001")
            .ty("com.github.pull_request.opened")
            .source("https://getkite.sh/hooks/team_abc/github")
            .data("application/json", payload)
            .build()
            .unwrap()
    }

    #[test]
    fn buffer_push_and_list() {
        let mut buf = EventBuffer::new(3);
        for i in 0..5 {
            let event = sample_event();
            let mut buffered = BufferedEvent::from_cloudevent(&event);
            buffered.id = format!("evt_{i}");
            buf.push(buffered);
        }
        // Capacity is 3, so only last 3 events remain
        assert_eq!(buf.events.len(), 3);
        let listed = buf.list(None, None, 10);
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].id, "evt_4"); // Most recent first
    }

    #[test]
    fn buffer_filter_by_source() {
        let mut buf = EventBuffer::new(10);
        let event = sample_event();
        let mut e1 = BufferedEvent::from_cloudevent(&event);
        e1.id = "evt_gh_1".to_string();
        e1.source = "https://getkite.sh/hooks/team_abc/github".to_string();
        e1.event_type = "com.github.push".to_string();
        buf.push(e1);

        let mut e2 = BufferedEvent::from_cloudevent(&event);
        e2.id = "evt_stripe_1".to_string();
        e2.source = "https://getkite.sh/hooks/team_abc/stripe".to_string();
        e2.event_type = "com.stripe.payment_intent.succeeded".to_string();
        buf.push(e2);

        let github = buf.list(Some("github"), None, 10);
        assert_eq!(github.len(), 1);
        assert_eq!(github[0].id, "evt_gh_1");

        let stripe = buf.list(None, Some("stripe"), 10);
        assert_eq!(stripe.len(), 1);
        assert_eq!(stripe[0].id, "evt_stripe_1");
    }

    #[test]
    fn buffer_get_by_id() {
        let mut buf = EventBuffer::new(10);
        let event = sample_event();
        let buffered = BufferedEvent::from_cloudevent(&event);
        buf.push(buffered);

        assert!(buf.get("evt_test_001").is_some());
        assert!(buf.get("nonexistent").is_none());
    }

    #[test]
    fn initialize_response_has_capabilities() {
        let resp = handle_initialize(serde_json::json!(1));
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["capabilities"]["tools"].is_object());
        assert!(result["capabilities"]["resources"].is_object());
    }

    #[test]
    fn tools_list_returns_both_tools() {
        let resp = handle_tools_list(serde_json::json!(1));
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["name"], "list_events");
        assert_eq!(tools[1]["name"], "get_event");
    }

    #[test]
    fn resources_list_has_recent_events() {
        let resp = handle_resources_list(serde_json::json!(1));
        let result = resp.result.unwrap();
        let resources = result["resources"].as_array().unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0]["uri"], "kite://events/recent");
    }

    #[test]
    fn buffer_respects_max_limit() {
        let mut buf = EventBuffer::new(100);
        for i in 0..20 {
            let event = sample_event();
            let mut buffered = BufferedEvent::from_cloudevent(&event);
            buffered.id = format!("evt_{i}");
            buf.push(buffered);
        }
        let limited = buf.list(None, None, 5);
        assert_eq!(limited.len(), 5);
    }

    #[test]
    fn buffer_filter_by_event_type() {
        let mut buf = EventBuffer::new(10);
        let event = sample_event();

        let mut e1 = BufferedEvent::from_cloudevent(&event);
        e1.id = "evt_1".to_string();
        e1.event_type = "com.github.push".to_string();
        buf.push(e1);

        let mut e2 = BufferedEvent::from_cloudevent(&event);
        e2.id = "evt_2".to_string();
        e2.event_type = "com.github.pull_request.opened".to_string();
        buf.push(e2);

        let push_events = buf.list(Some("push"), None, 10);
        assert_eq!(push_events.len(), 1);
        assert_eq!(push_events[0].id, "evt_1");
    }

    #[test]
    fn buffer_empty_returns_empty_list() {
        let buf = EventBuffer::new(10);
        let events = buf.list(None, None, 50);
        assert!(events.is_empty());
    }

    #[test]
    fn buffer_get_nonexistent_returns_none() {
        let buf = EventBuffer::new(10);
        assert!(buf.get("does-not-exist").is_none());
    }

    #[test]
    fn buffered_event_from_cloudevent_captures_fields() {
        let event = sample_event();
        let buffered = BufferedEvent::from_cloudevent(&event);
        assert_eq!(buffered.id, "evt_test_001");
        assert_eq!(buffered.event_type, "com.github.pull_request.opened");
        assert!(buffered.source.contains("getkite.sh"));
    }

    #[test]
    fn jsonrpc_response_success_structure() {
        let resp =
            JsonRpcResponse::success(serde_json::json!(42), serde_json::json!({"key": "value"}));
        assert_eq!(resp.id, serde_json::json!(42));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn jsonrpc_response_error_structure() {
        let resp =
            JsonRpcResponse::error(serde_json::json!(1), -32600, "Invalid Request".to_string());
        assert_eq!(resp.id, serde_json::json!(1));
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "Invalid Request");
    }
}
