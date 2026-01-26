//! CLI configuration

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct CliConfig {
    pub server_url: String,
    pub default_profile: String,
    pub agentshare_path: Option<String>,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            server_url: "http://localhost:8120".to_string(),
            default_profile: "basic".to_string(),
            agentshare_path: Some("/mnt/inbox".to_string()),
        }
    }
}

impl CliConfig {
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("agentic-sandbox").join("config.json"))
    }

    pub fn load() -> Self {
        Self::config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    #[allow(dead_code)]
    pub fn save(&self) -> Result<()> {
        if let Some(path) = Self::config_path() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        }
        Ok(())
    }
}
