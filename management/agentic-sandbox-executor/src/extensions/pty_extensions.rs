//! `agentic-sandbox/pty` extension. Filled in by #213 + W4.1.
//!
//! Advertises the PTY-over-WebSocket binding and surfaces TTY metadata
//! (rows, cols, encoding) on the AgentCard.

/// PTY-extension marker.
pub struct PtyExtension;
