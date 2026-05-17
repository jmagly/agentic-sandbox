//! Transport bindings for the executor.
//!
//! - [`rest`]: A2A REST + JSON-RPC over HTTP (#210).
//! - [`pty_ws`]: PTY-over-WebSocket binding (Wave 4 W4.1).
//! - [`pty_bridge`]: Source-of-output abstraction for `pty-ws/v1` sessions
//!   (#237). Lets management swap in a real agent-backed bridge while the
//!   executor crate defaults to a no-op for tests and harness builds.
//!
//! The real [`agent_pty_bridge::AgentPtyBridge`] implementation lives in the
//! `agentic-management` crate (under `crate::agent_pty_bridge`) because it
//! depends on management types (`AgentRegistry`, `CommandDispatcher`,
//! gRPC proto). #243 moved it there so the management binary can depend
//! on this crate without a workspace cycle.

pub mod message_dispatch;
pub mod pty_bridge;
pub mod pty_ws;
pub mod rest;
