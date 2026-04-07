//! Attach to a PTY session on an agent via WebSocket

use anyhow::{Context, Result};
use colored::Colorize;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// Client-to-server WebSocket message (matches management/src/ws/connection.rs)
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Subscribe {
        agent_id: String,
    },
    SendInput {
        agent_id: String,
        command_id: String,
        data: String,
    },
    StartShell {
        agent_id: String,
        cols: u32,
        rows: u32,
    },
    PtyResize {
        agent_id: String,
        command_id: String,
        cols: u32,
        rows: u32,
    },
    DetachSession {
        agent_id: String,
        session_name: String,
    },
    AttachSession {
        agent_id: String,
        session_name: String,
        cols: u32,
        rows: u32,
    },
}

/// Server-to-client WebSocket message (subset we handle)
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    Output {
        agent_id: String,
        data: String,
        stream: Option<String>,
    },
    PtyOutput {
        agent_id: String,
        command_id: Option<String>,
        data: String,
    },
    CommandStarted {
        command_id: String,
    },
    CommandCompleted {
        command_id: String,
        exit_code: i32,
    },
    Error {
        message: String,
    },
    Pong {
        timestamp: i64,
    },
    #[serde(other)]
    Unknown,
}

/// Get terminal size (cols, rows)
fn terminal_size() -> (u32, u32) {
    // Try to read actual terminal size via ioctl
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = std::io::stdout().as_raw_fd();
        let mut ws = libc_winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: fd is valid, ws is a valid output buffer
        unsafe {
            if libc_tiocgwinsz(fd, &mut ws) == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
                return (ws.ws_col as u32, ws.ws_row as u32);
            }
        }
    }
    (80, 24)
}

// Minimal inline ioctl wrapper for terminal size — avoids pulling in nix
#[cfg(unix)]
#[repr(C)]
struct libc_winsize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

#[cfg(unix)]
extern "C" {
    fn ioctl(fd: i32, request: u64, ...) -> i32;
}

#[cfg(unix)]
const TIOCGWINSZ: u64 = 0x5413; // Linux x86_64

#[cfg(unix)]
unsafe fn libc_tiocgwinsz(fd: i32, ws: *mut libc_winsize) -> i32 {
    ioctl(fd, TIOCGWINSZ, ws)
}

pub async fn run(server: &str, agent_id: &str, stdout: bool, stderr: bool) -> Result<()> {
    println!(
        "{} Attaching to agent: {}",
        "=>".blue().bold(),
        agent_id.cyan()
    );

    // Build WebSocket URL from management server address
    // server may be "http://host:8122" or "host:8121" or just "host"
    let ws_url = build_ws_url(server)?;
    println!("  WebSocket: {}", ws_url.dimmed());

    let filter = match (stdout, stderr) {
        (true, false) => "stdout only",
        (false, true) => "stderr only",
        _ => "stdout+stderr",
    };
    println!("  Filter: {}", filter);
    println!("  Ctrl+\\ to detach (leaves PTY running)\n");

    let (ws_stream, _) = connect_async(&ws_url)
        .await
        .with_context(|| format!("Failed to connect to WebSocket at {}", ws_url))?;

    let (mut write, mut read) = ws_stream.split();

    // Subscribe to this agent's output
    let sub_msg = serde_json::to_string(&ClientMessage::Subscribe {
        agent_id: agent_id.to_string(),
    })?;
    write.send(Message::Text(sub_msg.into())).await?;

    // Start a shell (PTY session)
    let (cols, rows) = terminal_size();
    let shell_msg = serde_json::to_string(&ClientMessage::StartShell {
        agent_id: agent_id.to_string(),
        cols,
        rows,
    })?;
    write.send(Message::Text(shell_msg.into())).await?;

    println!("{}", "Connected. Press Ctrl+\\ to detach.".green());

    // Track active command_id returned by server
    let command_id = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
    let agent_id_clone = agent_id.to_string();
    let command_id_read = command_id.clone();

    // Spawn task to read from WebSocket and write to stdout
    let mut print_task = tokio::spawn(async move {
        let mut stdout_w = tokio::io::stdout();
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => match serde_json::from_str::<ServerMessage>(&text) {
                    Ok(ServerMessage::Output { data, .. })
                    | Ok(ServerMessage::PtyOutput { data, .. }) => {
                        let _ = stdout_w.write_all(data.as_bytes()).await;
                        let _ = stdout_w.flush().await;
                    }
                    Ok(ServerMessage::CommandStarted { command_id: cid }) => {
                        *command_id_read.lock().await = cid;
                    }
                    Ok(ServerMessage::CommandCompleted { exit_code, .. }) => {
                        if exit_code != 0 {
                            eprintln!("\n[session ended with exit code {}]", exit_code);
                        } else {
                            eprintln!("\n[session ended]");
                        }
                        break;
                    }
                    Ok(ServerMessage::Error { message }) => {
                        eprintln!("\n{} {}", "Error:".red(), message);
                    }
                    _ => {}
                },
                Ok(Message::Binary(data)) => {
                    let _ = stdout_w.write_all(&data).await;
                    let _ = stdout_w.flush().await;
                }
                Ok(Message::Close(_)) => break,
                Err(e) => {
                    eprintln!("\n{} WebSocket error: {}", "!".red(), e);
                    break;
                }
                _ => {}
            }
        }
    });

    // Read from stdin and send as input
    let mut stdin = tokio::io::stdin();
    let mut buf = [0u8; 256];
    loop {
        tokio::select! {
            n = stdin.read(&mut buf) => {
                match n {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        // Check for Ctrl+\ (ASCII 28) — detach
                        if buf[..n].contains(&28u8) {
                            let cid = command_id.lock().await.clone();
                            if !cid.is_empty() {
                                let detach = serde_json::to_string(&ClientMessage::DetachSession {
                                    agent_id: agent_id_clone.clone(),
                                    session_name: cid,
                                });
                                if let Ok(msg) = detach {
                                    let _ = write.send(Message::Text(msg.into())).await;
                                }
                            }
                            println!("\n{}", "Detached (session still running).".yellow());
                            break;
                        }

                        let cid = command_id.lock().await.clone();
                        let input_msg = serde_json::to_string(&ClientMessage::SendInput {
                            agent_id: agent_id_clone.clone(),
                            command_id: cid,
                            data: String::from_utf8_lossy(&buf[..n]).to_string(),
                        });
                        if let Ok(msg) = input_msg {
                            if write.send(Message::Text(msg.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("stdin error: {}", e);
                        break;
                    }
                }
            }
            _ = &mut print_task => break,
        }
    }

    Ok(())
}

/// Build WebSocket URL from the management server address
fn build_ws_url(server: &str) -> Result<String> {
    // Strip http/https scheme and append WS port if needed
    let base = server
        .trim_start_matches("https://")
        .trim_start_matches("http://");

    // If it already has a port, use port 8121 as WS sibling
    let (host, _port) = if let Some(colon) = base.rfind(':') {
        let h = &base[..colon];
        let p: u16 = base[colon + 1..].parse().unwrap_or(8122);
        (h.to_string(), p)
    } else {
        (base.to_string(), 8122u16)
    };

    Ok(format!("ws://{}:8121", host))
}
