//! Extended VM operations - Create, Delete, Restart
//!
//! Phase 2 operations for full CRUD functionality

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{error, info, warn};
use virt::connect::Connect;
use virt::domain::Domain;

use super::events;
use super::operations::{CreateOperationResponse, Operation, OperationStore, OperationType};
use super::server::AppState;
use super::vms::{get_domain, get_domain_state, VmError, VmState};

/// VM name validation regex (must be agent-*)
const VM_NAME_PATTERN: &str = r"^agent-[a-z0-9-]+$";

/// Default path to provision-vm.sh (relative to project root)
const PROVISION_SCRIPT_PATH: &str = "images/qemu/provision-vm.sh";

/// Request body for VM creation
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CreateVmRequest {
    pub name: String,
    #[serde(default = "default_profile")]
    pub profile: String,
    /// Loadout manifest path (e.g., "profiles/claude-only.yaml").
    /// Mutually exclusive with `profile` — set profile to empty string when using loadout.
    #[serde(default)]
    pub loadout: String,
    #[serde(default = "default_vcpus")]
    pub vcpus: u32,
    #[serde(default = "default_memory")]
    pub memory_mb: u64,
    #[serde(default = "default_disk")]
    pub disk_gb: u64,
    #[serde(default = "default_agentshare")]
    pub agentshare: bool,
    #[serde(default = "default_start")]
    pub start: bool,
    /// SSH public key path (defaults to ~/.ssh/id_ed25519.pub or ~/.ssh/id_rsa.pub)
    #[serde(default = "default_ssh_key")]
    pub ssh_key: String,
}

fn default_profile() -> String {
    "agentic-dev".to_string()
}

fn default_vcpus() -> u32 {
    4
}

fn default_memory() -> u64 {
    8192
}

fn default_disk() -> u64 {
    50
}

fn default_agentshare() -> bool {
    true
}

fn default_start() -> bool {
    true
}

fn default_ssh_key() -> String {
    // Try common SSH key locations (including project-specific keys)
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let candidates = [
        format!("{home}/.ssh/agentic_ed25519.pub"), // Project-specific key
        format!("{home}/.ssh/vm_ed25519.pub"),      // VM-specific key
        format!("{home}/.ssh/id_ed25519.pub"),
        format!("{home}/.ssh/id_rsa.pub"),
        format!("{home}/.ssh/id_ecdsa.pub"),
    ];

    for path in &candidates {
        if std::path::Path::new(path).exists() {
            return path.clone();
        }
    }

    // Return empty string if no key found (will trigger error in provision)
    String::new()
}

/// Request body for VM restart
#[derive(Debug, Deserialize, Serialize)]
pub struct RestartVmRequest {
    #[serde(default = "default_restart_mode")]
    pub mode: RestartMode,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RestartMode {
    Graceful,
    Hard,
}

fn default_restart_mode() -> RestartMode {
    RestartMode::Graceful
}

fn default_timeout() -> u64 {
    60
}

/// Query parameters for VM deletion
#[derive(Debug, Deserialize)]
pub struct DeleteVmQuery {
    #[serde(default)]
    pub delete_disk: bool,
    #[serde(default)]
    pub force: bool,
}

/// Response for VM deletion
#[derive(Debug, Serialize)]
pub struct DeleteVmResponse {
    pub deleted: bool,
    pub name: String,
    pub disk_deleted: bool,
}

/// libvirt connection URI
const LIBVIRT_URI: &str = "qemu:///system";

/// Helper to connect to libvirt
fn connect_libvirt() -> Result<Connect, VmError> {
    Connect::open(Some(LIBVIRT_URI))
        .map_err(|e| VmError::ConnectionError(format!("Failed to connect to libvirt: {}", e)))
}

/// Validate VM name format
fn validate_vm_name(name: &str) -> Result<(), VmError> {
    let re = Regex::new(VM_NAME_PATTERN).unwrap();
    if !re.is_match(name) {
        return Err(VmError::InvalidVmName(format!(
            "VM name '{}' must match pattern '{}'",
            name, VM_NAME_PATTERN
        )));
    }
    Ok(())
}

/// Check if VM already exists in libvirt
fn vm_exists(conn: &Connect, name: &str) -> bool {
    Domain::lookup_by_name(conn, name).is_ok()
}

/// Find the provision-vm.sh script
fn find_provision_script() -> Result<PathBuf, VmError> {
    // Try relative to current working directory first
    let cwd = std::env::current_dir().map_err(|e| {
        VmError::ProvisioningError(format!("Failed to get current directory: {}", e))
    })?;

    // Try ../../images/qemu/provision-vm.sh (from management/)
    let script_path = cwd.join("..").join(PROVISION_SCRIPT_PATH);
    if script_path.exists() {
        return Ok(script_path);
    }

    // Try direct path (from project root)
    let script_path = cwd.join(PROVISION_SCRIPT_PATH);
    if script_path.exists() {
        return Ok(script_path);
    }

    // Try absolute path for production deployment
    let script_path = PathBuf::from("/opt/agentic-sandbox").join(PROVISION_SCRIPT_PATH);
    if script_path.exists() {
        return Ok(script_path);
    }

    Err(VmError::ProvisioningError(format!(
        "provision-vm.sh not found. Tried: {}/../../{}, {}/{}, /opt/agentic-sandbox/{}",
        cwd.display(),
        PROVISION_SCRIPT_PATH,
        cwd.display(),
        PROVISION_SCRIPT_PATH,
        PROVISION_SCRIPT_PATH
    )))
}

/// POST /api/v1/vms - Create a new VM
pub async fn create_vm(
    State(state): State<AppState>,
    Json(req): Json<CreateVmRequest>,
) -> Result<impl IntoResponse, VmError> {
    // Validate VM name
    validate_vm_name(&req.name)?;

    // Validate mutual exclusivity of profile and loadout
    let has_non_default_profile = !req.profile.is_empty() && req.profile != "agentic-dev";
    let has_loadout = !req.loadout.is_empty();
    if has_loadout && has_non_default_profile {
        return Err(VmError::ValidationError(
            "Cannot specify both 'profile' and 'loadout' — they are mutually exclusive".to_string(),
        ));
    }

    // Check for conflicts
    let conn = connect_libvirt()?;
    if vm_exists(&conn, &req.name) {
        return Err(VmError::AlreadyExists(req.name.clone()));
    }

    // Get operation store
    let store = state
        .operation_store
        .as_ref()
        .ok_or_else(|| VmError::ProvisioningError("Operation store not available".to_string()))?;

    // Create operation
    let operation = Operation::new(OperationType::VmCreate, req.name.clone());
    let op_id = store.insert(operation.clone());

    info!(
        vm_name = %req.name,
        operation_id = %op_id,
        "Creating VM with async provisioning"
    );

    // Spawn async provisioning task
    let vm_name = req.name.clone();
    let req_clone = req.clone();
    let op_store = store.clone();
    let secret_store = state.secret_store.clone();
    tokio::spawn(async move {
        match provision_vm_async(&vm_name, &req_clone, &op_store, &op_id).await {
            Ok(()) => {
                // Reload secrets after successful provisioning (new agent secret was created)
                if let Some(ref secrets) = secret_store {
                    if let Err(e) = secrets.reload() {
                        warn!(vm_name = %vm_name, error = %e, "Failed to reload secrets after provisioning");
                    } else {
                        info!(vm_name = %vm_name, "Secrets reloaded after provisioning");
                    }
                }
            }
            Err(e) => {
                error!(vm_name = %vm_name, operation_id = %op_id, error = %e, "Provisioning failed");
                op_store.mark_failed(&op_id, e.to_string());

                // Emit failure event
                events::add_libvirt_event(
                    "vm.provisioning.failed",
                    vm_name.clone(),
                    chrono::Utc::now(),
                    Some("provision_error".to_string()),
                    None,
                )
                .await;
            }
        }
    });

    // Emit provisioning started event
    events::add_libvirt_event(
        "vm.provisioning.started",
        req.name.clone(),
        chrono::Utc::now(),
        Some("api".to_string()),
        None,
    )
    .await;

    Ok((
        StatusCode::ACCEPTED,
        Json(CreateOperationResponse {
            operation: operation.to_response(),
            vm: None,
        }),
    ))
}

/// Async VM provisioning task
async fn provision_vm_async(
    vm_name: &str,
    req: &CreateVmRequest,
    store: &OperationStore,
    op_id: &str,
) -> Result<(), VmError> {
    use super::operations::OperationState;

    // Update to running
    store.update_state(op_id, OperationState::Running);
    store.update_progress(op_id, 10);

    // Find provision script
    let script_path = find_provision_script()?;
    info!(script_path = %script_path.display(), "Found provision script");

    // Validate SSH key
    if req.ssh_key.is_empty() {
        return Err(VmError::ProvisioningError(
            "No SSH public key found. Please specify ssh_key in the request or ensure ~/.ssh/id_ed25519.pub exists.".to_string()
        ));
    }

    if !std::path::Path::new(&req.ssh_key).exists() {
        return Err(VmError::ProvisioningError(format!(
            "SSH key not found at: {}",
            req.ssh_key
        )));
    }

    // Build command
    let mut cmd = Command::new(&script_path);
    if !req.loadout.is_empty() {
        cmd.arg("--loadout").arg(&req.loadout);
    } else {
        cmd.arg("--profile").arg(&req.profile);
    }
    cmd.arg("--cpus")
        .arg(req.vcpus.to_string())
        .arg("--memory")
        .arg(format!("{}M", req.memory_mb))
        .arg("--disk")
        .arg(format!("{}G", req.disk_gb))
        .arg("--ssh-key")
        .arg(&req.ssh_key);

    if req.agentshare {
        cmd.arg("--agentshare");
    }

    if req.start {
        cmd.arg("--start");
    }

    cmd.arg(vm_name);

    // Configure stdio
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    info!(
        vm_name = %vm_name,
        command = ?cmd,
        "Spawning provision-vm.sh"
    );

    store.update_progress(op_id, 20);

    // Execute provisioning
    let output = cmd.output().await.map_err(|e| {
        VmError::ProvisioningError(format!("Failed to spawn provision-vm.sh: {}", e))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(VmError::ProvisioningError(format!(
            "Provisioning failed with exit code {}: stderr={}, stdout={}",
            output.status.code().unwrap_or(-1),
            stderr,
            stdout
        )));
    }

    store.update_progress(op_id, 90);

    // Verify VM was created
    let conn = connect_libvirt()?;
    if !vm_exists(&conn, vm_name) {
        return Err(VmError::ProvisioningError(format!(
            "VM {} not found after provisioning",
            vm_name
        )));
    }

    info!(vm_name = %vm_name, "Provisioning completed successfully");

    // Mark complete
    let result = serde_json::json!({
        "vm": {
            "name": vm_name,
            "state": if req.start { "running" } else { "stopped" }
        }
    });

    store.mark_completed(op_id, Some(result));

    // Emit completion event
    events::add_libvirt_event(
        "vm.provisioning.completed",
        vm_name.to_string(),
        chrono::Utc::now(),
        Some("success".to_string()),
        None,
    )
    .await;

    Ok(())
}

/// DELETE /api/v1/vms/{name} - Delete a VM
pub async fn delete_vm(
    Path(name): Path<String>,
    Query(query): Query<DeleteVmQuery>,
) -> Result<Json<DeleteVmResponse>, VmError> {
    let conn = connect_libvirt()?;
    let domain = get_domain(&conn, &name)?;

    let state = get_domain_state(&domain)?;

    // Check if running and force not set
    if state == VmState::Running && !query.force {
        return Err(VmError::CannotDeleteRunning(name));
    }

    // Force destroy if running
    if state == VmState::Running {
        info!(vm = %name, "Force destroying running VM before deletion");
        domain
            .destroy()
            .map_err(|e| VmError::LibvirtError(format!("Failed to destroy VM: {}", e)))?;

        events::add_libvirt_event(
            "vm.stopped",
            name.clone(),
            chrono::Utc::now(),
            Some("force_destroy_before_delete".to_string()),
            None,
        )
        .await;
    }

    // Get disk path before undefining
    let disk_path = if query.delete_disk {
        // Extract disk path from domain XML
        let xml = domain
            .get_xml_desc(0)
            .map_err(|e| VmError::LibvirtError(format!("Failed to get domain XML: {}", e)))?;

        // Simple extraction - look for qcow2 disk path
        extract_disk_path_from_xml(&xml)
    } else {
        None
    };

    // Undefine the domain
    domain
        .undefine()
        .map_err(|e| VmError::LibvirtError(format!("Failed to undefine VM: {}", e)))?;

    info!(vm = %name, "VM undefined from libvirt");

    events::add_libvirt_event(
        "vm.undefined",
        name.clone(),
        chrono::Utc::now(),
        Some("api".to_string()),
        None,
    )
    .await;

    // Delete disk if requested
    let mut disk_deleted = false;
    if let Some(disk_path) = disk_path {
        if std::path::Path::new(&disk_path).exists() {
            match std::fs::remove_file(&disk_path) {
                Ok(_) => {
                    info!(vm = %name, disk_path = %disk_path, "Deleted VM disk");
                    disk_deleted = true;
                }
                Err(e) => {
                    warn!(vm = %name, disk_path = %disk_path, error = %e, "Failed to delete disk");
                }
            }
        }
    }

    Ok(Json(DeleteVmResponse {
        deleted: true,
        name,
        disk_deleted,
    }))
}

/// Extract disk path from domain XML
fn extract_disk_path_from_xml(xml: &str) -> Option<String> {
    // Simple regex-based extraction for .qcow2 files
    let re = regex::Regex::new(r"<source file='([^']+\.qcow2)'").ok()?;
    re.captures(xml)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
}

/// POST /api/v1/vms/{name}:restart - Restart a VM
pub async fn restart_vm(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<RestartVmRequest>,
) -> Result<impl IntoResponse, VmError> {
    let conn = connect_libvirt()?;
    let domain = get_domain(&conn, &name)?;

    let vm_state = get_domain_state(&domain)?;

    // VM must be running to restart
    if vm_state != VmState::Running {
        return Err(VmError::NotRunning(name));
    }

    // Get operation store
    let store = state
        .operation_store
        .as_ref()
        .ok_or_else(|| VmError::ProvisioningError("Operation store not available".to_string()))?;

    // Create operation
    let operation = Operation::new(OperationType::VmRestart, name.clone());
    let op_id = store.insert(operation.clone());

    info!(
        vm_name = %name,
        operation_id = %op_id,
        mode = ?req.mode,
        "Restarting VM"
    );

    // Spawn async restart task
    let vm_name = name.clone();
    let op_store = store.clone();
    tokio::spawn(async move {
        if let Err(e) = restart_vm_async(&vm_name, &req, &op_store, &op_id).await {
            error!(vm_name = %vm_name, operation_id = %op_id, error = %e, "Restart failed");
            op_store.mark_failed(&op_id, e.to_string());
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(CreateOperationResponse {
            operation: operation.to_response(),
            vm: None,
        }),
    ))
}

/// Async VM restart task
async fn restart_vm_async(
    vm_name: &str,
    req: &RestartVmRequest,
    store: &OperationStore,
    op_id: &str,
) -> Result<(), VmError> {
    use super::operations::OperationState;

    store.update_state(op_id, OperationState::Running);
    store.update_progress(op_id, 10);

    let conn = connect_libvirt()?;
    let domain = get_domain(&conn, vm_name)?;

    // Stop phase
    match req.mode {
        RestartMode::Graceful => {
            info!(vm = %vm_name, "Initiating graceful shutdown");
            domain
                .shutdown()
                .map_err(|e| VmError::LibvirtError(format!("Failed to shutdown VM: {}", e)))?;

            events::add_libvirt_event(
                "vm.stopped",
                vm_name.to_string(),
                chrono::Utc::now(),
                Some("restart_graceful".to_string()),
                None,
            )
            .await;

            store.update_progress(op_id, 30);

            // Wait for VM to stop (with timeout)
            let timeout = tokio::time::Duration::from_secs(req.timeout_seconds);
            let start = tokio::time::Instant::now();

            loop {
                if start.elapsed() > timeout {
                    warn!(vm = %vm_name, "Graceful shutdown timeout, forcing destroy");
                    domain.destroy().map_err(|e| {
                        VmError::LibvirtError(format!("Failed to destroy VM: {}", e))
                    })?;
                    break;
                }

                let state = get_domain_state(&domain)?;
                if state == VmState::Stopped {
                    break;
                }

                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                store.update_progress(
                    op_id,
                    30 + ((start.elapsed().as_secs() * 20) / req.timeout_seconds) as u8,
                );
            }
        }
        RestartMode::Hard => {
            info!(vm = %vm_name, "Force destroying VM");
            domain
                .destroy()
                .map_err(|e| VmError::LibvirtError(format!("Failed to destroy VM: {}", e)))?;

            events::add_libvirt_event(
                "vm.stopped",
                vm_name.to_string(),
                chrono::Utc::now(),
                Some("restart_hard".to_string()),
                None,
            )
            .await;
        }
    }

    store.update_progress(op_id, 60);

    // Start phase
    info!(vm = %vm_name, "Starting VM");
    domain
        .create()
        .map_err(|e| VmError::LibvirtError(format!("Failed to start VM: {}", e)))?;

    events::add_libvirt_event(
        "vm.started",
        vm_name.to_string(),
        chrono::Utc::now(),
        Some("restart".to_string()),
        None,
    )
    .await;

    store.update_progress(op_id, 100);

    // Mark complete
    let result = serde_json::json!({
        "vm": {
            "name": vm_name,
            "state": "running"
        }
    });

    store.mark_completed(op_id, Some(result));

    info!(vm = %vm_name, "Restart completed successfully");
    Ok(())
}

/// POST /api/v1/vms/{name}/deploy-agent - Deploy agent binary to VM
pub async fn deploy_agent(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, VmError> {
    // Validate VM name
    validate_vm_name(&name)?;

    // Check VM exists and is running
    let conn = connect_libvirt()?;
    let domain = get_domain(&conn, &name)?;
    let vm_state = get_domain_state(&domain)?;

    if vm_state != VmState::Running {
        return Err(VmError::NotRunning(name));
    }

    // Get operation store
    let store = state
        .operation_store
        .as_ref()
        .ok_or_else(|| VmError::ProvisioningError("Operation store not available".to_string()))?;

    // Create operation
    let operation = Operation::new(OperationType::VmCreate, name.clone()); // Reuse VmCreate type
    let op_id = store.insert(operation.clone());

    info!(
        vm_name = %name,
        operation_id = %op_id,
        "Deploying agent to VM"
    );

    // Spawn async deploy task
    let vm_name = name.clone();
    let op_store = store.clone();
    let secret_store = state.secret_store.clone();
    tokio::spawn(async move {
        match deploy_agent_async(&vm_name, &op_store, &op_id).await {
            Ok(()) => {
                // Reload secrets after deployment (agent secret should already exist)
                if let Some(ref secrets) = secret_store {
                    if let Err(e) = secrets.reload() {
                        warn!(vm_name = %vm_name, error = %e, "Failed to reload secrets after deploy");
                    }
                }
            }
            Err(e) => {
                error!(vm_name = %vm_name, operation_id = %op_id, error = %e, "Agent deployment failed");
                op_store.mark_failed(&op_id, e.to_string());
            }
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(CreateOperationResponse {
            operation: operation.to_response(),
            vm: None,
        }),
    ))
}

/// Async agent deployment task
async fn deploy_agent_async(
    vm_name: &str,
    store: &OperationStore,
    op_id: &str,
) -> Result<(), VmError> {
    use super::operations::OperationState;
    use tokio::process::Command;

    store.update_state(op_id, OperationState::Running);
    store.update_progress(op_id, 10);

    // Find the deploy script
    let script_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("scripts/deploy-agent.sh");

    if !script_path.exists() {
        return Err(VmError::ProvisioningError(format!(
            "Deploy script not found: {}",
            script_path.display()
        )));
    }

    info!(vm = %vm_name, script = %script_path.display(), "Running deploy-agent.sh");
    store.update_progress(op_id, 20);

    // Run the deploy script
    let output = Command::new("bash")
        .arg(&script_path)
        .arg(vm_name)
        .output()
        .await
        .map_err(|e| VmError::ProvisioningError(format!("Failed to run deploy script: {}", e)))?;

    store.update_progress(op_id, 80);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        error!(
            vm = %vm_name,
            exit_code = ?output.status.code(),
            stdout = %stdout,
            stderr = %stderr,
            "Deploy script failed"
        );
        return Err(VmError::ProvisioningError(format!(
            "Deploy script failed: {}",
            stderr
        )));
    }

    store.update_progress(op_id, 100);

    let result = serde_json::json!({
        "vm": {
            "name": vm_name,
            "agent_deployed": true
        }
    });

    store.mark_completed(op_id, Some(result));
    info!(vm = %vm_name, "Agent deployed successfully");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_vm_name_valid() {
        assert!(validate_vm_name("agent-01").is_ok());
        assert!(validate_vm_name("agent-test").is_ok());
        assert!(validate_vm_name("agent-dev-01").is_ok());
        assert!(validate_vm_name("agent-a").is_ok());
    }

    #[test]
    fn test_validate_vm_name_invalid() {
        assert!(validate_vm_name("vm-01").is_err());
        assert!(validate_vm_name("agent").is_err());
        assert!(validate_vm_name("agent-").is_err());
        assert!(validate_vm_name("agent-01_test").is_err());
        assert!(validate_vm_name("agent-01.test").is_err());
        assert!(validate_vm_name("AGENT-01").is_err());
    }

    #[test]
    fn test_create_vm_request_defaults() {
        let json = r#"{"name":"agent-01"}"#;
        let req: CreateVmRequest = serde_json::from_str(json).unwrap();

        assert_eq!(req.name, "agent-01");
        assert_eq!(req.profile, "agentic-dev");
        assert_eq!(req.loadout, "");
        assert_eq!(req.vcpus, 4);
        assert_eq!(req.memory_mb, 8192);
        assert_eq!(req.disk_gb, 50);
        assert!(req.agentshare);
        assert!(req.start);
    }

    #[test]
    fn test_create_vm_request_custom() {
        let json = r#"{
            "name":"agent-custom",
            "profile":"basic",
            "vcpus":2,
            "memory_mb":4096,
            "disk_gb":20,
            "agentshare":false,
            "start":false
        }"#;
        let req: CreateVmRequest = serde_json::from_str(json).unwrap();

        assert_eq!(req.name, "agent-custom");
        assert_eq!(req.profile, "basic");
        assert_eq!(req.loadout, "");
        assert_eq!(req.vcpus, 2);
        assert_eq!(req.memory_mb, 4096);
        assert_eq!(req.disk_gb, 20);
        assert!(!req.agentshare);
        assert!(!req.start);
    }

    #[test]
    fn test_create_vm_request_with_loadout() {
        let json = r#"{
            "name":"agent-claude",
            "profile":"",
            "loadout":"profiles/claude-only.yaml"
        }"#;
        let req: CreateVmRequest = serde_json::from_str(json).unwrap();

        assert_eq!(req.name, "agent-claude");
        assert_eq!(req.profile, "");
        assert_eq!(req.loadout, "profiles/claude-only.yaml");
    }

    #[test]
    fn test_restart_mode_serialization() {
        let mode = RestartMode::Graceful;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, r#""graceful""#);

        let mode = RestartMode::Hard;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, r#""hard""#);
    }

    #[test]
    fn test_restart_vm_request_defaults() {
        let json = r#"{}"#;
        let req: RestartVmRequest = serde_json::from_str(json).unwrap();

        assert_eq!(req.mode, RestartMode::Graceful);
        assert_eq!(req.timeout_seconds, 60);
    }

    #[test]
    fn test_restart_vm_request_custom() {
        let json = r#"{"mode":"hard","timeout_seconds":30}"#;
        let req: RestartVmRequest = serde_json::from_str(json).unwrap();

        assert_eq!(req.mode, RestartMode::Hard);
        assert_eq!(req.timeout_seconds, 30);
    }

    #[test]
    fn test_delete_vm_response_serialization() {
        let response = DeleteVmResponse {
            deleted: true,
            name: "agent-01".to_string(),
            disk_deleted: false,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["deleted"], true);
        assert_eq!(json["name"], "agent-01");
        assert_eq!(json["disk_deleted"], false);
    }

    #[test]
    fn test_extract_disk_path_from_xml() {
        let xml = r#"
            <domain>
                <devices>
                    <disk type='file'>
                        <source file='/var/lib/libvirt/images/agent-01.qcow2'/>
                    </disk>
                </devices>
            </domain>
        "#;

        let path = extract_disk_path_from_xml(xml);
        assert_eq!(
            path,
            Some("/var/lib/libvirt/images/agent-01.qcow2".to_string())
        );
    }

    #[test]
    fn test_extract_disk_path_no_match() {
        let xml = r#"<domain></domain>"#;
        let path = extract_disk_path_from_xml(xml);
        assert_eq!(path, None);
    }
}
