//! Server configuration

use anyhow::{Context, Result};
use std::env;
use std::path::{Path, PathBuf};

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
            secrets_dir: match env::var("SECRETS_DIR") {
                Ok(value) => value,
                Err(_) => default_secrets_dir()?.to_string_lossy().to_string(),
            },
            heartbeat_timeout_secs: env::var("HEARTBEAT_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(90),
            telemetry: TelemetryConfig::from_env(),
        })
    }
}

pub fn default_secrets_dir() -> Result<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from);
    default_secrets_dir_for(std::env::consts::OS, home.as_deref())
}

pub fn default_secrets_dir_for(target_os: &str, home_dir: Option<&Path>) -> Result<PathBuf> {
    if target_os == "macos" {
        let home_dir = home_dir
            .context("macOS management requires HOME for private per-user TLS and secret state")?;
        return Ok(home_dir
            .join("Library")
            .join("Application Support")
            .join("io.aiwg.agentic-sandbox")
            .join("secrets"));
    }
    Ok(PathBuf::from("/var/lib/agentic-sandbox/secrets"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn darwin_secrets_default_is_private_per_user_application_support() {
        assert_eq!(
            default_secrets_dir_for("macos", Some(Path::new("/Users/synthetic"))).unwrap(),
            PathBuf::from(
                "/Users/synthetic/Library/Application Support/io.aiwg.agentic-sandbox/secrets"
            )
        );
    }

    #[test]
    fn darwin_secrets_default_fails_closed_without_home() {
        let error = default_secrets_dir_for("macos", None).unwrap_err();
        assert!(error.to_string().contains("requires HOME"));
    }

    #[test]
    fn linux_secrets_default_is_unchanged() {
        assert_eq!(
            default_secrets_dir_for("linux", None).unwrap(),
            PathBuf::from("/var/lib/agentic-sandbox/secrets")
        );
    }
}
