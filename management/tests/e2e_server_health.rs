mod e2e_support;

use serde_json::json;
use tokio::time::{sleep, Duration};

use e2e_support::{require_rust_e2e, websocket_round_trip, ManagementServer, WsTestClient};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_e2e_http_health_endpoint() -> anyhow::Result<()> {
    if !require_rust_e2e() {
        return Ok(());
    }

    let server = ManagementServer::start()?;
    let response = reqwest::get(server.http_url("/health")).await?;

    assert!(response.status().is_success());
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["status"], "ok");
    assert_eq!(body["service"], "agentic-management");

    let response = reqwest::get(server.http_url("/api/v1/health")).await?;
    assert!(response.status().is_success());
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["status"], "ok");
    assert!(
        body.get("version").is_some(),
        "unexpected health body: {body}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_e2e_websocket_connects_and_stays_open() -> anyhow::Result<()> {
    if !require_rust_e2e() {
        return Ok(());
    }

    let server = ManagementServer::start()?;
    let _client = WsTestClient::connect(&server.ws_url()).await?;

    sleep(Duration::from_millis(250)).await;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_e2e_websocket_ping_pong() -> anyhow::Result<()> {
    if !require_rust_e2e() {
        return Ok(());
    }

    let server = ManagementServer::start()?;
    let timestamp = chrono::Utc::now().timestamp_millis();
    let frame = websocket_round_trip(
        &server.ws_url(),
        json!({
            "type": "ping",
            "timestamp": timestamp,
        }),
        "pong",
    )
    .await?;

    assert_eq!(frame["type"], "pong");
    assert!(
        frame.get("timestamp").is_some(),
        "unexpected pong frame: {frame}"
    );

    Ok(())
}
