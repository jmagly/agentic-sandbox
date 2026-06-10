//! VM lifecycle control API
//!
//! REST endpoints for managing QEMU/KVM virtual machines via libvirt.

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tracing::{error, info, warn};
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

    #[error("libvirt unresponsive")]
    LibvirtUnresponsive { retry_after_seconds: u64 },
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
            VmError::LibvirtUnresponsive { .. } => "LIBVIRT_UNRESPONSIVE",
        }
    }

    pub(super) fn status_code(&self) -> StatusCode {
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
            VmError::LibvirtUnresponsive { .. } => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    pub(super) fn retry_after_seconds(&self) -> Option<u64> {
        match self {
            VmError::LibvirtUnresponsive {
                retry_after_seconds,
            } => Some(*retry_after_seconds),
            _ => None,
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

        let mut response = (status, body).into_response();
        if let Some(seconds) = self.retry_after_seconds() {
            if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }
        response
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

pub(super) const LIBVIRT_READ_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const LIBVIRT_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
const LIBVIRT_CIRCUIT_FAILURE_THRESHOLD: usize = 3;
const LIBVIRT_CIRCUIT_OPEN_SECONDS: u64 = 30;

struct LibvirtCircuit {
    consecutive_timeouts: AtomicUsize,
    open_until_epoch_secs: AtomicU64,
}

impl LibvirtCircuit {
    fn new() -> Self {
        Self {
            consecutive_timeouts: AtomicUsize::new(0),
            open_until_epoch_secs: AtomicU64::new(0),
        }
    }

    fn before_call(&self) -> Result<(), VmError> {
        let now = epoch_seconds();
        let open_until = self.open_until_epoch_secs.load(Ordering::Relaxed);
        if open_until > now {
            return Err(VmError::LibvirtUnresponsive {
                retry_after_seconds: open_until.saturating_sub(now).max(1),
            });
        }
        Ok(())
    }

    fn record_success(&self) {
        self.consecutive_timeouts.store(0, Ordering::Relaxed);
        self.open_until_epoch_secs.store(0, Ordering::Relaxed);
    }

    fn record_timeout(&self) -> u64 {
        let failures = self.consecutive_timeouts.fetch_add(1, Ordering::Relaxed) + 1;
        if failures >= LIBVIRT_CIRCUIT_FAILURE_THRESHOLD {
            let open_until = epoch_seconds() + LIBVIRT_CIRCUIT_OPEN_SECONDS;
            self.open_until_epoch_secs
                .store(open_until, Ordering::Relaxed);
            LIBVIRT_CIRCUIT_OPEN_SECONDS
        } else {
            1
        }
    }

    #[cfg(test)]
    fn reset_for_tests(&self) {
        self.consecutive_timeouts.store(0, Ordering::Relaxed);
        self.open_until_epoch_secs.store(0, Ordering::Relaxed);
    }
}

static LIBVIRT_CIRCUIT: OnceLock<LibvirtCircuit> = OnceLock::new();

fn libvirt_circuit() -> &'static LibvirtCircuit {
    LIBVIRT_CIRCUIT.get_or_init(LibvirtCircuit::new)
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Helper to connect to libvirt (public for vms_extended)
pub(super) fn connect_libvirt() -> Result<Connect, VmError> {
    Connect::open(Some(LIBVIRT_URI))
        .map_err(|e| VmError::ConnectionError(format!("Failed to connect to libvirt: {}", e)))
}

/// Run a synchronous libvirt operation on a blocking thread so the
/// calling async task is not parked while libvirt syscalls stall.
/// All HTTP handlers that touch libvirt must route through this helper;
/// a single unguarded call can wedge the entire HTTP worker (see the
/// post-incident notes in management/systemd/agentic-mgmt.service).
///
/// Logs every call's duration at INFO; warns at >1s, errors at >5s
/// (#188 Section A). Pre-degradation slowness is observable from
/// `mgmt.log` before the Axum-level timeout fires.
pub(super) async fn libvirt_blocking_with_timeout<F, R>(
    operation: &'static str,
    f: F,
    timeout_dur: Duration,
) -> Result<R, VmError>
where
    F: FnOnce() -> Result<R, VmError> + Send + 'static,
    R: Send + 'static,
{
    libvirt_circuit().before_call()?;

    let start = Instant::now();
    info!(
        operation,
        timeout_ms = timeout_dur.as_millis(),
        "libvirt RPC started"
    );
    let task = tokio::task::spawn_blocking(f);
    let result = match tokio::time::timeout(timeout_dur, task).await {
        Ok(joined) => {
            joined.map_err(|e| VmError::LibvirtError(format!("libvirt task join error: {}", e)))?
        }
        Err(_) => {
            let elapsed_ms = start.elapsed().as_millis();
            let retry_after_seconds = libvirt_circuit().record_timeout();
            error!(
                operation,
                elapsed_ms,
                timeout_ms = timeout_dur.as_millis(),
                retry_after_seconds,
                "libvirt RPC timed out"
            );
            return Err(VmError::LibvirtUnresponsive {
                retry_after_seconds,
            });
        }
    };
    let elapsed_ms = start.elapsed().as_millis();
    let ok = result.is_ok();
    match elapsed_ms {
        d if d >= 5000 => error!(
            operation,
            elapsed_ms = d,
            ok,
            "libvirt RPC completed very slowly (>5s)"
        ),
        d if d >= 1000 => warn!(
            operation,
            elapsed_ms = d,
            ok,
            "libvirt RPC completed slowly (>1s)"
        ),
        d => info!(operation, elapsed_ms = d, ok, "libvirt RPC completed"),
    }
    if ok {
        libvirt_circuit().record_success();
    }
    result
}

pub(super) async fn libvirt_read<F, R>(operation: &'static str, f: F) -> Result<R, VmError>
where
    F: FnOnce() -> Result<R, VmError> + Send + 'static,
    R: Send + 'static,
{
    libvirt_blocking_with_timeout(operation, f, LIBVIRT_READ_TIMEOUT).await
}

pub(super) async fn libvirt_write<F, R>(operation: &'static str, f: F) -> Result<R, VmError>
where
    F: FnOnce() -> Result<R, VmError> + Send + 'static,
    R: Send + 'static,
{
    libvirt_blocking_with_timeout(operation, f, LIBVIRT_WRITE_TIMEOUT).await
}

#[allow(dead_code)]
pub(super) async fn libvirt_blocking<F, R>(f: F) -> Result<R, VmError>
where
    F: FnOnce() -> Result<R, VmError> + Send + 'static,
    R: Send + 'static,
{
    libvirt_write("vms.legacy_blocking", f).await
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
    let registry = state.registry.clone();
    let vms = libvirt_read("vms.list", move || -> Result<Vec<VmInfo>, VmError> {
        let conn = connect_libvirt()?;
        let domains = conn
            .list_all_domains(0)
            .map_err(|e| VmError::LibvirtError(format!("Failed to list domains: {}", e)))?;

        let mut vms = Vec::new();
        for domain in domains {
            let name = domain
                .get_name()
                .map_err(|e| VmError::LibvirtError(format!("Failed to get domain name: {}", e)))?;

            if query.prefix != "*" && !name.starts_with(&query.prefix) {
                continue;
            }

            let vm_state = get_domain_state(&domain)?;
            let include = match query.state.as_str() {
                "running" => vm_state == VmState::Running,
                "stopped" => vm_state == VmState::Stopped,
                "all" => true,
                _ => true,
            };
            if !include {
                continue;
            }

            match extract_vm_info(&domain, &registry) {
                Ok(info) => vms.push(info),
                Err(e) => {
                    warn!(vm = %name, error = %e, "Failed to extract VM info");
                    continue;
                }
            }
        }
        Ok(vms)
    })
    .await?;

    let total = vms.len();
    Ok(Json(ListVmsResponse { vms, total }))
}

/// GET /api/v1/vms/{name} - Get VM details
pub async fn get_vm(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<VmDetails>, VmError> {
    let registry = state.registry.clone();
    let name_blk = name.clone();
    let reg_blk = registry.clone();
    let vm_info = libvirt_read("vms.get", move || -> Result<VmInfo, VmError> {
        let conn = connect_libvirt()?;
        let domain = get_domain(&conn, &name_blk)?;
        extract_vm_info(&domain, &reg_blk)
    })
    .await?;

    let agent_info = registry.get(&name).map(|agent| AgentConnectionInfo {
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
    let name_blk = name.clone();
    let already_running = libvirt_write("vms.start", move || -> Result<bool, VmError> {
        let conn = connect_libvirt()?;
        let domain = get_domain(&conn, &name_blk)?;
        let state = get_domain_state(&domain)?;
        if state == VmState::Running {
            return Ok(true);
        }
        domain
            .create()
            .map_err(|e| VmError::LibvirtError(format!("Failed to start VM: {}", e)))?;
        Ok(false)
    })
    .await?;

    if already_running {
        info!(vm = %name, "VM is already running");
        return Ok(Json(VmActionResponse {
            vm: VmActionVm {
                name,
                state: VmState::Running,
            },
            message: Some("VM is already running".to_string()),
        }));
    }

    info!(vm = %name, "VM started successfully");

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

/// Query for `POST /api/v1/vms/{name}/stop`. Both knobs are honored
/// (#169): `force=true` skips ACPI shutdown and goes straight to
/// libvirt's `destroy()`; `timeout=<seconds>` overrides the default
/// 15s graceful window before reporting back.
#[derive(Debug, Deserialize)]
pub struct StopVmQuery {
    #[serde(default)]
    pub force: bool,
    #[serde(default = "default_stop_timeout")]
    pub timeout: u64,
}

fn default_stop_timeout() -> u64 {
    15
}

/// POST /api/v1/vms/{name}/stop - Stop a VM
///
/// Default: graceful (ACPI) shutdown with a `?timeout=` window for the
/// guest to react. `?force=true` jumps straight to `destroy()` (the
/// libvirt force-kill); equivalent to `/destroy` but keeps the
/// `/stop` route uniform for clients that don't differentiate.
pub async fn stop_vm(
    Path(name): Path<String>,
    Query(q): Query<StopVmQuery>,
) -> Result<Json<VmActionResponse>, VmError> {
    let name_blk = name.clone();
    let force = q.force;
    let outcome = libvirt_write("vms.stop", move || -> Result<&'static str, VmError> {
        let conn = connect_libvirt()?;
        let domain = get_domain(&conn, &name_blk)?;
        let state = get_domain_state(&domain)?;
        if state == VmState::Stopped {
            return Ok("already_stopped");
        }
        if force {
            domain
                .destroy()
                .map_err(|e| VmError::LibvirtError(format!("Failed to force-stop VM: {}", e)))?;
            Ok("force_stopped")
        } else {
            domain
                .shutdown()
                .map_err(|e| VmError::LibvirtError(format!("Failed to shutdown VM: {}", e)))?;
            Ok("graceful_initiated")
        }
    })
    .await?;

    match outcome {
        "already_stopped" => {
            info!(vm = %name, "VM is already stopped");
            Ok(Json(VmActionResponse {
                vm: VmActionVm {
                    name,
                    state: VmState::Stopped,
                },
                message: Some("VM is already stopped".to_string()),
            }))
        }
        "force_stopped" => {
            info!(vm = %name, "VM force-stopped");
            events::add_libvirt_event(
                "vm.stopped",
                name.clone(),
                chrono::Utc::now(),
                Some("force".to_string()),
                None,
            )
            .await;
            Ok(Json(VmActionResponse {
                vm: VmActionVm {
                    name,
                    state: VmState::Stopped,
                },
                message: Some("Forced stop completed".to_string()),
            }))
        }
        _ => {
            // Graceful path. The `timeout` param is informational —
            // libvirt's shutdown() returns immediately and the guest
            // reacts asynchronously. We surface the timeout in the
            // message so callers see what window we promised them.
            info!(vm = %name, timeout = q.timeout, "VM shutdown initiated (graceful)");
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
                    state: VmState::Shutdown,
                },
                message: Some(format!(
                    "Graceful shutdown initiated (timeout {}s)",
                    q.timeout
                )),
            }))
        }
    }
}

/// POST /api/v1/vms/{name}:destroy - Force stop a VM
pub async fn destroy_vm(
    _: super::operator_auth::RequireAdmin,
    Path(name): Path<String>,
) -> Result<Json<VmActionResponse>, VmError> {
    let name_blk = name.clone();
    let was_running = libvirt_write("vms.destroy", move || -> Result<bool, VmError> {
        let conn = connect_libvirt()?;
        let domain = get_domain(&conn, &name_blk)?;
        let state = get_domain_state(&domain)?;
        if state != VmState::Stopped {
            domain
                .destroy()
                .map_err(|e| VmError::LibvirtError(format!("Failed to destroy VM: {}", e)))?;
            Ok(true)
        } else {
            Ok(false)
        }
    })
    .await?;

    if was_running {
        info!(vm = %name, "VM destroyed (force stop)");
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

    static LIBVIRT_TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

    fn libvirt_test_lock() -> &'static tokio::sync::Mutex<()> {
        LIBVIRT_TEST_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

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
        assert_eq!(
            VmError::LibvirtUnresponsive {
                retry_after_seconds: 30,
            }
            .error_code(),
            "LIBVIRT_UNRESPONSIVE"
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
        assert_eq!(
            VmError::LibvirtUnresponsive {
                retry_after_seconds: 30,
            }
            .status_code(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn test_libvirt_blocking_timeout_returns_503_error() {
        let _guard = libvirt_test_lock().lock().await;
        libvirt_circuit().reset_for_tests();
        let result = libvirt_blocking_with_timeout(
            "test.timeout",
            || {
                std::thread::sleep(Duration::from_millis(100));
                Ok::<_, VmError>(())
            },
            Duration::from_millis(10),
        )
        .await;

        assert!(matches!(result, Err(VmError::LibvirtUnresponsive { .. })));
        let response = result.unwrap_err().into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(response.headers().contains_key(header::RETRY_AFTER));
        libvirt_circuit().reset_for_tests();
    }

    #[tokio::test]
    async fn test_libvirt_circuit_opens_after_repeated_timeouts() {
        let _guard = libvirt_test_lock().lock().await;
        libvirt_circuit().reset_for_tests();
        for _ in 0..LIBVIRT_CIRCUIT_FAILURE_THRESHOLD {
            let _ = libvirt_blocking_with_timeout(
                "test.circuit_timeout",
                || {
                    std::thread::sleep(Duration::from_millis(100));
                    Ok::<_, VmError>(())
                },
                Duration::from_millis(10),
            )
            .await;
        }

        let result = libvirt_blocking_with_timeout(
            "test.circuit_open",
            || Ok::<_, VmError>(()),
            Duration::from_secs(1),
        )
        .await;

        assert!(matches!(result, Err(VmError::LibvirtUnresponsive { .. })));
        libvirt_circuit().reset_for_tests();
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
