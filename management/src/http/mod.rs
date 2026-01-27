//! HTTP server for web dashboard
//!
//! Serves static files and REST API endpoints for the control plane UI.

mod server;

pub use server::HttpServer;
