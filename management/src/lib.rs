//! Agentic Management Server Library
//!
//! Core modules for the management server, exposed for testing.

pub mod agent_message_dispatch;
pub mod agent_pty_bridge;
pub mod aiwg_serve;
pub mod audit;
pub mod auth;
pub mod bootstrap_enrollment;
pub mod config;
pub mod dispatch;
pub mod docker_runtime;
pub mod grpc;
pub mod grpc_local_ca;
pub mod hitl;
pub mod http;
pub mod identity;
pub mod orchestrator;
pub mod output;
pub mod prompt_detector;
pub mod registry;
pub mod screen_state;
pub mod session;
pub mod telemetry;
pub mod transport_identity;
pub mod ws;

pub mod proto {
    tonic::include_proto!("agentic.sandbox.v1");
}
