//! HTTP server for web dashboard
//!
//! Serves static files and REST API endpoints for the control plane UI.

pub mod events;
pub mod health;
pub mod hitl;
pub mod idempotency;
pub mod loadouts;
pub mod operations;
pub mod rate_limit;
pub mod orchestrate;
mod server;
pub mod tasks;
pub mod validation;
pub mod vms;
mod vms_extended;

pub use operations::OperationStore;
pub use server::HttpServer;
pub use vms_extended::{create_vm, delete_vm, deploy_agent, restart_vm};
