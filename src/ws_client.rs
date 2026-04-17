use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use kite_protocol::ws_messages::{ClientMessage, ServerMessage};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

/// Connect to the Kite server WebSocket and perform the handshake.
/// Returns a split (sink, stream) for sending/receiving messages, plus last_seq and assigned client_id.
pub async fn connect(
    url: &str,
    token: &str,
    team_id: &str,
    scopes: Vec<String>,
    client_id: Option<String>,
) -> Result<(
    impl futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>,
    impl futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>,
    u64,
    String,
)> {
    let (ws_stream, _) = connect_async(url).await?;
    let (mut sink, mut stream) = ws_stream.split();

    // Send Connect handshake
    let connect_msg = ClientMessage::Connect {
        version: 1,
        token: token.to_string(),
        team_id: team_id.to_string(),
        scopes,
        client_id,
    };
    let json = serde_json::to_string(&connect_msg)?;
    sink.send(Message::Text(json.into())).await?;

    // Wait for Connected response (ignore unknown/non-critical frames)
    let (last_seq, client_id) = loop {
        match stream.next().await {
            Some(Ok(Message::Text(text))) => {
                if let Ok(msg) = serde_json::from_str::<ServerMessage>(&text) {
                    match msg {
                        ServerMessage::Connected {
                            last_seq,
                            client_id,
                        } => break (last_seq, client_id),
                        ServerMessage::Error { code, message } => {
                            anyhow::bail!("Server error ({code}): {message}");
                        }
                        _ => {
                            // Ignore non-handshake frames (e.g. snapshots/notifications)
                        }
                    }
                } else {
                    // Ignore unknown server frame variants for forward compatibility
                }
            }
            Some(Ok(Message::Ping(_))) => {}
            Some(Ok(Message::Close(_))) => anyhow::bail!("Connection closed during handshake"),
            Some(Ok(_)) => {}
            Some(Err(e)) => anyhow::bail!("WebSocket error during handshake: {e}"),
            None => anyhow::bail!("Connection closed during handshake"),
        }
    };

    Ok((sink, stream, last_seq, client_id))
}

/// Send an Ack message for the given sequence number.
#[allow(dead_code)]
pub async fn send_ack<S>(sink: &mut S, seq: u64) -> Result<()>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let ack = ClientMessage::Ack { seq };
    let json = serde_json::to_string(&ack)?;
    sink.send(Message::Text(json.into())).await?;
    Ok(())
}

/// Event loop that receives ServerMessages from the WebSocket and calls a handler for each event.
pub async fn event_loop<F, Fut>(
    mut stream: impl futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
    + Unpin,
    mut handler: F,
) -> Result<()>
where
    F: FnMut(u64, cloudevents::Event) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                match serde_json::from_str::<ServerMessage>(&text) {
                    Ok(ServerMessage::Event { seq, event }) => {
                        // Deserialize the CloudEvent from the JSON value
                        match serde_json::from_value::<cloudevents::Event>(event) {
                            Ok(ce) => {
                                if let Err(e) = handler(seq, ce).await {
                                    tracing::error!("Handler error: {e}");
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Failed to parse CloudEvent: {e}");
                            }
                        }
                    }
                    Ok(ServerMessage::Error { code, message }) => {
                        tracing::error!("Server error ({code}): {message}");
                        if code == kite_protocol::ws_messages::ERR_SLOW_CONSUMER {
                            anyhow::bail!("Disconnected: slow consumer");
                        }
                    }
                    Ok(_) => {} // Ignore other messages
                    Err(e) => {
                        tracing::warn!("Failed to parse server message: {e}");
                    }
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(_)) => {} // tungstenite handles pong automatically
            Err(e) => {
                tracing::error!("WebSocket error: {e}");
                break;
            }
            _ => {}
        }
    }

    Ok(())
}

/// Open a short-lived WS session, perform an RPC request, and return the payload.
pub async fn rpc_request(
    url: &str,
    token: &str,
    team_id: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let (ws_stream, _) = connect_async(url).await?;
    let (mut sink, mut stream) = ws_stream.split();

    let connect_msg = ClientMessage::Connect {
        version: 1,
        token: token.to_string(),
        team_id: team_id.to_string(),
        scopes: vec!["*".to_string()],
        client_id: None,
    };
    sink.send(Message::Text(serde_json::to_string(&connect_msg)?.into()))
        .await?;

    // Wait for handshake completion; ignore unknown/non-critical frames for compatibility.
    loop {
        match stream.next().await {
            Some(Ok(Message::Text(text))) => {
                if let Ok(msg) = serde_json::from_str::<ServerMessage>(&text) {
                    match msg {
                        ServerMessage::Connected { .. } => break,
                        ServerMessage::Error { code, message } => {
                            anyhow::bail!("Server error ({code}): {message}");
                        }
                        _ => {}
                    }
                } else {
                    // Ignore unknown frame variants.
                }
            }
            Some(Err(e)) => anyhow::bail!("WebSocket error during handshake: {e}"),
            Some(Ok(Message::Close(_))) => anyhow::bail!("Connection closed during handshake"),
            Some(Ok(_)) => {}
            None => anyhow::bail!("Connection closed during handshake"),
        }
    }

    let request_id = Uuid::now_v7().to_string();
    let request = ClientMessage::Request {
        id: request_id.clone(),
        method: method.to_string(),
        params,
    };
    sink.send(Message::Text(serde_json::to_string(&request)?.into()))
        .await?;

    while let Some(msg) = stream.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                if let Ok(server_msg) = serde_json::from_str::<ServerMessage>(&text) {
                    match server_msg {
                        ServerMessage::Response { id, ok, payload } => {
                            if id != request_id {
                                continue;
                            }
                            if ok {
                                return Ok(payload);
                            }
                            let error = payload
                                .get("error")
                                .and_then(|v| v.as_str())
                                .unwrap_or("request failed");
                            anyhow::bail!("{error}");
                        }
                        ServerMessage::Error { code, message } => {
                            anyhow::bail!("Server error ({code}): {message}");
                        }
                        _ => {}
                    }
                } else {
                    // Ignore unknown frame variants and keep waiting for our response.
                    continue;
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(_)) => {}
            Ok(_) => {}
            Err(e) => anyhow::bail!("WebSocket error while waiting for response: {e}"),
        }
    }

    anyhow::bail!("Connection closed before RPC response")
}
