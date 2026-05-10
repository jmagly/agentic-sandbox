//! Storage layer.
//!
//! The executor reuses the v2 SQLite [`TaskStore`] and [`IdempotencyCache`]
//! that landed in Wave 2 (W2.1 / W2.2) inside the existing
//! [`agentic_management`] crate. Rather than redefining those types here,
//! we re-export the existing module so handlers and extensions can depend on
//! a single canonical storage API.
//!
//! [`TaskStore`]: agentic_management::aiwg_serve::task_store
//! [`IdempotencyCache`]: agentic_management::aiwg_serve::idempotency

pub use agentic_management::aiwg_serve::idempotency;
pub use agentic_management::aiwg_serve::task_store;
