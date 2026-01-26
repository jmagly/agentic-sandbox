//! Server configuration

use anyhow::Result;
use std::env;
use std::path::Path;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub secrets_dir: String,
    pub heartbeat_timeout_secs: u64,
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
            listen_addr: env::var("LISTEN_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:8120".to_string()),
            secrets_dir: env::var("SECRETS_DIR")
                .unwrap_or_else(|_| "/etc/agentic-sandbox/secrets".to_string()),
            heartbeat_timeout_secs: env::var("HEARTBEAT_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(90),
        })
    }
}
