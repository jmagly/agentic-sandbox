//! `session` verbs against the formal session registry.
//!
//! Backing routes:
//! - `GET    /api/v1/sessions`                  ← `session list` (`--agent`)
//! - `GET    /api/v1/sessions` + filter         ← `session get <id>` (no per-id GET yet)
//! - `DELETE /api/v1/sessions/{id}?signal=...`  ← `session kill <id>`
//! - WS (formal protocol)                       ← `session attach/tail/record/input/resize`

use anyhow::Result;
use futures_util::StreamExt;
use serde_json::Value;

use crate::client::http::HttpClient;
use crate::client::ws::{self, ClientMessage, ServerMessage, SessionPayload};
use crate::output::{jstr, kv, table};

pub async fn list(c: &HttpClient, agent: Option<&str>, as_json: bool) -> Result<()> {
    let v: Value = c.get_value("/api/v1/sessions").await?;
    let arr = v.as_array().cloned().unwrap_or_default();
    let filtered: Vec<Value> = arr
        .into_iter()
        .filter(|s| match agent {
            Some(a) => jstr(s, "agent_id", "") == a,
            None => true,
        })
        .collect();
    let payload = Value::Array(filtered.clone());
    super::emit(&payload, as_json, || {
        let rows: Vec<Vec<String>> = filtered
            .iter()
            .map(|s| {
                vec![
                    jstr(s, "session_id", "").to_string(),
                    jstr(s, "agent_id", "-").to_string(),
                    jstr(s, "name", "-").to_string(),
                    crate::output::jnum(s, "attachment_count"),
                    crate::output::jnum(s, "max_client_lag"),
                ]
            })
            .collect();
        table::render(&["SESSION_ID", "AGENT", "NAME", "ATT", "LAG"], &rows)
    })
}

pub async fn get(c: &HttpClient, id: &str, as_json: bool) -> Result<()> {
    // No dedicated GET /api/v1/sessions/{id} exists yet; filter the list.
    let v: Value = c.get_value("/api/v1/sessions").await?;
    let arr = v.as_array().cloned().unwrap_or_default();
    let s = arr
        .iter()
        .find(|s| jstr(s, "session_id", "") == id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("session not found: {}", id))?;
    super::emit(&s, as_json, || {
        let controllers = s
            .get("controllers")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let observers = s
            .get("observers")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let pairs: Vec<(&str, String)> = vec![
            ("session_id", jstr(&s, "session_id", "").to_string()),
            ("agent_id", jstr(&s, "agent_id", "-").to_string()),
            ("command_id", jstr(&s, "command_id", "-").to_string()),
            ("name", jstr(&s, "name", "-").to_string()),
            (
                "attachment_count",
                crate::output::jnum(&s, "attachment_count"),
            ),
            ("controllers", controllers),
            ("observers", observers),
            (
                "replay_oldest_seq",
                crate::output::jnum(&s, "replay_oldest_seq"),
            ),
            (
                "replay_newest_seq",
                crate::output::jnum(&s, "replay_newest_seq"),
            ),
            ("replay_len", crate::output::jnum(&s, "replay_len")),
            (
                "replay_total_bytes",
                crate::output::jnum(&s, "replay_total_bytes"),
            ),
            ("max_client_lag", crate::output::jnum(&s, "max_client_lag")),
        ];
        kv::render(&pairs)
    })
}

/// `session kill <id> [--signal TERM|KILL|INT|HUP]` — admin verb.
/// Backing route: `DELETE /api/v1/sessions/{id}?signal=...`. Server
/// returns `{ session_id, signal, status }` on 200; 404 if missing.
pub async fn kill(c: &HttpClient, id: &str, signal: &str, as_json: bool) -> Result<()> {
    let mut q: Vec<(String, String)> = Vec::new();
    if !signal.is_empty() {
        q.push(("signal".into(), signal.into()));
    }
    let path = super::with_query(&format!("/api/v1/sessions/{}", id), &q);
    let v: Value = c.delete_json(&path).await?;
    super::emit(&v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("session_id", jstr(&v, "session_id", id).to_string()),
            ("signal", crate::output::jnum(&v, "signal")),
            ("status", jstr(&v, "status", "-").to_string()),
        ];
        kv::render(&pairs)
    })
}

/// `session tail <id>` — observer attach, line-buffered stdout, no TTY
/// switch. Suitable for scripts (`sandboxctl session tail $id | jq`).
pub async fn tail(c: &HttpClient, id: &str, replay_from: Option<u64>) -> Result<()> {
    let mut sock = ws::connect(c).await?;
    let _ = ws::join(&mut sock, id, "observer", replay_from).await?;
    while let Some(msg) = sock.next().await {
        let msg = msg?;
        match msg {
            tokio_tungstenite::tungstenite::Message::Text(t) => {
                let parsed: ServerMessage = match serde_json::from_str(&t) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let (stream, data) = match parsed {
                    ServerMessage::SessionFrame {
                        payload: SessionPayload::Output { stream, data },
                        ..
                    } => (stream, data),
                    ServerMessage::SessionFrame {
                        payload: SessionPayload::Keyframe { stream, data },
                        ..
                    } => (stream, data),
                    _ => continue,
                };
                let bytes = ws::decode_output(&data);
                // Tag stderr in tail mode so log-aggregation tools can
                // distinguish; stdout passthrough is unmodified.
                if stream == "stderr" {
                    let mut out = std::io::stderr().lock();
                    use std::io::Write as _;
                    let _ = out.write_all(&bytes);
                    let _ = out.flush();
                } else {
                    crate::pty::write_stdout(&bytes);
                }
            }
            tokio_tungstenite::tungstenite::Message::Close(_) => break,
            _ => continue,
        }
    }
    let _ = ws::send(
        &mut sock,
        &ClientMessage::LeaveSession {
            session_id: id.into(),
        },
    )
    .await;
    Ok(())
}

/// `session record <id> -o file` — write raw `SessionFrame` JSON Lines
/// to `file` (or stdout if `path` is `-`). One line per server frame so
/// the dump can be replayed offline by re-feeding it to a renderer.
pub async fn record(
    c: &HttpClient,
    id: &str,
    path: &std::path::Path,
    replay_from: Option<u64>,
) -> Result<()> {
    use std::io::Write;
    let mut sock = ws::connect(c).await?;
    let _ = ws::join(&mut sock, id, "observer", replay_from).await?;

    // Open output as either a file or unlocked stdout (per-line lock
    // is fine; each writeln! takes the lock briefly).
    let mut writer: Box<dyn Write + Send> = if path.as_os_str() == "-" {
        Box::new(std::io::stdout())
    } else {
        Box::new(std::fs::File::create(path)?)
    };

    while let Some(msg) = sock.next().await {
        let msg = msg?;
        match msg {
            tokio_tungstenite::tungstenite::Message::Text(t) => {
                // Pass through verbatim — t is already a JSON object per frame.
                writeln!(writer, "{}", t)?;
                writer.flush()?;
            }
            tokio_tungstenite::tungstenite::Message::Close(_) => break,
            _ => continue,
        }
    }
    let _ = ws::send(
        &mut sock,
        &ClientMessage::LeaveSession {
            session_id: id.into(),
        },
    )
    .await;
    Ok(())
}

/// `session input <id> --file -` — one-shot stdin push as a controller.
/// Sends the entire file (or stdin if `-`) as a single `SessionInput`
/// then leaves. Useful for piping a script into a session: `cat foo.sh
/// | sandboxctl session input <id> --file -`.
pub async fn input(c: &HttpClient, id: &str, path: &std::path::Path) -> Result<()> {
    let data = if path.as_os_str() == "-" {
        let mut buf = Vec::new();
        use std::io::Read as _;
        std::io::stdin().read_to_end(&mut buf)?;
        String::from_utf8_lossy(&buf).into_owned()
    } else {
        std::fs::read_to_string(path)?
    };
    let mut sock = ws::connect(c).await?;
    let _ = ws::join(&mut sock, id, "controller", None).await?;
    ws::send(
        &mut sock,
        &ClientMessage::SessionInput {
            session_id: id.into(),
            data,
        },
    )
    .await?;
    let _ = ws::send(
        &mut sock,
        &ClientMessage::LeaveSession {
            session_id: id.into(),
        },
    )
    .await;
    Ok(())
}

/// `session resize <id> --cols C --rows R` — one-shot PTY resize.
pub async fn resize(c: &HttpClient, id: &str, cols: u16, rows: u16) -> Result<()> {
    let mut sock = ws::connect(c).await?;
    let _ = ws::join(&mut sock, id, "controller", None).await?;
    ws::send(
        &mut sock,
        &ClientMessage::SessionResize {
            session_id: id.into(),
            cols,
            rows,
        },
    )
    .await?;
    let _ = ws::send(
        &mut sock,
        &ClientMessage::LeaveSession {
            session_id: id.into(),
        },
    )
    .await;
    Ok(())
}

/// `session attach <id>` — full interactive PTY join.
///
/// Default role is observer (read-only). `controller=true` requests
/// write access and prints a one-line warning naming current
/// controllers (from the `MembershipChanged` snapshot). Detach hotkey
/// is `Ctrl-A d` by default; the local TTY is restored on every exit
/// path including panic via the `RawGuard` Drop.
pub async fn attach(
    c: &HttpClient,
    id: &str,
    controller: bool,
    replay_from: Option<u64>,
) -> Result<()> {
    use tokio::sync::mpsc;
    use tokio_tungstenite::tungstenite::Message;

    let role = if controller { "controller" } else { "observer" };
    let mut sock = ws::connect(c).await?;
    let (granted, _seq) = ws::join(&mut sock, id, role, replay_from).await?;
    if granted != role {
        eprintln!(
            "warning: server granted role `{}` (we asked for `{}`)",
            granted, role
        );
    }

    // Synchronous print BEFORE entering raw mode so the line is on its
    // own. Operators care most about "who else is writing here" right
    // before they take control.
    if controller {
        eprintln!(
            "(attach: joining as controller; multi-writer is normal — \
             other controllers' input will interleave)"
        );
    }

    let _raw = crate::pty::RawGuard::enter()?;

    // Send the initial resize so the agent's PTY matches our local view.
    let (cols, rows) = crate::pty::current_size();
    ws::send(
        &mut sock,
        &ClientMessage::SessionResize {
            session_id: id.into(),
            cols,
            rows,
        },
    )
    .await?;

    // stdin → bytes → SessionInput
    let (mut stdin_rx, mut detach_rx, _stdin_handle) = crate::pty::spawn_stdin_pump(0x01, b'd'); // Ctrl-A 'd'
                                                                                                 // SIGWINCH → SessionResize
    let (mut winch_rx, _winch_handle) = crate::pty::spawn_winch_pump();

    // Channel that lets the main loop request a graceful close.
    let (close_tx, mut close_rx) = mpsc::channel::<()>(1);

    loop {
        tokio::select! {
            biased;

            _ = detach_rx.recv() => {
                break;
            }
            _ = close_rx.recv() => {
                break;
            }
            Some((c, r)) = winch_rx.recv() => {
                let _ = ws::send(
                    &mut sock,
                    &ClientMessage::SessionResize { session_id: id.into(), cols: c, rows: r },
                ).await;
            }
            Some(bytes) = stdin_rx.recv() => {
                if !controller {
                    continue; // observer stdin is dropped silently
                }
                let s = String::from_utf8_lossy(&bytes).into_owned();
                let _ = ws::send(
                    &mut sock,
                    &ClientMessage::SessionInput { session_id: id.into(), data: s },
                ).await;
            }
            msg = sock.next() => {
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        let parsed: ServerMessage = match serde_json::from_str(&t) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        match parsed {
                            ServerMessage::SessionFrame {
                                payload: SessionPayload::Output { stream, data }, ..
                            }
                            | ServerMessage::SessionFrame {
                                payload: SessionPayload::Keyframe { stream, data }, ..
                            } => {
                                let bytes = ws::decode_output(&data);
                                if stream == "stderr" {
                                    use std::io::Write as _;
                                    let mut out = std::io::stderr().lock();
                                    let _ = out.write_all(&bytes);
                                    let _ = out.flush();
                                } else {
                                    crate::pty::write_stdout(&bytes);
                                }
                            }
                            ServerMessage::SessionFrame {
                                payload: SessionPayload::Closed { exit_code }, ..
                            } => {
                                eprintln!(
                                    "\r\n[session ended; exit_code={:?}]\r",
                                    exit_code
                                );
                                let _ = close_tx.send(()).await;
                            }
                            ServerMessage::SessionFrame {
                                payload: SessionPayload::Error { message }, ..
                            } => {
                                eprintln!("\r\n[session error: {}]\r", message);
                            }
                            ServerMessage::Error { message } => {
                                eprintln!("\r\n[server error: {}]\r", message);
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        eprintln!("\r\n[ws error: {}]\r", e);
                        break;
                    }
                    _ => continue,
                }
            }
        }
    }

    // Best-effort detach.
    let _ = ws::send(
        &mut sock,
        &ClientMessage::LeaveSession {
            session_id: id.into(),
        },
    )
    .await;
    drop(_raw);
    Ok(())
}
