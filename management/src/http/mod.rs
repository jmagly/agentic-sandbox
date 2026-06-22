//! HTTP server for web dashboard
//!
//! Serves static files and REST API endpoints for the control plane UI.

pub mod admin_v2;
pub mod aiwg_proxy;
pub mod bootstrap_enrollment;
pub mod compat_v1;
pub mod container_images;
pub mod containers;
pub mod credentials;
pub mod dispatch;
pub mod events;
pub mod health;
pub mod hitl;
pub mod idempotency;
pub mod loadout_registry;
pub mod loadouts;
pub mod logs;
pub mod operations;
pub mod operator_auth;
pub mod orchestrate;
pub mod rate_limit;
pub mod server;
pub mod sessions;
pub mod ssh_gateway;
pub mod startup_profiles;
pub mod storage;
pub mod tasks;
pub mod tls_listener;
pub mod uds;
pub mod validation;
pub mod vms;
mod vms_extended;

pub use operations::OperationStore;
pub use server::HttpServer;
pub use vms_extended::{create_vm, delete_vm, deploy_agent, restart_vm};
