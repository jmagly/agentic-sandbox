//! VM lifecycle control API
//!
//! REST endpoints for managing QEMU/KVM virtual machines via libvirt.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;
use tracing::{info, warn};
use virt::connect::Connect;
use virt::domain::Domain;
use virt::sys;

use super::events;
use super::server::AppState;

/// Error types for VM operations
#[derive(Debug, Error)]
pub enum VmError {
    #[error("VM not found: {0}")]
    NotFound(String),

    #[error("VM is already running: {0}")]
    AlreadyRunning(String),

    #[error("VM is already stopped: {0}")]
    AlreadyStopped(String),

    #[error("VM already exists: {0}")]
    AlreadyExists(String),

    #[error("Cannot delete running VM: {0}")]
    CannotDeleteRunning(String),

    #[error("VM is not running: {0}")]
    NotRunning(String),

    #[error("Invalid VM name: {0}")]
    InvalidVmName(String),

    #[error("Provisioning error: {0}")]
    ProvisioningError(String),

    #[error("libvirt error: {0}")]
    LibvirtError(String),

    #[error("Connection error: {0}")]
    ConnectionError(String),

    #[error("Validation error: {0}")]
    ValidationError(String),
}

impl VmError {
    fn error_code(&self) -> &'static str {
        match self {
            VmError::NotFound(_) => "VM_NOT_FOUND",
            VmError::AlreadyRunning(_) => "VM_RUNNING",
            VmError::AlreadyStopped(_) => "VM_STOPPED",
            VmError::AlreadyExists(_) => "VM_ALREADY_EXISTS",
            VmError::CannotDeleteRunning(_) => "VM_RUNNING",
            VmError::NotRunning(_) => "VM_NOT_RUNNING",
            VmError::InvalidVmName(_) => "INVALID_VM_NAME",
            VmError::ProvisioningError(_) => "PROVISIONING_ERROR",
            VmError::LibvirtError(_) => "LIBVIRT_ERROR",
            VmError::ConnectionError(_) => "LIBVIRT_ERROR",
            VmError::ValidationError(_) => "VALIDATION_ERROR",
        }
    }

    fn status_code(&self) -> StatusCode {
        match self {
            VmError::NotFound(_) => StatusCode::NOT_FOUND,
            VmError::AlreadyRunning(_) => StatusCode::OK,
            VmError::AlreadyStopped(_) => StatusCode::OK,
            VmError::AlreadyExists(_) => StatusCode::CONFLICT,
            VmError::CannotDeleteRunning(_) => StatusCode::CONFLICT,
            VmError::NotRunning(_) => StatusCode::CONFLICT,
            VmError::InvalidVmName(_) => StatusCode::BAD_REQUEST,
            VmError::ProvisioningError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            VmError::LibvirtError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            VmError::ConnectionError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            VmError::ValidationError(_) => StatusCode::BAD_REQUEST,
        }
    }
}

impl IntoResponse for VmError {
    fn into_response(self) -> axum::response::Response {
        let status = self.status_code();
        let code = self.error_code();
        let message = self.to_string();

        let body = Json(ErrorResponse {
            error: ErrorDetail {
                code: code.to_string(),
                message,
            },
        });

        (status, body).into_response()
    }
}

#[derive(Serialize)]
struct ErrorResponse {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: String,
    message: String,
}

/// VM state as returned by libvirt
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VmState {
    Running,
    Stopped,
    Paused,
    Shutdown,
    Crashed,
    Suspended,
    Unknown,
}

impl From<sys::virDomainState> for VmState {
    fn from(state: sys::virDomainState) -> Self {
        match state {
            sys::VIR_DOMAIN_RUNNING => VmState::Running,
            sys::VIR_DOMAIN_BLOCKED => VmState::Running,
            sys::VIR_DOMAIN_PAUSED => VmState::Paused,
            sys::VIR_DOMAIN_SHUTDOWN => VmState::Shutdown,
            sys::VIR_DOMAIN_SHUTOFF => VmState::Stopped,
            sys::VIR_DOMAIN_CRASHED => VmState::Crashed,
            sys::VIR_DOMAIN_PMSUSPENDED => VmState::Suspended,
            _ => VmState::Unknown,
        }
    }
}

/// VM information summary
#[derive(Debug, Clone, Serialize)]
pub struct VmInfo {
    pub name: String,
    pub state: VmState,
    pub uuid: String,
    pub vcpus: u32,
    pub memory_mb: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_seconds: Option<u64>,
}

/// VM detailed information
#[derive(Debug, Clone, Serialize)]
pub struct VmDetails {
    pub name: String,
    pub state: VmState,
    pub uuid: String,
    pub vcpus: u32,
    pub memory_mb: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentConnectionInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentConnectionInfo {
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

/// Query parameters for listing VMs
#[derive(Debug, Deserialize)]
pub struct ListVmsQuery {
    /// Filter by state: running, stopped, or all
    #[serde(default = "default_state_filter")]
    pub state: String,
    /// Filter by name prefix (default: agent-, use * for all)
    #[serde(default = "default_prefix_filter")]
    pub prefix: String,
}

fn default_state_filter() -> String {
    "all".to_string()
}

fn default_prefix_filter() -> String {
    DEFAULT_VM_PREFIX.to_string()
}

/// Response for list VMs
#[derive(Debug, Serialize)]
pub struct ListVmsResponse {
    pub vms: Vec<VmInfo>,
    pub total: usize,
}

/// Response for VM action operations
#[derive(Debug, Serialize)]
pub struct VmActionResponse {
    pub vm: VmActionVm,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VmActionVm {
    pub name: String,
    pub state: VmState,
}

/// libvirt connection URI
const LIBVIRT_URI: &str = "qemu:///system";

/// Default VM prefix filter
const DEFAULT_VM_PREFIX: &str = "agent-";

/// Helper to connect to libvirt (public for vms_extended)
pub(super) fn connect_libvirt() -> Result<Connect, VmError> {
    Connect::open(Some(LIBVIRT_URI))
        .map_err(|e| VmError::ConnectionError(format!("Failed to connect to libvirt: {}", e)))
}

/// Helper to get domain by name (public for vms_extended)
pub(super) fn get_domain(conn: &Connect, name: &str) -> Result<Domain, VmError> {
    Domain::lookup_by_name(conn, name).map_err(|_| VmError::NotFound(name.to_string()))
}

/// Helper to get domain state (public for vms_extended)
pub(super) fn get_domain_state(domain: &Domain) -> Result<VmState, VmError> {
    domain
        .get_state()
        .map(|(state, _)| VmState::from(state))
        .map_err(|e| VmError::LibvirtError(format!("Failed to get domain state: {}", e)))
}

/// Helper to extract VM info from domain
fn extract_vm_info(
    domain: &Domain,
    registry: &Arc<crate::registry::AgentRegistry>,
) -> Result<VmInfo, VmError> {
    let name = domain
        .get_name()
        .map_err(|e| VmError::LibvirtError(format!("Failed to get domain name: {}", e)))?;

    let uuid = domain
        .get_uuid_string()
        .map_err(|e| VmError::LibvirtError(format!("Failed to get domain UUID: {}", e)))?;

    let state = get_domain_state(domain)?;

    let info = domain
        .get_info()
        .map_err(|e| VmError::LibvirtError(format!("Failed to get domain info: {}", e)))?;

    // Get IP address from agent registry if available
    let ip_address = registry
        .get(&name)
        .map(|agent| agent.registration.ip_address.clone());

    // Calculate uptime if running
    let uptime_seconds = if state == VmState::Running {
        // libvirt doesn't directly expose uptime, would need to track start times
        None
    } else {
        None
    };

    Ok(VmInfo {
        name,
        state,
        uuid,
        vcpus: info.nr_virt_cpu,
        memory_mb: info.max_mem / 1024, // KiB to MiB
        ip_address,
        uptime_seconds,
    })
}

/// GET /api/v1/vms - List all VMs
pub async fn list_vms(
    State(state): State<AppState>,
    Query(query): Query<ListVmsQuery>,
) -> Result<Json<ListVmsResponse>, VmError> {
    let conn = connect_libvirt()?;

    // Get all domains
    let domains = conn
        .list_all_domains(0)
        .map_err(|e| VmError::LibvirtError(format!("Failed to list domains: {}", e)))?;

    let mut vms = Vec::new();

    for domain in domains {
        let name = domain
            .get_name()
            .map_err(|e| VmError::LibvirtError(format!("Failed to get domain name: {}", e)))?;

        // Filter by prefix (use * for all VMs)
        if query.prefix != "*" && !name.starts_with(&query.prefix) {
            continue;
        }

        // Get state for filtering
        let vm_state = get_domain_state(&domain)?;

        // Apply state filter
        let include = match query.state.as_str() {
            "running" => vm_state == VmState::Running,
            "stopped" => vm_state == VmState::Stopped,
            "all" => true,
            _ => true, // Default to all for invalid filter
        };

        if !include {
            continue;
        }

        match extract_vm_info(&domain, &state.registry) {
            Ok(info) => vms.push(info),
            Err(e) => {
                warn!(vm = %name, error = %e, "Failed to extract VM info");
                continue;
            }
        }
    }

    let total = vms.len();

    Ok(Json(ListVmsResponse { vms, total }))
}

/// GET /api/v1/vms/{name} - Get VM details
pub async fn get_vm(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<VmDetails>, VmError> {
    let conn = connect_libvirt()?;
    let domain = get_domain(&conn, &name)?;

    let vm_info = extract_vm_info(&domain, &state.registry)?;

    // Check if agent is connected
    let agent_info = state.registry.get(&name).map(|agent| AgentConnectionInfo {
        connected: true,
        connected_at: Some(agent.connected_at.timestamp_millis()),
        hostname: Some(agent.registration.hostname.clone()),
    });

    Ok(Json(VmDetails {
        name: vm_info.name,
        state: vm_info.state,
        uuid: vm_info.uuid,
        vcpus: vm_info.vcpus,
        memory_mb: vm_info.memory_mb,
        ip_address: vm_info.ip_address,
        uptime_seconds: vm_info.uptime_seconds,
        agent: agent_info,
    }))
}

/// POST /api/v1/vms/{name}:start - Start a VM
pub async fn start_vm(Path(name): Path<String>) -> Result<Json<VmActionResponse>, VmError> {
    let conn = connect_libvirt()?;
    let domain = get_domain(&conn, &name)?;

    let state = get_domain_state(&domain)?;

    // Check if already running (idempotent)
    if state == VmState::Running {
        info!(vm = %name, "VM is already running");
        return Ok(Json(VmActionResponse {
            vm: VmActionVm {
                name,
                state: VmState::Running,
            },
            message: Some("VM is already running".to_string()),
        }));
    }

    // Start the domain
    domain
        .create()
        .map_err(|e| VmError::LibvirtError(format!("Failed to start VM: {}", e)))?;

    info!(vm = %name, "VM started successfully");

    // Emit event
    events::add_libvirt_event(
        "vm.started",
        name.clone(),
        chrono::Utc::now(),
        Some("manual".to_string()),
        None,
    )
    .await;

    Ok(Json(VmActionResponse {
        vm: VmActionVm {
            name,
            state: VmState::Running,
        },
        message: None,
    }))
}

/// POST /api/v1/vms/{name}:stop - Gracefully stop a VM
pub async fn stop_vm(Path(name): Path<String>) -> Result<Json<VmActionResponse>, VmError> {
    let conn = connect_libvirt()?;
    let domain = get_domain(&conn, &name)?;

    let state = get_domain_state(&domain)?;

    // Check if already stopped (idempotent)
    if state == VmState::Stopped {
        info!(vm = %name, "VM is already stopped");
        return Ok(Json(VmActionResponse {
            vm: VmActionVm {
                name,
                state: VmState::Stopped,
            },
            message: Some("VM is already stopped".to_string()),
        }));
    }

    // Initiate graceful shutdown (ACPI)
    domain
        .shutdown()
        .map_err(|e| VmError::LibvirtError(format!("Failed to shutdown VM: {}", e)))?;

    info!(vm = %name, "VM shutdown initiated (graceful)");

    // Emit event
    events::add_libvirt_event(
        "vm.stopped",
        name.clone(),
        chrono::Utc::now(),
        Some("shutdown".to_string()),
        None,
    )
    .await;

    Ok(Json(VmActionResponse {
        vm: VmActionVm {
            name,
            state: VmState::Shutdown, // Transitioning to stopped
        },
        message: Some("Graceful shutdown initiated".to_string()),
    }))
}

/// POST /api/v1/vms/{name}:destroy - Force stop a VM
pub async fn destroy_vm(Path(name): Path<String>) -> Result<Json<VmActionResponse>, VmError> {
    let conn = connect_libvirt()?;
    let domain = get_domain(&conn, &name)?;

    let state = get_domain_state(&domain)?;

    // Force destroy (immediate termination)
    if state != VmState::Stopped {
        domain
            .destroy()
            .map_err(|e| VmError::LibvirtError(format!("Failed to destroy VM: {}", e)))?;

        info!(vm = %name, "VM destroyed (force stop)");

        // Emit event
        events::add_libvirt_event(
            "vm.stopped",
            name.clone(),
            chrono::Utc::now(),
            Some("destroyed".to_string()),
            None,
        )
        .await;
    } else {
        info!(vm = %name, "VM is already stopped");
    }

    Ok(Json(VmActionResponse {
        vm: VmActionVm {
            name,
            state: VmState::Stopped,
        },
        message: Some("VM destroyed".to_string()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vm_state_serialization() {
        let state = VmState::Running;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, r#""running""#);

        let state = VmState::Stopped;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, r#""stopped""#);
    }

    #[test]
    fn test_vm_state_from_libvirt() {
        assert_eq!(VmState::from(sys::VIR_DOMAIN_RUNNING), VmState::Running);
        assert_eq!(VmState::from(sys::VIR_DOMAIN_SHUTOFF), VmState::Stopped);
        assert_eq!(VmState::from(sys::VIR_DOMAIN_PAUSED), VmState::Paused);
        assert_eq!(VmState::from(sys::VIR_DOMAIN_CRASHED), VmState::Crashed);
    }

    #[test]
    fn test_vm_error_codes() {
        assert_eq!(
            VmError::NotFound("test".to_string()).error_code(),
            "VM_NOT_FOUND"
        );
        assert_eq!(
            VmError::AlreadyRunning("test".to_string()).error_code(),
            "VM_RUNNING"
        );
        assert_eq!(
            VmError::AlreadyStopped("test".to_string()).error_code(),
            "VM_STOPPED"
        );
        assert_eq!(
            VmError::LibvirtError("test".to_string()).error_code(),
            "LIBVIRT_ERROR"
        );
        assert_eq!(
            VmError::AlreadyExists("test".to_string()).error_code(),
            "VM_ALREADY_EXISTS"
        );
        assert_eq!(
            VmError::InvalidVmName("test".to_string()).error_code(),
            "INVALID_VM_NAME"
        );
    }

    #[test]
    fn test_vm_error_status_codes() {
        assert_eq!(
            VmError::NotFound("test".to_string()).status_code(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            VmError::AlreadyRunning("test".to_string()).status_code(),
            StatusCode::OK
        );
        assert_eq!(
            VmError::LibvirtError("test".to_string()).status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            VmError::AlreadyExists("test".to_string()).status_code(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            VmError::InvalidVmName("test".to_string()).status_code(),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn test_default_state_filter() {
        assert_eq!(default_state_filter(), "all");
    }

    #[test]
    fn test_list_vms_query_deserialization() {
        let json = r#"{"state":"running"}"#;
        let query: ListVmsQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.state, "running");

        // Test default
        let json = r#"{}"#;
        let query: ListVmsQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.state, "all");
    }

    #[test]
    fn test_vm_info_serialization() {
        let info = VmInfo {
            name: "agent-01".to_string(),
            state: VmState::Running,
            uuid: "a1b2c3d4-e5f6-7890-abcd-ef1234567890".to_string(),
            vcpus: 4,
            memory_mb: 8192,
            ip_address: Some("192.168.122.201".to_string()),
            uptime_seconds: Some(3600),
        };

        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["name"], "agent-01");
        assert_eq!(json["state"], "running");
        assert_eq!(json["vcpus"], 4);
        assert_eq!(json["memory_mb"], 8192);
    }

    #[test]
    fn test_vm_action_response_serialization() {
        let response = VmActionResponse {
            vm: VmActionVm {
                name: "agent-01".to_string(),
                state: VmState::Running,
            },
            message: Some("VM started".to_string()),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["vm"]["name"], "agent-01");
        assert_eq!(json["vm"]["state"], "running");
        assert_eq!(json["message"], "VM started");
    }
}
