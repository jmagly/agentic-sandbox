//! Server-side extension behaviors.
//!
//! Filled in by W3.6 (#213). Each submodule implements one A2A extension
//! advertised in the AgentCard. The skeleton declares the modules so #213
//! can land each one without adding new files.

pub mod hitl_prompt;
pub mod idempotency;
pub mod multi_tenant;
pub mod pty_extensions;
pub mod runtime;
