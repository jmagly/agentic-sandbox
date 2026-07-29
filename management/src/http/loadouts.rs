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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_options: Option<LoadoutRuntimeOptions>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compatibility: Vec<LoadoutCompatibility>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<LoadoutGpuResources>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadoutGpuResources {
    #[serde(default)]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
}

/// Framework reference
#[derive(Debug, Clone, Serialize)]
pub struct FrameworkRef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoadoutRuntimeOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_strategy: Option<LoadoutLaunchStrategy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constraints: Option<LoadoutRuntimeConstraints>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadoutLaunchStrategy {
    #[serde(default = "default_cold_launch_mode")]
    pub mode: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub prefer_fast_start: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restore_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoadoutRuntimeConstraints {
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_vfio_fast_start: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadoutCompatibility {
    pub runtime_kind: String,
    pub provider: String,
    pub eligible: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<LoadoutCapabilityConstraint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_strategy: Option<LoadoutLaunchStrategy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fast_start_assets: Vec<LoadoutFastStartAsset>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadoutCapabilityConstraint {
    pub condition: String,
    pub excludes: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadoutFastStartAsset {
    pub id: String,
    pub provider: String,
    pub kind: String,
    pub state: String,
    pub capabilities: Vec<String>,
    pub reason: String,
}

fn default_cold_launch_mode() -> String {
    "cold".to_string()
}

fn is_false(value: &bool) -> bool {
    !*value
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
pub(crate) fn parse_loadout_file(path: &std::path::Path) -> Option<LoadoutInfo> {
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
        let gpu = r.get("gpu").and_then(|gpu| {
            let enabled = gpu
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let device = gpu
                .get("device")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let driver = gpu
                .get("driver")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if enabled || device.is_some() || driver.is_some() {
                Some(LoadoutGpuResources {
                    enabled,
                    device,
                    driver,
                })
            } else {
                None
            }
        });
        if cpus.is_some() || memory.is_some() || disk.is_some() || gpu.is_some() {
            Some(LoadoutResources {
                cpus,
                memory,
                disk,
                gpu,
            })
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
    let has_gpu = resources
        .as_ref()
        .and_then(|resources| resources.gpu.as_ref())
        .is_some_and(|gpu| gpu.enabled);
    let runtime_options = yaml
        .get("runtime_options")
        .and_then(|value| serde_yaml::from_value::<LoadoutRuntimeOptions>(value.clone()).ok());
    let compatibility = resolve_compatibility(runtime_options.as_ref(), has_gpu);

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
        runtime_options,
        compatibility,
    })
}

fn resolve_compatibility(
    runtime_options: Option<&LoadoutRuntimeOptions>,
    has_gpu: bool,
) -> Vec<LoadoutCompatibility> {
    let Some(options) = runtime_options else {
        return ["cloud-hypervisor", "libvirt"]
            .into_iter()
            .map(|provider| cold_vm_compatibility(provider))
            .collect();
    };

    let runtime_kind = options.kind.as_deref().unwrap_or("vm").to_string();
    let providers = match options
        .provider
        .as_deref()
        .filter(|provider| !provider.is_empty())
    {
        Some(provider) => vec![provider.to_string()],
        None if runtime_kind == "host" => vec!["host".to_string()],
        None if runtime_kind == "container" => vec!["docker".to_string()],
        None => vec!["cloud-hypervisor".to_string(), "libvirt".to_string()],
    };

    providers
        .into_iter()
        .map(|provider| compatibility_for_provider(options, &runtime_kind, &provider, has_gpu))
        .collect()
}

fn cold_vm_compatibility(provider: &str) -> LoadoutCompatibility {
    LoadoutCompatibility {
        runtime_kind: "vm".to_string(),
        provider: provider.to_string(),
        eligible: true,
        required_capabilities: Vec::new(),
        excluded_capabilities: Vec::new(),
        constraints: Vec::new(),
        launch_strategy: Some(LoadoutLaunchStrategy {
            mode: "cold".to_string(),
            prefer_fast_start: false,
            asset_ref: None,
            restore_mode: None,
        }),
        fast_start_assets: Vec::new(),
        reason: None,
    }
}

fn compatibility_for_provider(
    options: &LoadoutRuntimeOptions,
    runtime_kind: &str,
    provider: &str,
    has_gpu: bool,
) -> LoadoutCompatibility {
    let launch_strategy =
        options
            .launch_strategy
            .clone()
            .unwrap_or_else(|| LoadoutLaunchStrategy {
                mode: "cold".to_string(),
                prefer_fast_start: false,
                asset_ref: None,
                restore_mode: None,
            });
    let fast_start_capabilities = [
        "instance.snapshot",
        "instance.restore",
        "instance.fork",
        "warm_pool.manage",
    ];
    let vfio_required = has_gpu
        || options
            .required_capabilities
            .iter()
            .any(|capability| capability == "device.vfio");
    let mut eligible = true;
    let mut reason = None;
    let mut constraints = Vec::new();

    if vfio_required {
        constraints.push(LoadoutCapabilityConstraint {
            condition: "device.vfio".to_string(),
            excludes: fast_start_capabilities
                .iter()
                .map(|capability| (*capability).to_string())
                .collect(),
            reason: "VFIO-backed VMs cannot safely reuse snapshot, restore, fork, or warm-pool memory state".to_string(),
        });
        if launch_strategy.mode != "cold" {
            eligible = false;
            reason = Some(
                "runtime_options requests VFIO with a fast-start launch mode; use cold launch"
                    .to_string(),
            );
        }
    }

    if launch_strategy.mode != "cold"
        && launch_strategy
            .asset_ref
            .as_deref()
            .is_none_or(|asset| asset.trim().is_empty())
    {
        eligible = false;
        reason = Some("fast-start launch strategy is missing asset_ref".to_string());
    }

    let fast_start_assets = launch_strategy
        .asset_ref
        .as_ref()
        .filter(|asset| launch_strategy.mode != "cold" && !asset.trim().is_empty())
        .map(|asset| LoadoutFastStartAsset {
            id: asset.clone(),
            provider: provider.to_string(),
            kind: fast_start_asset_kind(provider, &launch_strategy.mode).to_string(),
            state: "degraded".to_string(),
            capabilities: capabilities_for_launch_mode(&launch_strategy.mode),
            reason: "declared by loadout; provider asset readiness must be verified before launch"
                .to_string(),
        })
        .into_iter()
        .collect();

    LoadoutCompatibility {
        runtime_kind: runtime_kind.to_string(),
        provider: provider.to_string(),
        eligible,
        required_capabilities: options.required_capabilities.clone(),
        excluded_capabilities: options.excluded_capabilities.clone(),
        constraints,
        launch_strategy: Some(launch_strategy),
        fast_start_assets,
        reason,
    }
}

fn fast_start_asset_kind(provider: &str, mode: &str) -> &'static str {
    match mode {
        "restore" if provider == "libvirt" => "checkpoint",
        "restore" => "snapshot",
        "fork" => "fork_base",
        "warm_pool" => "warm_pool",
        _ => "none",
    }
}

fn capabilities_for_launch_mode(mode: &str) -> Vec<String> {
    match mode {
        "restore" => vec!["instance.restore".to_string()],
        "fork" => vec!["instance.fork".to_string()],
        "warm_pool" => vec!["warm_pool.manage".to_string()],
        _ => Vec::new(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_profile(content: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().expect("temp profile");
        file.write_all(content.as_bytes()).expect("write profile");
        file
    }

    #[test]
    fn default_vm_loadout_reports_cold_compatibility_for_supported_providers() {
        let profile = write_profile(
            r#"
apiVersion: loadout/v1
kind: loadout
metadata:
  name: basic
  description: Basic VM
"#,
        );

        let loadout = parse_loadout_file(profile.path()).expect("loadout");
        assert_eq!(loadout.compatibility.len(), 2);
        assert!(loadout
            .compatibility
            .iter()
            .any(|entry| entry.provider == "cloud-hypervisor"
                && entry.eligible
                && entry
                    .launch_strategy
                    .as_ref()
                    .is_some_and(|strategy| strategy.mode == "cold")));
        assert!(loadout
            .compatibility
            .iter()
            .any(|entry| entry.provider == "libvirt" && entry.eligible));
    }

    #[test]
    fn runtime_options_reports_cloud_hypervisor_snapshot_asset() {
        let profile = write_profile(
            r#"
apiVersion: loadout/v1
kind: loadout
metadata:
  name: ch-restore
  description: CH restore
runtime_options:
  kind: vm
  provider: cloud-hypervisor
  required_capabilities: [instance.restore]
  launch_strategy:
    mode: restore
    prefer_fast_start: true
    asset_ref: ch-snapshot-agentic-dev
    restore_mode: copy
"#,
        );

        let loadout = parse_loadout_file(profile.path()).expect("loadout");
        let entry = loadout.compatibility.first().expect("compatibility");
        assert_eq!(entry.provider, "cloud-hypervisor");
        assert!(entry.eligible);
        assert_eq!(entry.fast_start_assets[0].id, "ch-snapshot-agentic-dev");
        assert_eq!(entry.fast_start_assets[0].kind, "snapshot");
        assert_eq!(entry.fast_start_assets[0].state, "degraded");
    }

    #[test]
    fn runtime_options_reports_cloud_hypervisor_warm_pool_asset() {
        let profile = write_profile(
            r#"
apiVersion: loadout/v1
kind: loadout
metadata:
  name: ch-warm
  description: Cloud Hypervisor warm pool
runtime_options:
  kind: vm
  provider: cloud-hypervisor
  required_capabilities: [warm_pool.manage]
  launch_strategy:
    mode: warm_pool
    prefer_fast_start: true
    asset_ref: ch-pool-sdlc-team
"#,
        );

        let loadout = parse_loadout_file(profile.path()).expect("loadout");
        let entry = loadout.compatibility.first().expect("compatibility");
        assert_eq!(entry.provider, "cloud-hypervisor");
        assert!(entry.eligible);
        assert_eq!(entry.fast_start_assets[0].kind, "warm_pool");
        assert_eq!(
            entry.fast_start_assets[0].capabilities,
            vec!["warm_pool.manage"]
        );
    }

    #[test]
    fn runtime_options_reports_libvirt_checkpoint_asset() {
        let profile = write_profile(
            r#"
apiVersion: loadout/v1
kind: loadout
metadata:
  name: libvirt-restore
  description: libvirt checkpoint restore
runtime_options:
  kind: vm
  provider: libvirt
  required_capabilities: [instance.restore]
  launch_strategy:
    mode: restore
    prefer_fast_start: true
    asset_ref: libvirt-checkpoint-sdlc-team
    restore_mode: copy
"#,
        );

        let loadout = parse_loadout_file(profile.path()).expect("loadout");
        let entry = loadout.compatibility.first().expect("compatibility");
        assert_eq!(entry.provider, "libvirt");
        assert!(entry.eligible);
        assert_eq!(
            entry.fast_start_assets[0].id,
            "libvirt-checkpoint-sdlc-team"
        );
        assert_eq!(entry.fast_start_assets[0].kind, "checkpoint");
        assert_eq!(
            entry.fast_start_assets[0].capabilities,
            vec!["instance.restore"]
        );
    }

    #[test]
    fn vfio_fast_start_loadout_is_marked_ineligible_with_safe_reason() {
        let profile = write_profile(
            r#"
apiVersion: loadout/v1
kind: loadout
metadata:
  name: bad-vfio
  description: invalid VFIO fast start
resources:
  gpu:
    enabled: true
    device: "0000:41:00.0"
    driver: vfio-pci
runtime_options:
  kind: vm
  provider: cloud-hypervisor
  required_capabilities: [device.vfio]
  excluded_capabilities: [instance.snapshot, instance.restore, instance.fork, warm_pool.manage]
  launch_strategy:
    mode: restore
    asset_ref: ch-snapshot-gpu
  constraints:
    allow_vfio_fast_start: false
"#,
        );

        let loadout = parse_loadout_file(profile.path()).expect("loadout");
        let entry = loadout.compatibility.first().expect("compatibility");
        assert!(!entry.eligible);
        assert!(entry
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("VFIO")));
        assert_eq!(entry.constraints[0].condition, "device.vfio");
        assert!(entry.constraints[0]
            .excludes
            .iter()
            .any(|capability| capability == "instance.restore"));
    }

    #[test]
    fn fast_start_without_asset_ref_is_marked_ineligible() {
        let profile = write_profile(
            r#"
apiVersion: loadout/v1
kind: loadout
metadata:
  name: missing-asset
  description: restore without asset
runtime_options:
  kind: vm
  provider: cloud-hypervisor
  required_capabilities: [instance.restore]
  launch_strategy:
    mode: restore
"#,
        );

        let loadout = parse_loadout_file(profile.path()).expect("loadout");
        let entry = loadout.compatibility.first().expect("compatibility");
        assert!(!entry.eligible);
        assert!(entry.fast_start_assets.is_empty());
        assert!(entry
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("asset_ref")));
    }
}
