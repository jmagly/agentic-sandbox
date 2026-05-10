//! Transport bindings for the executor.
//!
//! - [`rest`]: A2A REST + JSON-RPC over HTTP (#210).
//! - [`pty_ws`]: PTY-over-WebSocket binding (Wave 4 W4.1).
//! - [`pty_bridge`]: Source-of-output abstraction for `pty-ws/v1` sessions
//!   (#237). Lets management swap in a real agent-backed bridge while the
//!   executor crate defaults to a no-op for tests and harness builds.

pub mod pty_bridge;
pub mod pty_ws;
pub mod rest;
