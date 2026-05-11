//! Storage layer.
//!
//! Owns the v2 SQLite [`task_store::TaskStore`] and [`idempotency::IdempotencyCache`]
//! that landed in Wave 2 (W2.1 / W2.2).
//!
//! Previously these modules lived under `agentic_management::aiwg_serve` and
//! were re-exported here, but #243 inverted the dependency direction so the
//! management binary can mount the executor's REST router. The modules now
//! live here canonically; management consumes them as
//! `agentic_sandbox_executor::store::{task_store, idempotency}`.
//!
//! The v1→v2 migration tool (W2.3) stays in the management crate because it
//! straddles both `MissionRecord` (v1, lives in management) and `TaskStore`
//! (v2, lives here).

pub mod idempotency;
pub mod task_store;
