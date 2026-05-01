//! WebSocket client for the formal session-registry protocol.
//!
//! Speaks `JoinSession` / `LeaveSession` / `SessionInput` / `SessionResize`
//! to the server defined in `management/src/ws/connection.rs`. Used by
//! `session attach`, `session tail`, `session record`, `session input`,
//! `session resize`, and `agent shell`.
//!
//! WS endpoint: `ws://<host>:<ws-port>/`. The CLI's `--server` flag
//! gives an HTTP URL; we derive `<host>` and assume the WS sibling
//! port is `<http-port> - 1` (the default mgmt-server convention:
//! gRPC 8120, WS 8121, HTTP 8122). Override via `AGENTIC_WS_PORT`.

use anyhow::{anyhow, Context, Result};
use futures_util::sink::SinkExt;
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async, tungstenite::client::IntoClientRequest, tungstenite::Message, MaybeTlsStream,
    WebSocketStream,
};
use tracing::warn;

use super::http::HttpClient;

pub type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

// ── Wire types (formal session protocol) ─────────────────────────────────
//
// These match `management/src/ws/connection.rs`. When the server adds new
// variants, expand here. Anything not in this enum the server sends will
// be ignored (or surface as a parse error in `--json` mode).

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    JoinSession {
        session_id: String,
        role: String, // "controller" | "observer"
        #[serde(skip_serializing_if = "Option::is_none")]
        replay_from: Option<u64>,
    },
    LeaveSession {
        session_id: String,
    },
    SessionInput {
        session_id: String,
        data: String,
    },
    SessionResize {
        session_id: String,
        cols: u16,
        rows: u16,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    SessionJoined {
        session_id: String,
        role: String,
        current_seq: u64,
    },
    SessionLeft {
        session_id: String,
    },
    SessionFrame {
        session_id: String,
        seq: u64,
        ts: i64,
        #[serde(flatten)]
        payload: SessionPayload,
    },
    Error {
        message: String,
    },
    /// Catch-all so the legacy WS messages (Output, AgentList, etc.) and
    /// any future server additions don't blow up our deserializer.
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionPayload {
    Output {
        stream: String, // "stdout" | "stderr" | "log"
        data: String,   // base64-encoded bytes
    },
    /// Periodic full-repaint snapshot — same wire shape as Output but
    /// flagged so smart clients can recognize it as a safe replay
    /// starting point (#145). For the CLI's attach/tail/record paths
    /// we treat it identically to Output: write the decoded bytes to
    /// stdout. The bytes already include the SGR/cursor escape
    /// sequences needed to reproduce the screen state.
    Keyframe {
        stream: String,
        data: String,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    RoleAssigned {
        role: String,
    },
    MembershipChanged {
        controllers: Vec<String>,
        observers: Vec<String>,
    },
    Closed {
        exit_code: Option<i32>,
    },
    Error {
        message: String,
    },
}

// ── Connection ────────────────────────────────────────────────────────────

/// Open a WS connection to the management server's session port.
/// Adds the bearer token from the active context as an `Authorization`
/// header on the upgrade request.
pub async fn connect(c: &HttpClient) -> Result<WsStream> {
    let url = ws_url_from_http(c.base())?;
    let mut req = url
        .into_client_request()
        .with_context(|| "building WS upgrade request")?;
    if let Some(tok) = c.bearer_token() {
        let v = format!("Bearer {}", tok)
            .parse()
            .map_err(|_| anyhow!("invalid bearer token for WS Authorization header"))?;
        req.headers_mut().insert("Authorization", v);
    }
    let (stream, _resp) = connect_async(req)
        .await
        .with_context(|| "WS connect failed")?;
    Ok(stream)
}

/// Send a typed `ClientMessage` as a JSON text frame.
pub async fn send(ws: &mut WsStream, msg: &ClientMessage) -> Result<()> {
    let text = serde_json::to_string(msg)?;
    ws.send(Message::Text(text.into())).await?;
    Ok(())
}

/// Convenience: round-trip JoinSession and wait for the matching
/// SessionJoined reply (or an Error). Returns `(role, current_seq)`.
pub async fn join(
    ws: &mut WsStream,
    session_id: &str,
    role: &str,
    replay_from: Option<u64>,
) -> Result<(String, u64)> {
    use futures_util::StreamExt;
    send(
        ws,
        &ClientMessage::JoinSession {
            session_id: session_id.to_string(),
            role: role.to_string(),
            replay_from,
        },
    )
    .await?;
    while let Some(msg) = ws.next().await {
        let msg = msg.with_context(|| "WS read")?;
        match msg {
            Message::Text(t) => match serde_json::from_str::<ServerMessage>(&t) {
                Ok(ServerMessage::SessionJoined {
                    role, current_seq, ..
                }) => return Ok((role, current_seq)),
                Ok(ServerMessage::Error { message }) => {
                    return Err(anyhow!("server rejected JoinSession: {}", message))
                }
                Ok(_) | Err(_) => {
                    // Pre-join chatter (legacy messages, etc.). Skip.
                    continue;
                }
            },
            Message::Close(_) => return Err(anyhow!("WS closed before SessionJoined")),
            _ => continue,
        }
    }
    Err(anyhow!("WS stream ended before SessionJoined"))
}

/// Build the WS URL from the HTTP base URL. Strips the scheme, swaps
/// the trailing port for the WS port (HTTP - 1, override via env), and
/// emits `ws://<host>:<port>/`.
fn ws_url_from_http(http_base: &str) -> Result<String> {
    let stripped = http_base
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let scheme = if http_base.starts_with("https://") {
        "wss"
    } else {
        "ws"
    };
    let (host, http_port) = match stripped.rfind(':') {
        Some(i) => {
            let h = &stripped[..i];
            let p: u16 = stripped[i + 1..].parse().unwrap_or(8122);
            (h.to_string(), p)
        }
        None => (stripped.to_string(), 8122u16),
    };
    let ws_port: u16 = match std::env::var("AGENTIC_WS_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        Some(p) => p,
        None => http_port.saturating_sub(1).max(1),
    };
    Ok(format!("{scheme}://{host}:{ws_port}/"))
}

/// Decode the base64 `data` field of a `SessionPayload::Output` into raw
/// bytes. Logs a warning and returns empty on bad input rather than
/// killing the attach loop over a single corrupt frame.
pub fn decode_output(data: &str) -> Vec<u8> {
    use base64::Engine as _;
    match base64::engine::general_purpose::STANDARD.decode(data) {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "failed to decode SessionFrame::Output base64; skipping frame");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_port_derives_from_http_port() {
        std::env::remove_var("AGENTIC_WS_PORT");
        assert_eq!(
            ws_url_from_http("http://localhost:8122").unwrap(),
            "ws://localhost:8121/"
        );
        assert_eq!(
            ws_url_from_http("https://example.org:8122/").unwrap(),
            "wss://example.org:8121/"
        );
    }

    #[test]
    fn ws_port_override_via_env() {
        std::env::set_var("AGENTIC_WS_PORT", "9001");
        let url = ws_url_from_http("http://localhost:8122").unwrap();
        std::env::remove_var("AGENTIC_WS_PORT");
        assert_eq!(url, "ws://localhost:9001/");
    }

    #[test]
    fn decode_output_handles_garbage() {
        assert!(decode_output("!!!not_base64!!!").is_empty());
        assert_eq!(decode_output("aGVsbG8="), b"hello".to_vec());
    }

    #[test]
    fn server_message_other_variant_for_unknown_types() {
        let raw = r#"{"type":"output","agent_id":"a","data":"x","stream":"stdout"}"#;
        let v: ServerMessage = serde_json::from_str(raw).unwrap();
        assert!(matches!(v, ServerMessage::Other));
    }

    #[test]
    fn session_frame_output_payload_round_trips() {
        let raw = r#"{"type":"session_frame","session_id":"s","seq":5,"ts":1,"kind":"output","stream":"stdout","data":"aGk="}"#;
        let v: ServerMessage = serde_json::from_str(raw).unwrap();
        match v {
            ServerMessage::SessionFrame {
                seq,
                payload: SessionPayload::Output { stream, data },
                ..
            } => {
                assert_eq!(seq, 5);
                assert_eq!(stream, "stdout");
                assert_eq!(decode_output(&data), b"hi");
            }
            _ => panic!("expected SessionFrame::Output"),
        }
    }
}
