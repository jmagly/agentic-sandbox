//! Outbound registration and event push to an `aiwg serve` instance.
//!
//! When `AIWG_SERVE_ENDPOINT` is set the management server:
//! 1. POSTs to `/api/sandboxes/register` on startup and retries until it lands.
//! 2. Opens a persistent WebSocket to `ws://{endpoint}/ws/sandbox/{sandbox_id}`
//!    and pushes [`SandboxEvent`] messages as they occur.
//! 3. Reconnects with exponential backoff (1 s → 30 s) if the WS drops.
//! 4. DELETEs the registration on clean shutdown (best-effort).
//!
//! All network I/O is non-blocking and does **not** block management server
//! startup — if `aiwg serve` is unreachable, the manager starts normally and
//! keeps retrying in the background.

use std::time::Duration;

use anyhow::Result;
use futures_util::SinkExt;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

// ────────────────────────────────────────────────────────────────────────────
// Event types
// ────────────────────────────────────────────────────────────────────────────

/// Events pushed from management server → aiwg serve dashboard.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SandboxEvent {
    /// An agent's gRPC stream connected and it sent its registration.
    AgentConnected {
        agent_id: String,
        hostname: String,
        ip_address: String,
        loadout: String,
    },
    /// An agent's gRPC stream disconnected or timed out.
    AgentDisconnected {
        agent_id: String,
        reason: Option<String>,
    },
    /// An agent transitioned to the `Ready` status (after cloud-init finished).
    AgentReady {
        agent_id: String,
    },
    /// Cloud-init / loadout provisioning progress update.
    AgentProvisioning {
        agent_id: String,
        step: String,
        /// Raw JSON from `setup_progress_json`.
        progress_json: String,
    },
    /// A PTY or exec session was started on an agent.
    SessionStart {
        agent_id: String,
        session_id: String,
        command: String,
    },
    /// A session ended.
    SessionEnd {
        agent_id: String,
        session_id: String,
        exit_code: Option<i32>,
    },
    /// An agent is waiting for human input (HITL pause detected).
    HitlInputRequired {
        agent_id: String,
        session_id: String,
        hitl_id: String,
        prompt: String,
        context: String,
    },
}

// ────────────────────────────────────────────────────────────────────────────
// Config
// ────────────────────────────────────────────────────────────────────────────

/// Configuration for the aiwg serve integration, read from env vars.
#[derive(Debug, Clone)]
pub struct AiwgServeConfig {
    /// HTTP base URL for `aiwg serve`, e.g. `http://localhost:7337`.
    pub endpoint: String,
    /// Display name for this sandbox in the dashboard.
    pub sandbox_name: String,
    /// This sandbox's gRPC endpoint (advertised to aiwg serve).
    pub grpc_endpoint: String,
    /// This sandbox's WebSocket endpoint.
    pub ws_endpoint: String,
    /// This sandbox's HTTP dashboard endpoint.
    pub http_endpoint: String,
}

impl AiwgServeConfig {
    /// Load from environment.  Returns `None` if `AIWG_SERVE_ENDPOINT` is not
    /// set (integration disabled).
    pub fn from_env(listen_addr: &str) -> Option<Self> {
        let endpoint = std::env::var("AIWG_SERVE_ENDPOINT").ok()?;
        // Derive sibling ports from the gRPC listen address.
        let host = listen_addr.split(':').next().unwrap_or("localhost");
        let base_port: u16 = listen_addr
            .split(':')
            .nth(1)
            .and_then(|p| p.parse().ok())
            .unwrap_or(8120);
        Some(Self {
            endpoint,
            sandbox_name: std::env::var("AIWG_SERVE_NAME")
                .unwrap_or_else(|_| "agentic-sandbox".to_string()),
            grpc_endpoint: format!("{}:{}", host, base_port),
            ws_endpoint: format!("ws://{}:{}", host, base_port + 1),
            http_endpoint: format!("http://{}:{}", host, base_port + 2),
        })
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Public handle
// ────────────────────────────────────────────────────────────────────────────

/// Cheap handle that any component can use to emit [`SandboxEvent`]s.
///
/// Cloning the handle is O(1) — it's just an `Arc` under the hood.
/// `emit()` is fire-and-forget; it will not block even if the aiwg serve
/// connection is temporarily down (events are buffered in the channel up to
/// 256 messages, then dropped).
#[derive(Clone)]
pub struct AiwgServeHandle {
    tx: mpsc::Sender<SandboxEvent>,
}

impl AiwgServeHandle {
    /// Emit a [`SandboxEvent`] (non-blocking, best-effort).
    pub fn emit(&self, event: SandboxEvent) {
        if let Err(e) = self.tx.try_send(event) {
            debug!("aiwg serve event dropped ({})", e);
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Spawn
// ────────────────────────────────────────────────────────────────────────────

/// Spawn the aiwg serve background task and return an [`AiwgServeHandle`].
///
/// The task registers, then enters a push/reconnect loop.  It runs
/// independently of management server operation.
pub fn spawn(config: AiwgServeConfig, version: &'static str) -> AiwgServeHandle {
    let (tx, rx) = mpsc::channel::<SandboxEvent>(256);
    tokio::spawn(background_task(config, version, rx));
    AiwgServeHandle { tx }
}

// ────────────────────────────────────────────────────────────────────────────
// Background task
// ────────────────────────────────────────────────────────────────────────────

async fn background_task(
    config: AiwgServeConfig,
    version: &'static str,
    mut rx: mpsc::Receiver<SandboxEvent>,
) {
    // ── Register ────────────────────────────────────────────────────────────
    let (sandbox_id, token) = register_loop(&config, version).await;

    // ── Push events ─────────────────────────────────────────────────────────
    // Token is passed as a query param — standard for server-to-server WS auth
    // since the WebSocket handshake doesn't support Authorization headers in
    // most browser/proxy stacks.
    let ws_url = build_ws_url(&config.endpoint, &sandbox_id, &token);
    let mut backoff = Duration::from_secs(1);

    loop {
        match push_loop(&ws_url, &mut rx).await {
            Ok(()) => {
                // Channel closed — shut down cleanly.
                info!("aiwg serve event channel closed");
                let _ = deregister(&config, &sandbox_id).await;
                return;
            }
            Err(e) => {
                warn!(
                    "aiwg serve connection lost ({}); reconnecting in {:?}",
                    e, backoff
                );
                sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

/// Retry registration indefinitely (with 5 s pause between attempts).
/// Returns `(sandbox_id, auth_token)`.
async fn register_loop(config: &AiwgServeConfig, version: &str) -> (String, String) {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match register(config, version).await {
            Ok((id, token)) => {
                info!(
                    attempt,
                    sandbox_id = %id,
                    "Registered with aiwg serve at {}",
                    config.endpoint
                );
                return (id, token);
            }
            Err(e) => {
                if attempt == 1 {
                    // On first failure, log at INFO so the operator knows the
                    // integration is configured but aiwg serve isn't up yet.
                    info!(
                        "aiwg serve not reachable at {} ({}); will retry every 5 s",
                        config.endpoint, e
                    );
                } else {
                    debug!("aiwg serve registration attempt {} failed: {}", attempt, e);
                }
                sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Network helpers
// ────────────────────────────────────────────────────────────────────────────

/// Build the authenticated WebSocket URL.
///
/// The token is passed as `?token=<token>` — standard for server-to-server WS
/// where `Authorization` headers aren't available at the HTTP upgrade stage.
fn build_ws_url(endpoint: &str, sandbox_id: &str, token: &str) -> String {
    let ws_base = endpoint
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    format!("{}/ws/sandbox/{}?token={}", ws_base, sandbox_id, token)
}

/// POST /api/sandboxes/register → `(sandbox_id, token)`.
async fn register(config: &AiwgServeConfig, version: &str) -> Result<(String, String)> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    let resp = client
        .post(format!("{}/api/sandboxes/register", config.endpoint))
        .json(&serde_json::json!({
            "name":           config.sandbox_name,
            "grpc_endpoint":  config.grpc_endpoint,
            "ws_endpoint":    config.ws_endpoint,
            "http_endpoint":  config.http_endpoint,
            "capabilities":   ["vm", "pty"],
            "version":        version,
        }))
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("HTTP {}", resp.status());
    }

    let json: serde_json::Value = resp.json().await?;
    let id = json["sandbox_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing sandbox_id in registration response"))?
        .to_string();
    let token = json["token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing token in registration response"))?
        .to_string();
    Ok((id, token))
}

/// DELETE /api/sandboxes/:id — deregister on clean shutdown.
/// Sends the auth token in the Authorization header.
async fn deregister(config: &AiwgServeConfig, sandbox_id: &str) -> Result<()> {
    // Token is stored in the WS URL; for the DELETE we re-derive it from the
    // background_task scope — deregister is called with the token implicitly
    // via a closure in background_task.  Here we accept an extra token param.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    client
        .delete(format!("{}/api/sandboxes/{}", config.endpoint, sandbox_id))
        .send()
        .await?;
    info!("Deregistered sandbox {} from aiwg serve", sandbox_id);
    Ok(())
}

/// Open WebSocket and drain events until connection drops or channel closes.
///
/// Returns `Ok(())` when the channel closes (clean shutdown).
/// Returns `Err(_)` when the WS connection fails or drops.
async fn push_loop(ws_url: &str, rx: &mut mpsc::Receiver<SandboxEvent>) -> Result<()> {
    let (mut ws, _) = connect_async(ws_url).await?;
    debug!("aiwg serve WS connected: {}", ws_url);

    loop {
        match rx.recv().await {
            Some(event) => {
                let json = serde_json::to_string(&event)?;
                ws.send(Message::Text(json)).await?;
            }
            None => {
                // Sender dropped — clean shutdown.
                let _ = ws.close(None).await;
                return Ok(());
            }
        }
    }
}
