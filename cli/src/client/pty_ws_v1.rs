//! WebSocket client for the executor `pty-ws.v1` attach binding.
//!
//! Endpoint:
//! `/agents/{instance_id}/sessions/{session_id}/attach`.

use anyhow::{anyhow, Context, Result};
use futures_util::sink::SinkExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async, tungstenite::client::IntoClientRequest, tungstenite::Message, MaybeTlsStream,
    WebSocketStream,
};
use tracing::warn;

use super::http::HttpClient;

pub const SUBPROTOCOL: &str = "pty-ws.v1";

pub type PtyWsV1Stream = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug, Clone, Serialize)]
pub struct ClientFrame<'a> {
    pub op: &'a str,
    pub ts: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerFrame {
    pub op: String,
    #[serde(default, alias = "sequence")]
    pub seq: Option<u64>,
    #[serde(default)]
    pub payload: Value,
}

pub async fn connect(
    c: &HttpClient,
    instance_id: &str,
    session_id: &str,
    replay_from: Option<u64>,
) -> Result<PtyWsV1Stream> {
    let url = pty_ws_url_from_http(c.base(), instance_id, session_id, replay_from)?;
    let mut req = url
        .into_client_request()
        .with_context(|| "building pty-ws/v1 upgrade request")?;
    req.headers_mut()
        .insert("Sec-WebSocket-Protocol", SUBPROTOCOL.parse()?);
    if let Some(tok) = c.bearer_token() {
        let v = format!("Bearer {}", tok)
            .parse()
            .map_err(|_| anyhow!("invalid bearer token for WS Authorization header"))?;
        req.headers_mut().insert("Authorization", v);
    }

    let (stream, resp) = connect_async(req)
        .await
        .with_context(|| "pty-ws/v1 connect failed")?;
    let echoed = resp
        .headers()
        .get("Sec-WebSocket-Protocol")
        .and_then(|v| v.to_str().ok());
    if echoed != Some(SUBPROTOCOL) {
        return Err(anyhow!(
            "server did not negotiate {} subprotocol; got {:?}",
            SUBPROTOCOL,
            echoed
        ));
    }
    Ok(stream)
}

pub async fn send(ws: &mut PtyWsV1Stream, op: &'static str, payload: Value) -> Result<()> {
    let frame = ClientFrame {
        op,
        ts: chrono::Utc::now().to_rfc3339(),
        payload,
    };
    let text = serde_json::to_string(&frame)?;
    ws.send(Message::Text(text.into())).await?;
    Ok(())
}

pub fn encode_input(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn decode_output(data: &str) -> Vec<u8> {
    use base64::Engine as _;
    match base64::engine::general_purpose::STANDARD.decode(data) {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "failed to decode pty-ws/v1 output base64; skipping frame");
            Vec::new()
        }
    }
}

fn pty_ws_url_from_http(
    http_base: &str,
    instance_id: &str,
    session_id: &str,
    replay_from: Option<u64>,
) -> Result<String> {
    let stripped = http_base
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let scheme = if http_base.starts_with("https://") {
        "wss"
    } else {
        "ws"
    };
    let mut url = format!(
        "{scheme}://{}/agents/{}/sessions/{}/attach",
        stripped,
        encode_path_segment(instance_id),
        encode_path_segment(session_id)
    );
    if let Some(seq) = replay_from {
        url.push_str(&format!("?replay_from={seq}"));
    }
    Ok(url)
}

fn encode_path_segment(raw: &str) -> String {
    let mut out = String::new();
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_executor_attach_url_on_http_port() {
        assert_eq!(
            pty_ws_url_from_http("http://localhost:8122", "agent-a", "sess-1", None).unwrap(),
            "ws://localhost:8122/agents/agent-a/sessions/sess-1/attach"
        );
        assert_eq!(
            pty_ws_url_from_http("https://example.org:8443/", "agent/a", "sess 1", Some(42))
                .unwrap(),
            "wss://example.org:8443/agents/agent%2Fa/sessions/sess%201/attach?replay_from=42"
        );
    }

    #[test]
    fn parses_seq_and_sequence_alias() {
        let seq: ServerFrame =
            serde_json::from_str(r#"{"op":"output","seq":7,"payload":{}}"#).unwrap();
        assert_eq!(seq.seq, Some(7));
        let sequence: ServerFrame =
            serde_json::from_str(r#"{"op":"output","sequence":8,"payload":{}}"#).unwrap();
        assert_eq!(sequence.seq, Some(8));
    }

    #[test]
    fn input_base64_round_trip() {
        let encoded = encode_input(b"hello\n");
        assert_eq!(decode_output(&encoded), b"hello\n");
    }
}
