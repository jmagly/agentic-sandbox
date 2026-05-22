//! Orchestrator-oriented TUI helpers.
//!
//! These commands sit above the lower-level `session` verbs. They use the
//! structured orchestrator screen endpoint when possible so external agents can
//! read and drive a TUI without hand-rolling WebSocket frames.

use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
    MaybeTlsStream, WebSocketStream,
};

use crate::client::http::HttpClient;
use crate::output::{jstr, kv};

const DEFAULT_OBSERVE_TIMEOUT_SECS: u64 = 10;

pub async fn snapshot(c: &HttpClient, id: &str, as_json: bool) -> Result<()> {
    let v: Value = c
        .get_value(&format!("/api/v1/sessions/{}/screen", super::urlencode(id)))
        .await?;
    super::emit(&v, as_json, || {
        let text = jstr(&v, "text", "");
        let prompt = jstr(&v, "prompt_text", "");
        let mut pairs: Vec<(&str, String)> = vec![
            ("session_id", jstr(&v, "session_id", id).to_string()),
            ("rows", crate::output::jnum(&v, "rows")),
            ("cols", crate::output::jnum(&v, "cols")),
            (
                "prompt_detected",
                v.get("prompt_detected")
                    .and_then(|x| x.as_bool())
                    .map(|b| b.to_string())
                    .unwrap_or_else(|| "false".to_string()),
            ),
        ];
        if !prompt.is_empty() {
            pairs.push(("prompt_text", prompt.to_string()));
        }
        pairs.push(("text", text.to_string()));
        kv::render(&pairs)
    })
}

pub async fn observe(
    c: &HttpClient,
    id: &str,
    frames: usize,
    timeout: Duration,
    idle_ok: bool,
    as_json: bool,
) -> Result<()> {
    let mut sock = connect_orchestrator(c, id, "observer").await?;
    let limit = frames.max(1);
    let mut seen = 0usize;

    let result = tokio::time::timeout(timeout, async {
        while let Some(msg) = sock.next().await {
            let msg = msg?;
            let Message::Text(text) = msg else {
                continue;
            };
            let value: Value =
                serde_json::from_str(&text).with_context(|| "decoding orchestrator frame")?;
            emit_frame(&value, as_json)?;
            seen += 1;
            if seen >= limit {
                break;
            }
        }
        Ok::<(), anyhow::Error>(())
    })
    .await;

    let _ = sock.close(None).await;
    match result {
        Ok(inner) => inner,
        Err(_) if observe_timeout_is_success(seen, idle_ok) => Ok(()),
        Err(_) => Err(anyhow!(
            "timed out after {:?} waiting for orchestrator frames",
            timeout
        )),
    }
}

pub async fn send(
    c: &HttpClient,
    id: &str,
    text: &str,
    enter: bool,
    yes_controller: bool,
    as_json: bool,
) -> Result<()> {
    if !yes_controller {
        anyhow::bail!(
            "refusing controller write without --yes-controller; observe first, then opt in explicitly"
        );
    }

    let mut sock = connect_orchestrator(c, id, "controller").await?;
    let first =
        wait_for_session_start(&mut sock, Duration::from_secs(DEFAULT_OBSERVE_TIMEOUT_SECS))
            .await?;
    let can_write = first
        .get("can_write")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    if !can_write {
        let _ = sock.close(None).await;
        anyhow::bail!("server did not grant write authority for controller attach");
    }

    let payload = if enter {
        format!("{}\n", text)
    } else {
        text.to_string()
    };
    let frame = json!({ "type": "write", "text": payload });
    sock.send(Message::Text(frame.to_string().into())).await?;
    let _ = sock.close(None).await;

    let result = json!({
        "session_id": id,
        "role": "controller",
        "sent_bytes": frame["text"].as_str().unwrap_or_default().len(),
        "entered": enter,
    });
    super::emit(&result, as_json, || {
        kv::render(&[
            ("session_id", id.to_string()),
            ("role", "controller".to_string()),
            (
                "sent_bytes",
                frame["text"].as_str().unwrap_or_default().len().to_string(),
            ),
        ])
    })
}

pub async fn search(
    c: &HttpClient,
    id: &str,
    query: &str,
    limit: usize,
    as_json: bool,
) -> Result<()> {
    let path = super::with_query(
        &format!("/api/v1/sessions/{}/transcript", super::urlencode(id)),
        &[
            ("q".to_string(), query.to_string()),
            ("limit".to_string(), limit.to_string()),
        ],
    );
    let v: Value = c.get_value(&path).await?;
    super::emit(&v, as_json, || {
        let mut out = String::new();
        out.push_str(&format!("session_id: {}\n", jstr(&v, "session_id", id)));
        if let Some(items) = v.get("items").and_then(|x| x.as_array()) {
            for item in items {
                let seq = item.get("seq").and_then(|x| x.as_u64()).unwrap_or(0);
                let stream = jstr(item, "stream", "-");
                let data = jstr(item, "data", "");
                out.push_str(&format!("[{seq}] {stream}: {data}\n"));
            }
        }
        out
    })
}

fn emit_frame(value: &Value, as_json: bool) -> Result<()> {
    if as_json {
        println!("{}", serde_json::to_string(value)?);
        return Ok(());
    }

    match jstr(value, "type", "") {
        "session_start" => {
            let can_write = value
                .get("can_write")
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            println!(
                "session_start session_id={} role={} can_write={}",
                jstr(value, "session_id", "-"),
                jstr(value, "role", "-"),
                can_write
            );
        }
        "screen_update" => {
            let screen = value.get("screen").unwrap_or(value);
            println!(
                "screen_update session_id={} rows={} cols={} prompt_detected={}",
                jstr(value, "session_id", "-"),
                crate::output::jnum(screen, "rows"),
                crate::output::jnum(screen, "cols"),
                value
                    .get("prompt_detected")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false)
            );
            println!("{}", jstr(screen, "text", ""));
        }
        other => println!(
            "{}",
            if other.is_empty() {
                value.to_string()
            } else {
                other.to_string()
            }
        ),
    }
    Ok(())
}

async fn wait_for_session_start(
    sock: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    timeout: Duration,
) -> Result<Value> {
    tokio::time::timeout(timeout, async {
        while let Some(msg) = sock.next().await {
            let msg = msg?;
            let Message::Text(text) = msg else {
                continue;
            };
            let value: Value = serde_json::from_str(&text)?;
            if value.get("type").and_then(|x| x.as_str()) == Some("session_start") {
                return Ok(value);
            }
        }
        Err(anyhow!(
            "orchestrator websocket closed before session_start"
        ))
    })
    .await
    .map_err(|_| anyhow!("timed out waiting for session_start"))?
}

async fn connect_orchestrator(
    c: &HttpClient,
    session_id: &str,
    role: &str,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
    let url = orchestrator_url(c.base(), session_id, role)?;
    let mut req = url
        .into_client_request()
        .with_context(|| "building orchestrator WS upgrade request")?;
    if let Some(tok) = c.bearer_token() {
        let v = format!("Bearer {}", tok)
            .parse()
            .map_err(|_| anyhow!("invalid bearer token for WS Authorization header"))?;
        req.headers_mut().insert("Authorization", v);
    }
    let (stream, _resp) = connect_async(req)
        .await
        .with_context(|| "orchestrator WS connect failed")?;
    Ok(stream)
}

fn observe_timeout_is_success(seen_frames: usize, idle_ok: bool) -> bool {
    idle_ok && seen_frames > 0
}

fn orchestrator_url(http_base: &str, session_id: &str, role: &str) -> Result<String> {
    let stripped = http_base
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let scheme = if http_base.starts_with("https://") {
        "wss"
    } else {
        "ws"
    };
    Ok(format!(
        "{scheme}://{stripped}/ws/sessions/{}/orchestrate?role={}",
        super::urlencode(session_id),
        super::urlencode(role)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_idle_ok_requires_at_least_one_frame() {
        assert!(observe_timeout_is_success(1, true));
        assert!(!observe_timeout_is_success(0, true));
        assert!(!observe_timeout_is_success(1, false));
    }

    #[test]
    fn orchestrator_url_uses_http_port_and_path() {
        let url = orchestrator_url("http://localhost:8122", "sess-1", "observer").unwrap();
        assert_eq!(
            url,
            "ws://localhost:8122/ws/sessions/sess-1/orchestrate?role=observer"
        );
    }

    #[test]
    fn orchestrator_url_uses_wss_for_https() {
        let url = orchestrator_url("https://example.test:9443/", "s/1", "controller").unwrap();
        assert_eq!(
            url,
            "wss://example.test:9443/ws/sessions/s%2F1/orchestrate?role=controller"
        );
    }
}
