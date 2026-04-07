//! Loadout profile listing endpoint
//!
//! Scans loadout profile YAML files and serves metadata via REST API.

use axum::{extract::Query, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, warn};

/// Default path to loadout profiles (relative to project root)
const PROFILES_DIR: &str = "images/qemu/loadouts/profiles";

/// Response for GET /api/v1/loadouts
#[derive(Debug, Serialize)]
pub struct LoadoutsResponse {
    pub loadouts: Vec<LoadoutInfo>,
}

/// Individual loadout profile metadata
#[derive(Debug, Clone, Serialize)]
pub struct LoadoutInfo {
    pub name: String,
    pub path: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complexity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<LoadoutResources>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ai_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frameworks: Vec<FrameworkRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extends: Vec<String>,
}

/// Resource configuration from loadout
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadoutResources {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk: Option<String>,
}

/// Framework reference
#[derive(Debug, Clone, Serialize)]
pub struct FrameworkRef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
}

/// Query parameters for filtering loadouts
#[derive(Debug, Deserialize)]
pub struct LoadoutQuery {
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub complexity: Option<String>,
}

/// Find the profiles directory
fn find_profiles_dir() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;

    // Try ../images/qemu/loadouts/profiles (from management/)
    let path = cwd.join("..").join(PROFILES_DIR);
    if path.is_dir() {
        return Some(path);
    }

    // Try direct path (from project root)
    let path = cwd.join(PROFILES_DIR);
    if path.is_dir() {
        return Some(path);
    }

    // Try absolute path for production
    let path = PathBuf::from("/opt/agentic-sandbox").join(PROFILES_DIR);
    if path.is_dir() {
        return Some(path);
    }

    None
}

/// Parse a single loadout YAML file into LoadoutInfo
fn parse_loadout_file(path: &std::path::Path) -> Option<LoadoutInfo> {
    let content = std::fs::read_to_string(path).ok()?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&content).ok()?;

    let metadata = yaml.get("metadata")?;
    let name = metadata.get("name")?.as_str()?.to_string();
    let description = metadata
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let labels = metadata.get("labels");
    let category = labels
        .and_then(|l| l.get("category"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let complexity = labels
        .and_then(|l| l.get("complexity"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Parse resources
    let resources = yaml.get("resources").and_then(|r| {
        let cpus = r.get("cpus").and_then(|v| v.as_u64()).map(|v| v as u32);
        let memory = r
            .get("memory")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let disk = r
            .get("disk")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if cpus.is_some() || memory.is_some() || disk.is_some() {
            Some(LoadoutResources { cpus, memory, disk })
        } else {
            None
        }
    });

    // Parse network mode
    let network_mode = yaml
        .get("network")
        .and_then(|n| n.get("mode"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Parse AI tools (collect enabled tool names)
    let mut ai_tools = Vec::new();
    if let Some(tools) = yaml.get("ai_tools") {
        if let Some(mapping) = tools.as_mapping() {
            for (key, val) in mapping {
                if let Some(tool_name) = key.as_str() {
                    let enabled = val
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if enabled {
                        ai_tools.push(tool_name.to_string());
                    }
                }
            }
        }
    }

    // Parse frameworks
    let mut frameworks = Vec::new();
    if let Some(aiwg) = yaml.get("aiwg") {
        if let Some(fw_list) = aiwg.get("frameworks").and_then(|f| f.as_sequence()) {
            for fw in fw_list {
                if let Some(fw_name) = fw.get("name").and_then(|v| v.as_str()) {
                    let providers = fw
                        .get("providers")
                        .and_then(|p| p.as_sequence())
                        .map(|seq| {
                            seq.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    frameworks.push(FrameworkRef {
                        name: fw_name.to_string(),
                        providers,
                    });
                }
            }
        }
    }

    // Parse extends
    let extends = yaml
        .get("extends")
        .and_then(|e| e.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    // Build relative path for the response
    let file_name = path.file_name()?.to_str()?;
    let rel_path = format!("profiles/{}", file_name);

    Some(LoadoutInfo {
        name,
        path: rel_path,
        description,
        category,
        complexity,
        resources,
        network_mode,
        ai_tools,
        frameworks,
        extends,
    })
}

/// GET /api/v1/loadouts/:name - Get a single loadout profile by name
pub async fn get_loadout(
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let profiles_dir = find_profiles_dir().ok_or_else(|| {
        warn!("Loadout profiles directory not found");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Loadout profiles directory not found"})),
        )
    })?;

    // Try <name>.yaml
    let path = profiles_dir.join(format!("{}.yaml", name));
    if let Some(loadout) = parse_loadout_file(&path) {
        return Ok(Json(loadout));
    }

    // Try exact filename match (in case caller passed "profiles/foo.yaml")
    let bare = std::path::Path::new(&name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&name);
    let path2 = profiles_dir.join(format!("{}.yaml", bare));
    if let Some(loadout) = parse_loadout_file(&path2) {
        return Ok(Json(loadout));
    }

    Err((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": format!("Loadout '{}' not found", name)})),
    ))
}

/// GET /api/v1/loadouts - List available loadout profiles
pub async fn list_loadouts(
    Query(query): Query<LoadoutQuery>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let profiles_dir = find_profiles_dir().ok_or_else(|| {
        warn!("Loadout profiles directory not found");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Loadout profiles directory not found"})),
        )
    })?;

    debug!(dir = %profiles_dir.display(), "Scanning loadout profiles");

    let entries = std::fs::read_dir(&profiles_dir).map_err(|e| {
        warn!(error = %e, "Failed to read profiles directory");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to read profiles: {}", e)})),
        )
    })?;

    let mut loadouts: Vec<LoadoutInfo> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
                parse_loadout_file(&path)
            } else {
                None
            }
        })
        .collect();

    // Apply filters
    if let Some(ref category) = query.category {
        loadouts.retain(|l| l.category.as_deref() == Some(category));
    }
    if let Some(ref complexity) = query.complexity {
        loadouts.retain(|l| l.complexity.as_deref() == Some(complexity));
    }

    // Sort by name for consistent ordering
    loadouts.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(Json(LoadoutsResponse { loadouts }))
}
