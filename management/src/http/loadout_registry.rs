//! Loadout registry endpoint
//!
//! Serves the merged framework/provider/init-script registry from
//! registry.json + any operator extension files in extensions/*.json.

use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::warn;

const REGISTRY_FILE: &str = "images/qemu/loadouts/registry.json";
const EXTENSIONS_DIR: &str = "images/qemu/loadouts/extensions";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoadoutRegistry {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub frameworks: Vec<FrameworkDef>,
    #[serde(default)]
    pub providers: Vec<ProviderDef>,
    #[serde(default)]
    pub init_scripts: Vec<InitScriptDef>,
    #[serde(default)]
    pub presets: Vec<PresetDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameworkDef {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub reserved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderDef {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub layer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitScriptDef {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub layers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetDef {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_ubuntu")]
    pub init: String,
    #[serde(default)]
    pub aiwg: AiwgCompositionDef,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AiwgCompositionDef {
    #[serde(default)]
    pub frameworks: Vec<String>,
    #[serde(default)]
    pub providers: Vec<String>,
}

fn default_ubuntu() -> String {
    "ubuntu".to_string()
}

fn find_registry_file() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;

    let candidates = [
        cwd.join("..").join(REGISTRY_FILE),
        cwd.join(REGISTRY_FILE),
        PathBuf::from("/opt/agentic-sandbox").join(REGISTRY_FILE),
    ];

    candidates.into_iter().find(|p| p.is_file())
}

fn find_extensions_dir() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;

    let candidates = [
        cwd.join("..").join(EXTENSIONS_DIR),
        cwd.join(EXTENSIONS_DIR),
        PathBuf::from("/opt/agentic-sandbox").join(EXTENSIONS_DIR),
    ];

    candidates.into_iter().find(|p| p.is_dir())
}

/// Load the registry from disk, merging extension files.
pub fn load_registry() -> LoadoutRegistry {
    let Some(registry_path) = find_registry_file() else {
        warn!("registry.json not found, returning empty registry");
        return LoadoutRegistry::default();
    };

    let content = match std::fs::read_to_string(&registry_path) {
        Ok(c) => c,
        Err(e) => {
            warn!(path = %registry_path.display(), error = %e, "Failed to read registry.json");
            return LoadoutRegistry::default();
        }
    };

    let mut registry: LoadoutRegistry = match serde_json::from_str(&content) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "Failed to parse registry.json");
            return LoadoutRegistry::default();
        }
    };

    // Merge extension files
    if let Some(ext_dir) = find_extensions_dir() {
        if let Ok(entries) = std::fs::read_dir(&ext_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                // Skip the example file
                if path.file_name().and_then(|n| n.to_str()) == Some("example.json") {
                    continue;
                }
                match std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|c| serde_json::from_str::<LoadoutRegistry>(&c).ok())
                {
                    Some(ext) => {
                        registry.frameworks.extend(ext.frameworks);
                        registry.providers.extend(ext.providers);
                        registry.init_scripts.extend(ext.init_scripts);
                        registry.presets.extend(ext.presets);
                    }
                    None => {
                        warn!(path = %path.display(), "Failed to parse extension file, skipping");
                    }
                }
            }
        }
    }

    registry
}

/// GET /api/v1/loadout/registry — serve merged registry
pub async fn get_registry() -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let registry = load_registry();
    if registry.frameworks.is_empty() && registry.providers.is_empty() {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Registry not available — registry.json not found"})),
        ));
    }
    Ok(Json(registry))
}
