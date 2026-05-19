//! Server configuration

use anyhow::Result;
use std::env;
use std::path::Path;

use crate::telemetry::TelemetryConfig;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub secrets_dir: String,
    pub heartbeat_timeout_secs: u64,
    pub telemetry: TelemetryConfig,
}

impl ServerConfig {
    pub fn from_env() -> Result<Self> {
        // Load from env file if exists
        let env_file = "/etc/agentic-sandbox/management.env";
        if Path::new(env_file).exists() {
            if let Ok(contents) = std::fs::read_to_string(env_file) {
                for line in contents.lines() {
                    let line = line.trim();
                    if !line.is_empty() && !line.starts_with('#') {
                        if let Some((key, value)) = line.split_once('=') {
                            env::set_var(key.trim(), value.trim());
                        }
                    }
                }
            }
        }

        Ok(Self {
            // Default to loopback per the documented single-host threat model
            // (memory: project_sandbox_deployment_default).
            //
            // gRPC binds here; WS uses port+1, HTTP uses port+2 — all three
            // derive from this IP via grpc_addr.ip() in main.rs. Loopback
            // cuts the cross-VM lateral path on virbr0 entirely: VMs cannot
            // reach 127.0.0.1 from their interfaces.
            //
            // Operators who explicitly want non-loopback exposure (multi-host
            // deployments, remote dashboards) set LISTEN_ADDR=0.0.0.0:8120
            // and SHOULD also configure TLS + bearer/mTLS auth — see #256
            // (WS auth) and #257 (TLS wiring). Until those land, non-loopback
            // exposure on virbr0 is a known cross-VM RCE vector.
            //
            // Refs: #256, #257
            listen_addr: env::var("LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:8120".to_string()),
            secrets_dir: env::var("SECRETS_DIR")
                .unwrap_or_else(|_| "/var/lib/agentic-sandbox/secrets".to_string()),
            heartbeat_timeout_secs: env::var("HEARTBEAT_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(90),
            telemetry: TelemetryConfig::from_env(),
        })
    }
}
