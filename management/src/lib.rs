//! Agentic Management Server Library
//!
//! Core modules for the management server, exposed for testing.

pub mod audit;
pub mod auth;
pub mod config;
pub mod dispatch;
pub mod grpc;
pub mod http;
pub mod orchestrator;
pub mod output;
pub mod registry;
pub mod telemetry;
pub mod ws;

pub mod proto {
    tonic::include_proto!("agentic.sandbox.v1");
}
