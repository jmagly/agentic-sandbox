//! Fail-closed extended VM routes for non-Linux management builds.

use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use super::{server::AppState, vms::VmError};

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct AiwgComposition {
    #[serde(default)]
    pub frameworks: Vec<String>,
    #[serde(default)]
    pub providers: Vec<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Composition {
    #[serde(default)]
    pub init: String,
    #[serde(default)]
    pub aiwg: AiwgComposition,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CreateVmRequest {
    pub name: String,
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub loadout: String,
    #[serde(default)]
    pub composition: Option<Composition>,
    #[serde(default)]
    pub vcpus: u32,
    #[serde(default)]
    pub memory_mb: u64,
    #[serde(default)]
    pub disk_gb: u64,
    #[serde(default)]
    pub agentshare: bool,
    #[serde(default)]
    pub start: bool,
    #[serde(default)]
    pub ssh_key: String,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct RestartVmRequest {
    #[serde(default)]
    pub mode: RestartMode,
    #[serde(default)]
    pub timeout_seconds: u64,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RestartMode {
    #[default]
    Graceful,
    Hard,
}
#[derive(Debug, Deserialize)]
pub struct DeleteVmQuery {
    #[serde(default)]
    pub delete_disk: bool,
    #[serde(default)]
    pub force: bool,
}
#[derive(Debug, Serialize)]
pub struct DeleteVmResponse {
    pub deleted: bool,
    pub name: String,
    pub disk_deleted: bool,
}

fn unavailable<T>() -> Result<T, VmError> {
    Err(VmError::ConnectionError(
        "Linux VM support is not available in this management build".into(),
    ))
}
pub async fn create_vm(
    State(_): State<AppState>,
    Json(_): Json<CreateVmRequest>,
) -> Result<impl IntoResponse, VmError> {
    unavailable::<Json<serde_json::Value>>()
}
pub async fn delete_vm(
    _: super::operator_auth::RequireAdmin,
    Path(_): Path<String>,
    Query(_): Query<DeleteVmQuery>,
) -> Result<Json<DeleteVmResponse>, VmError> {
    unavailable()
}
pub async fn restart_vm(
    State(_): State<AppState>,
    Path(_): Path<String>,
    Json(_): Json<RestartVmRequest>,
) -> Result<impl IntoResponse, VmError> {
    unavailable::<Json<serde_json::Value>>()
}
pub async fn deploy_agent(
    State(_): State<AppState>,
    Path(_): Path<String>,
) -> Result<impl IntoResponse, VmError> {
    unavailable::<Json<serde_json::Value>>()
}
