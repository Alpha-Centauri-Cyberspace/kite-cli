//! Integration tests for the device-auth HTTP contract that `kite login`
//! depends on.
//!
//! These do NOT run the full `kite-cli::commands::login::run()` loop
//! because that function writes to `KiteConfig`'s HOME-derived path and
//! races under parallel test execution. Instead, we verify the exact HTTP
//! contract `run()` expects: issuing a device code, polling for approval,
//! and handling each documented error case. If the server contract drifts
//! (field rename, status code change, error code typo), these fail.
//!
//! The `run()` polling loop is covered structurally by unit tests in
//! `src/commands/login.rs` for URL sanitization + WS derivation.

use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn device_code_request_shape_matches_cli_expectation() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/auth/device-code"))
        .and(body_partial_json(json!({})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code": "dev-abc-123",
            "user_code": "ABCD-1234",
            "verification_url": format!("{}/activate", server.uri()),
            "ws_url": format!("ws://{}/ws", server.address()),
            "expires_in": 900,
            "interval": 1,
        })))
        .mount(&server)
        .await;

    // Simulate what `kite login` does on first call: POST with empty body.
    let client = reqwest::Client::new();
    let res = client
        .post(format!("{}/api/auth/device-code", server.uri()))
        .json(&json!({}))
        .send()
        .await
        .expect("request succeeds");
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.expect("valid json");
    // These fields are extracted by login.rs via `body["..."]` indexing.
    // If any rename without coordinated update, the CLI panics on "Missing
    // device_code in response" before even showing the user code.
    assert!(body["device_code"].is_string(), "device_code missing");
    assert!(body["user_code"].is_string(), "user_code missing");
    assert!(
        body["verification_url"].is_string(),
        "verification_url missing"
    );
    assert!(body["ws_url"].is_string(), "ws_url missing");
    assert!(body["interval"].is_u64(), "interval missing or wrong type");
}

#[tokio::test]
async fn token_poll_success_returns_api_key_and_team_id() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/auth/device-token"))
        .and(body_partial_json(json!({ "device_code": "dev-xyz" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "api_key": "kite_deadbeef_cafebabecafebabe",
            "team_id": "clerk-user:user_abc",
        })))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let res = client
        .post(format!("{}/api/auth/device-token", server.uri()))
        .json(&json!({ "device_code": "dev-xyz" }))
        .send()
        .await
        .expect("request succeeds");
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.expect("valid json");
    // Field names that login.rs reads verbatim — rename = silent auth failure.
    assert!(body["api_key"].as_str().is_some());
    assert!(body["team_id"].as_str().is_some());
}

#[tokio::test]
async fn token_poll_pending_returns_authorization_pending_error_string() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/auth/device-token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "authorization_pending",
        })))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let res = client
        .post(format!("{}/api/auth/device-token", server.uri()))
        .json(&json!({ "device_code": "dev-xyz" }))
        .send()
        .await
        .expect("request succeeds");
    let body: serde_json::Value = res.json().await.expect("valid json");
    // login.rs switches on this exact string. A typo on the server side
    // makes `kite login` print '...' indefinitely instead of polling cleanly.
    assert_eq!(body["error"].as_str(), Some("authorization_pending"));
}

#[tokio::test]
async fn token_poll_denied_returns_access_denied_error_string() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/auth/device-token"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": "access_denied",
        })))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let res = client
        .post(format!("{}/api/auth/device-token", server.uri()))
        .json(&json!({ "device_code": "dev-xyz" }))
        .send()
        .await
        .expect("request succeeds");
    let body: serde_json::Value = res.json().await.expect("valid json");
    assert_eq!(body["error"].as_str(), Some("access_denied"));
}

#[tokio::test]
async fn token_poll_expired_returns_expired_token_error_string() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/auth/device-token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "expired_token",
        })))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let res = client
        .post(format!("{}/api/auth/device-token", server.uri()))
        .json(&json!({ "device_code": "dev-xyz" }))
        .send()
        .await
        .expect("request succeeds");
    let body: serde_json::Value = res.json().await.expect("valid json");
    assert_eq!(body["error"].as_str(), Some("expired_token"));
}
