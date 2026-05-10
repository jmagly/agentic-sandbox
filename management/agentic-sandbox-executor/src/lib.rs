//! # agentic-sandbox-executor
//!
//! v2 reference implementation of the agentic-sandbox executor contract,
//! aligned to the A2A protocol per ADR-018, ADR-021, and ADR-022.
//!
//! ## Three-Surface Architecture (ADR-022)
//!
//! agentic-sandbox v2 exposes three distinct surfaces:
//!
//! 1. **Surface 1 — Sandbox/Host control plane** (existing management server,
//!    crate `agentic-management`). VM lifecycle, registry, dashboard.
//! 2. **Surface 2 — A2A per-instance executor** (this crate). Each running
//!    agent instance speaks A2A via JSON-RPC over HTTP plus optional REST,
//!    SSE, and PTY/WebSocket bindings. The executor exposes one A2A
//!    `AgentCard` per instance and serves `message/send`, `tasks/get`,
//!    `tasks/list`, `tasks/cancel`, and the streaming/subscription methods.
//! 3. **Surface 3 — Aggregator/coordinator** (e.g. `aiwg serve`). Discovers
//!    Surface-2 endpoints and orchestrates missions across many instances.
//!
//! ## Issue Map
//!
//! This crate is built incrementally across Wave 3 of the v2 executor
//! contract initiative:
//!
//! | Issue | Surface area filled in |
//! |-------|------------------------|
//! | #208  | Crate bootstrap + skeleton (this commit) |
//! | #209  | [`agent_card`] generation, JCS canonicalization, JWS signing |
//! | #210  | [`bindings::rest`] A2A REST handlers backed by [`handlers`] |
//! | #211  | [`handlers::push_notification`] outbound delivery |
//! | #212  | [`instance`] `InstanceContext` registry + per-instance routing |
//! | #213  | [`extensions`] server-side behaviors (HITL, idempotency, etc.) |
//! | W4.1  | [`bindings::pty_ws`] PTY-over-WebSocket binding |
//!
//! Storage is backed by the existing v2 [`agentic_management::aiwg_serve`]
//! `TaskStore` and `IdempotencyCache` (Wave 2 W2.1 / W2.2). See [`store`]
//! for the re-export.
//!
//! ## Status
//!
//! Bootstrap-stage skeleton. All public items are placeholders that compile
//! but contain `todo!()` or empty stubs. Subsequent issues fill in real
//! behavior.

#![allow(dead_code, unused_variables)]

pub mod agent_card;
pub mod auth;
pub mod bindings;
pub mod extensions;
pub mod handlers;
pub mod instance;
pub mod server;
pub mod store;

// Re-export the most commonly used types so downstream callers can write
// `use agentic_sandbox_executor::{InstanceContext, ExecutorServer}`.
pub use crate::instance::InstanceContext;
pub use crate::server::ExecutorServer;
