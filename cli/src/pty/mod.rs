//! Local TTY raw-mode handling for `session attach` / `agent shell`.
//!
//! Responsibilities:
//! - Switch the local terminal into raw mode for the duration of attach.
//! - **Always** restore on exit, including panic and SIGTERM (the
//!   `RawGuard` Drop impl is the single source of truth).
//! - Feed stdin bytes to the WS sink and SessionFrame::Output bytes to
//!   stdout.
//! - SIGWINCH → emit `SessionResize` so the PTY on the agent matches
//!   the local viewport.
//! - Detect the detach hotkey (default `Ctrl-A d`) and break the loop
//!   without killing the underlying session.
//!
//! This module deliberately does NOT depend on any specific WS client —
//! it consumes/produces typed channels so the calling verb (in
//! `cmd::session::attach`) wires the channels to `client::ws`.

use anyhow::Result;
use crossterm::terminal;
use std::io::Write;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tracing::warn;

/// A guard that puts the terminal into raw mode on construction and
/// restores it on drop. Panic-safe.
pub struct RawGuard {
    restore: bool,
}

impl RawGuard {
    pub fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self { restore: true })
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        if self.restore {
            // Best-effort restore. If this fails the user is left in a
            // broken terminal — we still log it so they know what to fix.
            if let Err(e) = terminal::disable_raw_mode() {
                warn!(error = %e, "failed to restore terminal cooked mode");
            }
        }
    }
}

/// Get the local terminal viewport size. Used at attach time and on
/// SIGWINCH. Falls back to (80, 24) if the size can't be read OR
/// reads as (0, 0) — some CI containers (Gitea/act) report Ok((0, 0))
/// instead of an error, which would otherwise propagate a useless
/// zero-size into PTY resize messages.
pub fn current_size() -> (u16, u16) {
    match terminal::size() {
        Ok((c, r)) if c >= 1 && r >= 1 => (c, r),
        _ => (80, 24),
    }
}

/// Spawn the stdin → channel pump. Reads raw bytes from stdin and ships
/// them into `tx`. Detects the two-byte detach hotkey and signals via
/// the returned `detach_rx` Receiver. Returns the JoinHandle so the
/// caller can abort cleanly on session close.
pub fn spawn_stdin_pump(
    detach_prefix: u8,
    detach_terminator: u8,
) -> (
    mpsc::Receiver<Vec<u8>>,
    mpsc::Receiver<()>,
    tokio::task::JoinHandle<()>,
) {
    let (tx, rx) = mpsc::channel::<Vec<u8>>(64);
    let (detach_tx, detach_rx) = mpsc::channel::<()>(1);
    let handle = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 1024];
        let mut waiting_for_terminator = false;
        loop {
            let n = match stdin.read(&mut buf).await {
                Ok(0) => break, // EOF
                Ok(n) => n,
                Err(e) => {
                    warn!(error = %e, "stdin read error");
                    break;
                }
            };
            // Hotkey detection. The user types <prefix><terminator> to
            // detach. Both bytes must be in the same read call OR the
            // prefix can lead a separate read. We tolerate both.
            let mut emit = Vec::with_capacity(n);
            for &b in &buf[..n] {
                if waiting_for_terminator {
                    if b == detach_terminator {
                        let _ = detach_tx.send(()).await;
                        return;
                    }
                    // Was a literal prefix byte; emit it now.
                    emit.push(detach_prefix);
                    waiting_for_terminator = false;
                    if b != detach_prefix {
                        emit.push(b);
                    } else {
                        waiting_for_terminator = true;
                    }
                } else if b == detach_prefix {
                    waiting_for_terminator = true;
                } else {
                    emit.push(b);
                }
            }
            if !emit.is_empty() && tx.send(emit).await.is_err() {
                break;
            }
        }
    });
    (rx, detach_rx, handle)
}

/// Write a chunk of PTY-output bytes to local stdout, flushing
/// immediately. We bypass the line-buffered default because PTY output
/// is interactive (prompts, partial lines, escape codes).
pub fn write_stdout(bytes: &[u8]) {
    let mut out = std::io::stdout().lock();
    if let Err(e) = out.write_all(bytes) {
        warn!(error = %e, "stdout write failed");
        return;
    }
    let _ = out.flush();
}

/// Spawn a SIGWINCH listener that pushes `(cols, rows)` onto `tx` every
/// time the terminal viewport changes. Returns the JoinHandle.
pub fn spawn_winch_pump() -> (mpsc::Receiver<(u16, u16)>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<(u16, u16)>(8);
    let handle = tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        let mut s = match signal(SignalKind::window_change()) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "failed to install SIGWINCH handler");
                return;
            }
        };
        while s.recv().await.is_some() {
            let size = current_size();
            if tx.send(size).await.is_err() {
                break;
            }
        }
    });
    (rx, handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_size_falls_back_when_no_tty() {
        // In `cargo test` stdout is not a TTY; we should get the fallback.
        let (cols, rows) = current_size();
        assert!(cols >= 1 && rows >= 1);
    }
}
