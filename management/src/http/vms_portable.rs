//! Fail-closed VM API used by management builds without Linux VM support.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::server::AppState;

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
    pub(super) fn status_code(&self) -> StatusCode {
        StatusCode::NOT_IMPLEMENTED
    }
    pub(super) fn retry_after_seconds(&self) -> Option<u64> {
        None
    }
}

impl IntoResponse for VmError {
    fn into_response(self) -> axum::response::Response {
        (StatusCode::NOT_IMPLEMENTED, Json(serde_json::json!({
            "error": {"code": "VM_RUNTIME_UNAVAILABLE", "message": "Linux VM support is not available in this management build"}
        }))).into_response()
    }
}

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

#[derive(Debug, Clone, Serialize)]
pub struct AgentConnectionInfo {
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VmDetails {
    pub name: String,
    pub state: VmState,
    pub uuid: String,
    pub vcpus: u32,
    pub memory_mb: u64,
    pub ip_address: Option<String>,
    pub uptime_seconds: Option<u64>,
    pub agent: Option<AgentConnectionInfo>,
}

#[derive(Debug, Deserialize)]
pub struct ListVmsQuery {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub prefix: String,
}
#[derive(Debug, Serialize)]
pub struct ListVmsResponse {
    pub vms: Vec<VmInfo>,
    pub total: usize,
}
#[derive(Debug, Serialize)]
pub struct VmActionResponse {
    pub vm: VmActionVm,
    pub message: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct VmActionVm {
    pub name: String,
    pub state: VmState,
}
#[derive(Debug, Deserialize)]
pub struct StopVmQuery {
    #[serde(default)]
    pub force: bool,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}
fn default_timeout() -> u64 {
    15
}

#[cfg(test)]
pub(crate) fn libvirt_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn unavailable<T>() -> Result<T, VmError> {
    Err(VmError::ConnectionError(
        "Linux VM support is not available in this management build".into(),
    ))
}
pub async fn list_vms(
    State(_): State<AppState>,
    Query(_): Query<ListVmsQuery>,
) -> Result<Json<ListVmsResponse>, VmError> {
    unavailable()
}
pub async fn get_vm(
    State(_): State<AppState>,
    Path(_): Path<String>,
) -> Result<Json<VmDetails>, VmError> {
    unavailable()
}
pub async fn start_vm(Path(_): Path<String>) -> Result<Json<VmActionResponse>, VmError> {
    unavailable()
}
pub async fn stop_vm(
    Path(_): Path<String>,
    Query(_): Query<StopVmQuery>,
) -> Result<Json<VmActionResponse>, VmError> {
    unavailable()
}
pub async fn destroy_vm(
    _: super::operator_auth::RequireAdmin,
    Path(_): Path<String>,
) -> Result<Json<VmActionResponse>, VmError> {
    unavailable()
}
