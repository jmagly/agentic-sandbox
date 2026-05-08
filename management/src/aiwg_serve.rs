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

use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::{mpsc, Notify};
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

// ────────────────────────────────────────────────────────────────────────────
// Event types
// ────────────────────────────────────────────────────────────────────────────

/// One session entry in `AgentSessions`. Mirrors the REST shape returned
/// by `GET /api/v1/agents/{id}/sessions` so consumers can use the same
/// type for both push and pull paths.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub session_name: String,
    /// "interactive" | "headless" | "background"
    pub session_type: String,
    pub command: String,
    pub created_at_secs: u64,
    pub has_screen: bool,
}

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
        /// Stable per-agent UUIDv7 for persistent identity tracking (#917).
        /// Always present — absent only when receiving events from an old sandbox build.
        agent_instance_id: Option<String>,
    },
    /// An agent's gRPC stream disconnected or timed out.
    AgentDisconnected {
        agent_id: String,
        reason: Option<String>,
    },
    /// An agent transitioned to the `Ready` status (after cloud-init finished).
    AgentReady { agent_id: String },
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
    /// Authoritative snapshot of an agent's current session inventory (#192).
    /// Emitted after AgentConnected (initial sync, may be empty), and after
    /// every SessionStart / SessionEnd on the affected agent. AIWG should
    /// replace its per-agent cache with this list — it's authoritative,
    /// not a delta.
    AgentSessions {
        agent_id: String,
        sessions: Vec<SessionSummary>,
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
    /// Stable instance identity (UUID persisted across restarts).
    pub instance_id: String,
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
    pub fn from_env(listen_addr: &str, instance_id: String) -> Option<Self> {
        let endpoint = std::env::var("AIWG_SERVE_ENDPOINT").ok()?;
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
            instance_id,
            grpc_endpoint: format!("{}:{}", host, base_port),
            ws_endpoint: format!("ws://{}:{}", host, base_port + 1),
            http_endpoint: format!("http://{}:{}", host, base_port + 2),
        })
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Public handle
// ────────────────────────────────────────────────────────────────────────────

/// Observable connection state — updated by the background task.
#[derive(Debug, Clone, Serialize)]
pub struct AiwgConnState {
    pub configured: bool,
    pub connected: bool,
    pub endpoint: String,
    pub sandbox_id: Option<String>,
    /// Executor registration result (#193). `None` until the first
    /// registration attempt completes; `Some(Ok(executor_id))` if the
    /// executor-contract route is available on the AIWG side, or
    /// `Some(Err(reason))` if the route returned 404 / unavailable.
    /// Sandbox registration is independent and continues regardless.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executor_register_error: Option<String>,
}

/// Cheap handle that any component can use to emit [`SandboxEvent`]s.
///
/// Cloning the handle is O(1) — it's just an `Arc` under the hood.
/// `emit()` is fire-and-forget; it will not block even if the aiwg serve
/// connection is temporarily down (events are buffered in the channel up to
/// 256 messages, then dropped).
#[derive(Clone)]
pub struct AiwgServeHandle {
    tx: mpsc::Sender<SandboxEvent>,
    state: Arc<RwLock<AiwgConnState>>,
    reconnect: Arc<Notify>,
}

impl AiwgServeHandle {
    /// Emit a [`SandboxEvent`] (non-blocking, best-effort).
    pub fn emit(&self, event: SandboxEvent) {
        if let Err(e) = self.tx.try_send(event) {
            debug!("aiwg serve event dropped ({})", e);
        }
    }

    /// Current connection state snapshot.
    pub fn conn_state(&self) -> AiwgConnState {
        self.state.read().unwrap().clone()
    }

    /// Signal the background task to reconnect immediately (skips backoff sleep).
    pub fn trigger_reconnect(&self) {
        self.reconnect.notify_one();
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
    let state = Arc::new(RwLock::new(AiwgConnState {
        configured: true,
        connected: false,
        endpoint: config.endpoint.clone(),
        sandbox_id: None,
        executor_id: None,
        executor_register_error: None,
    }));
    let reconnect = Arc::new(Notify::new());
    tokio::spawn(background_task(
        config,
        version,
        rx,
        state.clone(),
        reconnect.clone(),
    ));
    AiwgServeHandle {
        tx,
        state,
        reconnect,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Background task
// ────────────────────────────────────────────────────────────────────────────

async fn background_task(
    config: AiwgServeConfig,
    version: &'static str,
    mut rx: mpsc::Receiver<SandboxEvent>,
    state: Arc<RwLock<AiwgConnState>>,
    reconnect: Arc<Notify>,
) {
    // Single shared client — creating a new reqwest::Client per request spawns
    // Hyper background tasks (eventfd wakers) and causes FD leaks under retry loops.
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("reqwest client build failed");

    let mut backoff = Duration::from_secs(1);

    loop {
        // ── Register ─────────────────────────────────────────────────────────
        let (sandbox_id, token) = register_loop(&config, version, &http_client).await;
        backoff = Duration::from_secs(1);
        {
            let mut s = state.write().unwrap();
            s.sandbox_id = Some(sandbox_id.clone());
        }

        // ── Register as executor (#193, AIWG executor.v1.md) ─────────────────
        // Best-effort: this route is added by AIWG #1179. Until that lands
        // we'll get 404 / connection-refused — log a warning and proceed.
        // Reuses the sandbox instance_id as the executor_id so dashboard
        // correlation works (one identity, two registrations).
        match register_executor(&config, version, &http_client).await {
            Ok(executor_id) => {
                info!(
                    executor_id = %executor_id,
                    "Registered as executor with aiwg serve"
                );
                let mut s = state.write().unwrap();
                s.executor_id = Some(executor_id);
                s.executor_register_error = None;
            }
            Err(e) => {
                let msg = e.to_string();
                warn!("Executor registration unavailable ({msg}); sandbox registration will continue. This is expected until AIWG #1179 lands.");
                let mut s = state.write().unwrap();
                s.executor_id = None;
                s.executor_register_error = Some(msg);
            }
        }

        // ── Push events ──────────────────────────────────────────────────────
        let ws_url = build_ws_url(&config.endpoint, &sandbox_id, &token);

        match push_loop(&ws_url, &mut rx, &state, &reconnect).await {
            Ok(()) => {
                info!("aiwg serve event channel closed");
                let _ = deregister(&config, &sandbox_id, &http_client).await;
                state.write().unwrap().connected = false;
                return;
            }
            Err(e) => {
                state.write().unwrap().connected = false;
                warn!(
                    "aiwg serve WS lost ({}); re-registering in {:?}",
                    e, backoff
                );
                let _ = deregister(&config, &sandbox_id, &http_client).await;
                // Sleep with backoff, but wake immediately if reconnect is triggered.
                tokio::select! {
                    _ = sleep(backoff) => {}
                    _ = reconnect.notified() => {
                        info!("aiwg serve reconnect triggered manually");
                    }
                }
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

/// Retry registration indefinitely (with 5 s pause between attempts).
/// Returns `(sandbox_id, auth_token)`.
async fn register_loop(
    config: &AiwgServeConfig,
    version: &str,
    client: &reqwest::Client,
) -> (String, String) {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match register(config, version, client).await {
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
async fn register(
    config: &AiwgServeConfig,
    version: &str,
    client: &reqwest::Client,
) -> Result<(String, String)> {
    let resp = client
        .post(format!("{}/api/sandboxes/register", config.endpoint))
        .json(&serde_json::json!({
            "name":           config.sandbox_name,
            "instance_id":    config.instance_id,
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

/// POST /api/v1/executors/register — register this sandbox as a mission
/// executor per AIWG `executor.v1.md` (#193). One-shot: returns the
/// executor_id (which we set equal to the sandbox instance_id for
/// dashboard correlation) or an error if the route is unavailable.
///
/// Capabilities are static for now and reflect the agentic-sandbox
/// runtime: KVM VMs and Docker containers, claude-code agent runtime,
/// linux/x64 host, resumable across mgmt-server restarts (mission state
/// persists in dispatcher.rs), HITL pause/resume.
async fn register_executor(
    config: &AiwgServeConfig,
    version: &str,
    client: &reqwest::Client,
) -> Result<String> {
    let payload = serde_json::json!({
        "executor_id":   config.instance_id,
        "name":          format!("agentic-sandbox-{}", config.sandbox_name),
        "version":       version,
        "spec_version":  "1.0.0",
        "transport_endpoints": {
            "rest": config.http_endpoint,
            "ws":   config.ws_endpoint,
        },
        "capabilities": [
            "isolation:vm",
            "isolation:container",
            "runtime:claude-code",
            "platform:linux/x64",
            "resumable",
            "hitl",
        ],
    });

    let resp = client
        .post(format!("{}/api/v1/executors/register", config.endpoint))
        .json(&payload)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("HTTP {}", status);
    }

    let json: serde_json::Value = resp.json().await?;
    let id = json["executor_id"]
        .as_str()
        .unwrap_or(&config.instance_id)
        .to_string();
    Ok(id)
}

/// DELETE /api/sandboxes/:id — deregister on clean shutdown.
async fn deregister(
    config: &AiwgServeConfig,
    sandbox_id: &str,
    client: &reqwest::Client,
) -> Result<()> {
    client
        .delete(format!("{}/api/sandboxes/{}", config.endpoint, sandbox_id))
        .send()
        .await?;
    info!("Deregistered sandbox {} from aiwg serve", sandbox_id);
    Ok(())
}

const PING_INTERVAL: Duration = Duration::from_secs(20);
const PONG_TIMEOUT: Duration = Duration::from_secs(10);

/// Open WebSocket and drain events until connection drops, channel closes, or
/// a manual reconnect is requested.
///
/// Returns `Ok(())` when the event channel closes (clean shutdown).
/// Returns `Err(_)` when the WS connection fails, the server closes the
/// connection, a ping times out, or `reconnect` is signalled.
async fn push_loop(
    ws_url: &str,
    rx: &mut mpsc::Receiver<SandboxEvent>,
    state: &Arc<RwLock<AiwgConnState>>,
    reconnect: &Arc<Notify>,
) -> Result<()> {
    let (ws, _) = connect_async(ws_url).await?;
    state.write().unwrap().connected = true;
    info!("aiwg serve WS connected: {}", ws_url);

    let (mut sink, mut stream) = ws.split();

    let mut ping_ticker = tokio::time::interval(PING_INTERVAL);
    ping_ticker.tick().await; // consume immediate first tick
    let mut waiting_for_pong = false;

    loop {
        tokio::select! {
            // ── Outbound events ───────────────────────────────────────────
            event = rx.recv() => {
                match event {
                    Some(ev) => {
                        let json = serde_json::to_string(&ev)?;
                        sink.send(Message::Text(json)).await?;
                    }
                    None => {
                        // Sender dropped — clean shutdown.
                        let _ = sink.close().await;
                        return Ok(());
                    }
                }
            }

            // ── Inbound frames ────────────────────────────────────────────
            // Reading continuously means we detect server-side Close frames
            // immediately rather than waiting up to PING_INTERVAL for a
            // write to fail.
            frame = stream.next() => {
                match frame {
                    Some(Ok(Message::Pong(_))) => {
                        debug!("aiwg serve pong received");
                        waiting_for_pong = false;
                    }
                    Some(Ok(Message::Close(frame))) => {
                        info!("aiwg serve closed WS: {:?}", frame);
                        anyhow::bail!("server closed connection");
                    }
                    Some(Ok(_)) => {} // ping / text echo — ignore
                    Some(Err(e)) => {
                        warn!("aiwg serve WS read error: {}", e);
                        return Err(anyhow::anyhow!(e));
                    }
                    None => {
                        anyhow::bail!("aiwg serve WS stream ended");
                    }
                }
            }

            // ── Periodic keepalive ────────────────────────────────────────
            _ = ping_ticker.tick() => {
                if waiting_for_pong {
                    anyhow::bail!("pong timeout — aiwg serve connection silently dead");
                }
                sink.send(Message::Ping(vec![])).await?;
                waiting_for_pong = true;
                debug!("aiwg serve ping sent");
            }

            // ── Manual reconnect ──────────────────────────────────────────
            // Consuming the notification here means the reconnect button is
            // honoured even while the WS is actively running.
            _ = reconnect.notified() => {
                info!("aiwg serve reconnect requested — dropping current connection");
                let _ = sink.close().await;
                anyhow::bail!("manual reconnect");
            }
        }
    }
}
