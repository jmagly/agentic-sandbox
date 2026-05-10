//! Transport bindings for the executor.
//!
//! - [`rest`]: A2A REST + JSON-RPC over HTTP (#210).
//! - [`pty_ws`]: PTY-over-WebSocket binding (Wave 4 W4.1).

pub mod pty_ws;
pub mod rest;
