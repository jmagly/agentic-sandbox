use crate::celld::{
    auth::{AuthError, PreviousKeyFile, RequestVerifier, SignedRequest},
    diagnose_fleet, plan_upgrade, preflight_bucket, BucketPreflightEvidence, CellCommand,
    CelldClient, CelldConfig, CelldFleetManifest, CelldStatus, EffectLedger, EffectLedgerError,
    EffectRecord, EffectStatus, FleetDiagnoseRequest, WorkerBundleManifest,
};
use async_trait::async_trait;
use axum::{
    body::Bytes,
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path as FsPath, PathBuf},
    sync::Arc,
    time::Duration,
};

use super::operator_auth::MtlsIdentity;
use super::operator_auth::RequireAdmin;
use super::server::AppState;

const EFFECT_CALLBACK_PATH: &str = "/api/v2/celld/effects";
const QUALIFICATION_GATE_SCHEMA: &str = "agentic-sandbox.celld-dispatch-gate/v1";
const QUALIFICATION_GATE_ENV: &str = "AGENTIC_CELLD_QUALIFICATION_DISPATCH_GATE_DIR";
const QUALIFICATION_GATE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum QualificationDispatchPhase {
    DuringDispatch,
    AfterDispatch,
}

impl QualificationDispatchPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::DuringDispatch => "during_dispatch",
            Self::AfterDispatch => "after_dispatch",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QualificationGateRequest {
    schema_version: String,
    operation_id_sha256: String,
    phase: QualificationDispatchPhase,
}

#[derive(Clone)]
struct QualificationDispatchGate {
    root: PathBuf,
}

impl QualificationDispatchGate {
    fn from_env() -> Result<Option<Self>, String> {
        let Some(root) = env::var_os(QUALIFICATION_GATE_ENV).map(PathBuf::from) else {
            return Ok(None);
        };
        Self::open(root).map(Some)
    }

    fn open(root: PathBuf) -> Result<Self, String> {
        if !root.is_absolute() {
            return Err(format!("{QUALIFICATION_GATE_ENV} must be an absolute path"));
        }
        let metadata = fs::symlink_metadata(&root)
            .map_err(|error| format!("qualification dispatch gate is unavailable: {error}"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("qualification dispatch gate must be a non-symlink directory".into());
        }
        verify_private_gate_path(&root)?;
        Ok(Self { root })
    }

    async fn reach(
        &self,
        operation_id: &str,
        phase: QualificationDispatchPhase,
    ) -> Result<bool, String> {
        let operation_id_sha256 = hex::encode(Sha256::digest(operation_id.as_bytes()));
        let request_path = self
            .root
            .join(format!("{operation_id_sha256}.request.json"));
        let metadata = match fs::symlink_metadata(&request_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(format!(
                    "qualification gate request is unavailable: {error}"
                ))
            }
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4096 {
            return Err(
                "qualification gate request must be a bounded regular non-symlink file".into(),
            );
        }
        verify_private_gate_path(&request_path)?;
        let request: QualificationGateRequest =
            serde_json::from_slice(&fs::read(&request_path).map_err(|error| {
                format!("qualification gate request could not be read: {error}")
            })?)
            .map_err(|error| format!("qualification gate request is invalid: {error}"))?;
        if request.schema_version != QUALIFICATION_GATE_SCHEMA
            || request.operation_id_sha256 != operation_id_sha256
        {
            return Err("qualification gate request does not bind the exact operation".into());
        }
        if request.phase != phase {
            return Ok(false);
        }

        let reached_path = self
            .root
            .join(format!("{operation_id_sha256}.reached.json"));
        write_private_atomic_json(
            &reached_path,
            &json!({
                "schema_version": QUALIFICATION_GATE_SCHEMA,
                "operation_id_sha256": operation_id_sha256,
                "phase": phase.as_str(),
                "management_pid": std::process::id(),
                "reached_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            }),
        )?;

        let started = tokio::time::Instant::now();
        while request_path.exists() {
            if started.elapsed() >= QUALIFICATION_GATE_TIMEOUT {
                return Err(
                    "qualification dispatch gate timed out without operator release".into(),
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(true)
    }
}

#[cfg(unix)]
fn verify_private_gate_path(path: &FsPath) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::symlink_metadata(path)
        .map_err(|error| format!("qualification gate metadata is unavailable: {error}"))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err("qualification dispatch gate must not be group/world accessible".into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_private_gate_path(path: &FsPath) -> Result<(), String> {
    fs::symlink_metadata(path)
        .map_err(|error| format!("qualification gate metadata is unavailable: {error}"))?;
    Ok(())
}

fn write_private_atomic_json(path: &FsPath, value: &serde_json::Value) -> Result<(), String> {
    use std::io::Write;
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| format!("qualification gate event could not be created: {error}"))?;
        serde_json::to_writer(&mut file, value)
            .map_err(|error| format!("qualification gate event could not be encoded: {error}"))?;
        file.write_all(b"\n")
            .map_err(|error| format!("qualification gate event could not be written: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("qualification gate event could not be synced: {error}"))?;
        fs::rename(&temporary, path)
            .map_err(|error| format!("qualification gate event could not be published: {error}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[async_trait]
trait EffectDispatcher: Send + Sync {
    async fn dispatch(&self, command: &CellCommand) -> super::admin_v2::CelldProviderEffect;
    fn lookup(&self, management_operation_id: &str)
        -> Option<super::admin_v2::CelldProviderEffect>;
}

struct ManagementEffectDispatcher {
    state: AppState,
}

#[async_trait]
impl EffectDispatcher for ManagementEffectDispatcher {
    async fn dispatch(&self, command: &CellCommand) -> super::admin_v2::CelldProviderEffect {
        super::admin_v2::dispatch_celld_effect(self.state.clone(), command).await
    }

    fn lookup(
        &self,
        management_operation_id: &str,
    ) -> Option<super::admin_v2::CelldProviderEffect> {
        super::admin_v2::lookup_celld_management_effect(&self.state, management_operation_id)
    }
}

#[derive(Clone)]
struct CelldApiState {
    status: CelldStatus,
    client: Option<CelldClient>,
    effect_ledger: Option<Arc<EffectLedger>>,
    callback_verifier: Option<Arc<RequestVerifier>>,
    effect_dispatcher: Option<Arc<dyn EffectDispatcher>>,
    qualification_dispatch_gate: Option<Arc<QualificationDispatchGate>>,
    callback_mtls_cn: Option<String>,
    configuration_error: Option<String>,
}

pub fn router_from_env(app_state: AppState) -> Router {
    let state = match CelldConfig::from_env() {
        Ok(config) => {
            let status = config.status();
            match CelldClient::new(config.clone()) {
                Ok(client) => match callback_components(&config, app_state) {
                    Ok((ledger, verifier, dispatcher)) => {
                        let qualification_dispatch_gate =
                            match QualificationDispatchGate::from_env() {
                                Ok(gate) => gate.map(Arc::new),
                                Err(error) => return router(invalid_state(status, error)),
                            };
                        CelldApiState {
                            status,
                            client: Some(client),
                            effect_ledger: Some(ledger),
                            callback_verifier: Some(verifier),
                            effect_dispatcher: Some(dispatcher),
                            qualification_dispatch_gate,
                            callback_mtls_cn: config.callback_mtls_cn.clone(),
                            configuration_error: None,
                        }
                    }
                    Err(error) => invalid_state(status, error),
                },
                Err(crate::celld::client::ClientError::Disabled) => CelldApiState {
                    status,
                    client: None,
                    effect_ledger: None,
                    callback_verifier: None,
                    effect_dispatcher: None,
                    qualification_dispatch_gate: None,
                    callback_mtls_cn: None,
                    configuration_error: None,
                },
                Err(error) => invalid_state(status, error.to_string()),
            }
        }
        Err(error) => invalid_state(
            CelldStatus {
                enabled: true,
                configured: false,
                endpoint: None,
                celld_version: crate::celld::client::PINNED_CELLD_VERSION.into(),
                celld_commit: crate::celld::client::PINNED_CELLD_COMMIT.into(),
                protocol_version: "celld-internal-v1".into(),
                adapter_version: env!("CARGO_PKG_VERSION").into(),
                security_posture: "configuration rejected; no Celld traffic permitted".into(),
                unavailable_code: Some("celld.configuration_invalid".into()),
            },
            error.to_string(),
        ),
    };
    router(state)
}

fn invalid_state(mut status: CelldStatus, error: String) -> CelldApiState {
    status.configured = false;
    status.security_posture = "configuration rejected; no Celld traffic permitted".into();
    status.unavailable_code = Some("celld.configuration_invalid".into());
    CelldApiState {
        status,
        client: None,
        effect_ledger: None,
        callback_verifier: None,
        effect_dispatcher: None,
        qualification_dispatch_gate: None,
        callback_mtls_cn: None,
        configuration_error: Some(error),
    }
}

fn callback_components(
    config: &CelldConfig,
    app_state: AppState,
) -> Result<
    (
        Arc<EffectLedger>,
        Arc<RequestVerifier>,
        Arc<dyn EffectDispatcher>,
    ),
    String,
> {
    let ledger_path = config
        .effect_ledger_path
        .as_deref()
        .ok_or_else(|| "durable effect ledger path is missing".to_string())?;
    let ledger = EffectLedger::open(ledger_path).map_err(|error| error.to_string())?;
    let previous = match (
        config.previous_key_id.as_deref(),
        config.previous_auth_key_file.as_deref(),
        config.previous_key_valid_from,
        config.previous_key_valid_until,
    ) {
        (Some(key_id), Some(path), Some(valid_from), Some(valid_until)) => Some(PreviousKeyFile {
            key_id,
            path,
            valid_from,
            valid_until,
        }),
        _ => None,
    };
    let verifier = RequestVerifier::from_files(
        config.key_id.clone().unwrap_or_default(),
        config.auth_key_file.as_deref().expect("validated"),
        previous,
        chrono::Duration::minutes(2),
    )
    .map_err(|error| error.to_string())?;
    Ok((
        Arc::new(ledger),
        Arc::new(verifier),
        Arc::new(ManagementEffectDispatcher { state: app_state }),
    ))
}

fn router(state: CelldApiState) -> Router {
    Router::new()
        .route("/api/v2/celld/status", get(status))
        .route(EFFECT_CALLBACK_PATH, post(effect_callback))
        .route("/api/v2/celld/cells/{instance_id}", get(get_cell))
        .route("/api/v2/celld/cells/{instance_id}/commands", post(command))
        .route(
            "/api/v2/celld/cells/{instance_id}/reconcile",
            post(reconcile),
        )
        .route("/api/v2/celld/bundles/validate", post(validate_bundle))
        .route("/api/v2/celld/fleets/validate", post(validate_fleet))
        .route("/api/v2/celld/fleets/preflight", post(preflight))
        .route("/api/v2/celld/fleets/diagnose", post(diagnose))
        .route("/api/v2/celld/fleets/plan-upgrade", post(upgrade))
        .with_state(state)
}

async fn status(State(state): State<CelldApiState>) -> Json<serde_json::Value> {
    let mut capabilities = vec![
        "bundle.validate",
        "fleet.validate",
        "fleet.preflight",
        "fleet.diagnose",
        "fleet.plan-upgrade",
    ];
    if state.status.enabled && state.status.configured && state.client.is_some() {
        capabilities.extend(["cell.read", "cell.command", "cell.reconcile"]);
    }
    Json(json!({
        "schema_version": "management.celld-capabilities/v1",
        "status": state.status,
        "capabilities": capabilities,
        "configuration_error": state.configuration_error,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EffectCallbackRequest {
    instance_id: String,
    generation: u64,
    effect: EffectRecord,
}

async fn effect_callback(
    State(state): State<CelldApiState>,
    mtls_identity: Option<Extension<MtlsIdentity>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.len() > 1024 * 1024 {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "celld.effect_payload_too_large",
            "effect callback body exceeds 1 MiB",
        );
    }
    let (ledger, verifier, dispatcher) = match require_effect_callback(&state) {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    if let Some(expected_cn) = state.callback_mtls_cn.as_deref() {
        if mtls_identity.as_ref().map(|identity| identity.cn.as_str()) != Some(expected_cn) {
            return error(
                StatusCode::UNAUTHORIZED,
                "celld.callback_mtls_identity_required",
                "the Celld callback requires its configured mTLS identity",
            );
        }
    }
    let signed = match signed_request(&headers) {
        Ok(signed) => signed,
        Err(response) => return response,
    };
    if let Err(auth_error) = verifier.verify(
        &signed,
        "POST",
        EFFECT_CALLBACK_PATH,
        &body,
        chrono::Utc::now(),
    ) {
        return auth_error_response(auth_error);
    }
    let callback: EffectCallbackRequest = match serde_json::from_slice(&body) {
        Ok(callback) => callback,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "celld.effect_callback_invalid",
                "effect callback is not a valid v1 request",
            )
        }
    };
    let command = match callback.effect.to_command(callback.instance_id.clone()) {
        Ok(command) => command,
        Err(_) => {
            return error(
                StatusCode::CONFLICT,
                "celld.effect_hash_invalid",
                "effect request hash does not bind the callback payload",
            )
        }
    };
    let idempotency_key = header(&headers, "idempotency-key");
    if callback.generation != callback.effect.generation
        || signed.generation != callback.generation
        || signed.operation_id != callback.effect.operation_id
        || idempotency_key.as_deref() != Some(callback.effect.operation_id.as_str())
        || command.validate().is_err()
    {
        return error(
            StatusCode::CONFLICT,
            "celld.effect_identity_mismatch",
            "signed identity, callback identity, and idempotency key must match",
        );
    }

    if let Err(ledger_error) = ledger.claim_managed(&command) {
        return ledger_error_response(ledger_error);
    }
    let record = match ledger.record(&command.operation_id) {
        Ok(record) => record,
        Err(ledger_error) => return ledger_error_response(ledger_error),
    };
    if record.status.is_terminal() {
        return effect_response(&record, StatusCode::OK);
    }
    if matches!(
        record.status,
        EffectStatus::Dispatched | EffectStatus::Unknown
    ) {
        return resume_effect(ledger, dispatcher, record).await;
    }
    match ledger.begin_dispatch(&command.operation_id) {
        Ok(true) => {}
        Ok(false) => {
            let record = match ledger.record(&command.operation_id) {
                Ok(record) => record,
                Err(ledger_error) => return ledger_error_response(ledger_error),
            };
            return resume_effect(ledger, dispatcher, record).await;
        }
        Err(ledger_error) => return ledger_error_response(ledger_error),
    }

    if let Some(gate) = state.qualification_dispatch_gate.as_deref() {
        if let Err(gate_error) = gate
            .reach(
                &command.operation_id,
                QualificationDispatchPhase::DuringDispatch,
            )
            .await
        {
            return qualification_gate_error_response(gate_error);
        }
    }

    let effect = dispatcher.dispatch(&command).await;
    if let Some(management_operation_id) = effect.management_operation_id.as_deref() {
        if let Err(ledger_error) =
            ledger.set_management_operation(&command.operation_id, management_operation_id)
        {
            return ledger_error_response(ledger_error);
        }
    }
    if effect.status.is_terminal() {
        if let Err(ledger_error) = ledger.complete_with_code(
            &command.operation_id,
            effect.status,
            effect.terminal_code.as_deref(),
            effect.result.as_ref(),
        ) {
            return ledger_error_response(ledger_error);
        }
    } else if effect.status == EffectStatus::Unknown {
        if let Err(ledger_error) = ledger.mark_unknown(&command.operation_id) {
            return ledger_error_response(ledger_error);
        }
    }
    match ledger.record(&command.operation_id) {
        Ok(record) => {
            if let Some(gate) = state.qualification_dispatch_gate.as_deref() {
                if let Err(gate_error) = gate
                    .reach(
                        &command.operation_id,
                        QualificationDispatchPhase::AfterDispatch,
                    )
                    .await
                {
                    return qualification_gate_error_response(gate_error);
                }
            }
            effect_response(
                &record,
                if record.status.is_terminal() {
                    StatusCode::OK
                } else {
                    StatusCode::ACCEPTED
                },
            )
        }
        Err(ledger_error) => ledger_error_response(ledger_error),
    }
}

async fn resume_effect(
    ledger: Arc<EffectLedger>,
    dispatcher: Arc<dyn EffectDispatcher>,
    record: crate::celld::EffectLedgerRecord,
) -> Response {
    let mut management_operation_found = false;
    if record.status == EffectStatus::Dispatched
        && record.management_operation_id.is_none()
        && record.action == crate::celld::CellAction::Observe
    {
        if let Err(ledger_error) = ledger.complete_with_code(
            &record.operation_id,
            EffectStatus::Succeeded,
            Some("celld.observe_no_effect"),
            Some(&json!({"effect":"none"})),
        ) {
            return ledger_error_response(ledger_error);
        }
        management_operation_found = true;
    }
    if let Some(management_operation_id) = record.management_operation_id.as_deref() {
        if let Some(effect) = dispatcher.lookup(management_operation_id) {
            management_operation_found = true;
            if effect.status.is_terminal() {
                if let Err(ledger_error) = ledger.complete_with_code(
                    &record.operation_id,
                    effect.status,
                    effect.terminal_code.as_deref(),
                    effect.result.as_ref(),
                ) {
                    return ledger_error_response(ledger_error);
                }
            }
        }
    }
    if record.status == EffectStatus::Dispatched && !management_operation_found {
        if let Err(ledger_error) = ledger.mark_unknown(&record.operation_id) {
            return ledger_error_response(ledger_error);
        }
    }
    match ledger.record(&record.operation_id) {
        Ok(current) => effect_response(
            &current,
            if current.status.is_terminal() {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            },
        ),
        Err(ledger_error) => ledger_error_response(ledger_error),
    }
}

fn require_effect_callback(
    state: &CelldApiState,
) -> Result<
    (
        Arc<EffectLedger>,
        Arc<RequestVerifier>,
        Arc<dyn EffectDispatcher>,
    ),
    Response,
> {
    match (
        state.effect_ledger.as_ref(),
        state.callback_verifier.as_ref(),
        state.effect_dispatcher.as_ref(),
    ) {
        (Some(ledger), Some(verifier), Some(dispatcher)) => Ok((
            Arc::clone(ledger),
            Arc::clone(verifier),
            Arc::clone(dispatcher),
        )),
        _ => Err(error(
            StatusCode::SERVICE_UNAVAILABLE,
            state
                .status
                .unavailable_code
                .as_deref()
                .unwrap_or("celld.unavailable"),
            "Celld effect callback is disabled or invalidly configured",
        )),
    }
}

fn signed_request(headers: &HeaderMap) -> Result<SignedRequest, Response> {
    let required = |name| {
        header(headers, name).ok_or_else(|| {
            error(
                StatusCode::UNAUTHORIZED,
                "celld.signature_missing",
                "required callback authentication header is missing",
            )
        })
    };
    let generation = required("x-agentic-generation")?
        .parse::<u64>()
        .map_err(|_| {
            error(
                StatusCode::UNAUTHORIZED,
                "celld.signature_invalid",
                "callback generation header is invalid",
            )
        })?;
    Ok(SignedRequest {
        key_id: required("x-agentic-key-id")?,
        timestamp: required("x-agentic-timestamp")?,
        nonce: required("x-agentic-nonce")?,
        generation,
        operation_id: required("x-agentic-operation-id")?,
        body_sha256: required("x-agentic-body-sha256")?,
        signature: required("x-agentic-signature")?,
    })
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn auth_error_response(auth_error: AuthError) -> Response {
    let (status, code) = match auth_error {
        AuthError::Replay => (StatusCode::CONFLICT, "celld.signature_replay"),
        AuthError::Stale | AuthError::InvalidTimestamp => {
            (StatusCode::UNAUTHORIZED, "celld.signature_stale")
        }
        _ => (StatusCode::UNAUTHORIZED, "celld.signature_invalid"),
    };
    error(status, code, "effect callback authentication failed")
}

fn ledger_error_response(ledger_error: EffectLedgerError) -> Response {
    let (status, code, pre_provider_rejection) = match ledger_error {
        EffectLedgerError::Collision => (StatusCode::CONFLICT, "celld.operation_collision", false),
        EffectLedgerError::StaleGeneration { .. } => {
            (StatusCode::CONFLICT, "celld.stale_generation_fenced", true)
        }
        EffectLedgerError::FutureGeneration { .. }
        | EffectLedgerError::GenerationAdvanceNotAllowed => {
            (StatusCode::CONFLICT, "celld.future_generation_fenced", true)
        }
        EffectLedgerError::UnknownInstance(_) => (
            StatusCode::CONFLICT,
            "celld.instance_generation_unknown",
            true,
        ),
        EffectLedgerError::TerminalConflict { .. } => {
            (StatusCode::CONFLICT, "celld.terminal_conflict", false)
        }
        EffectLedgerError::ManagementOperationConflict => (
            StatusCode::CONFLICT,
            "celld.management_operation_collision",
            false,
        ),
        EffectLedgerError::Database(_)
        | EffectLedgerError::Io(_)
        | EffectLedgerError::InsecurePermissions
        | EffectLedgerError::UnknownOperation(_)
        | EffectLedgerError::CorruptRecord(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "celld.effect_ledger_failed",
            false,
        ),
    };
    if pre_provider_rejection {
        return (
            status,
            Json(json!({
                "error": {
                    "code": code,
                    "detail": "effect callback was rejected before a new provider effect",
                },
                "provider_dispatch_count_delta": 0,
            })),
        )
            .into_response();
    }
    error(
        status,
        code,
        "effect callback did not create a new provider effect",
    )
}

fn qualification_gate_error_response(detail: String) -> Response {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "celld.qualification_dispatch_gate_failed",
        &detail,
    )
}

fn effect_response(record: &crate::celld::EffectLedgerRecord, status: StatusCode) -> Response {
    (
        status,
        Json(json!({
            "operation_id": record.operation_id,
            "instance_id": record.instance_id,
            "generation": record.generation,
            "status": record.status,
            "provider_dispatch_count": record.provider_dispatch_count,
            "management_operation_id": record.management_operation_id,
            "terminal_code": record.terminal_code,
            "result": record.result,
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
struct GenerationQuery {
    generation: u64,
}
async fn get_cell(
    State(state): State<CelldApiState>,
    Path(instance_id): Path<String>,
    Query(query): Query<GenerationQuery>,
) -> Response {
    match require_client(&state) {
        Ok(client) => respond(client.get_cell(&instance_id, query.generation).await),
        Err(response) => response,
    }
}

async fn command(
    State(state): State<CelldApiState>,
    Path(instance_id): Path<String>,
    _admin: RequireAdmin,
    Json(command): Json<CellCommand>,
) -> Response {
    if command.instance_id != instance_id {
        return error(
            StatusCode::BAD_REQUEST,
            "celld.instance_mismatch",
            "path and command instance ids differ",
        );
    }
    match require_client(&state) {
        Ok(client) => respond(client.command(&command).await),
        Err(response) => response,
    }
}

#[derive(Deserialize)]
struct ReconcileRequest {
    management_generation: u64,
}
async fn reconcile(
    State(state): State<CelldApiState>,
    Path(instance_id): Path<String>,
    _admin: RequireAdmin,
    Json(request): Json<ReconcileRequest>,
) -> Response {
    match require_client(&state) {
        Ok(client) => respond(
            client
                .reconcile(&instance_id, request.management_generation)
                .await,
        ),
        Err(response) => response,
    }
}

async fn validate_bundle(Json(manifest): Json<WorkerBundleManifest>) -> Response {
    match manifest.validate() {
        Ok(()) => (
            StatusCode::OK,
            Json(
                json!({"valid":true,"runtime":"worker-celld","capabilities":manifest.capabilities}),
            ),
        )
            .into_response(),
        Err(error) => error_response(error),
    }
}
async fn validate_fleet(Json(manifest): Json<CelldFleetManifest>) -> Response {
    match manifest.validate() {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({"valid":true,"fleet_id":manifest.fleet_id})),
        )
            .into_response(),
        Err(error) => error_response(error),
    }
}
async fn preflight(Json(evidence): Json<BucketPreflightEvidence>) -> Response {
    match preflight_bucket(&evidence) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({"qualified":true,"evidence":evidence})),
        )
            .into_response(),
        Err(error) => error_response(error),
    }
}
async fn diagnose(Json(request): Json<FleetDiagnoseRequest>) -> Response {
    match diagnose_fleet(request) {
        Ok(report) => (StatusCode::OK, Json(report)).into_response(),
        Err(error) => error_response(error),
    }
}
#[derive(Deserialize)]
struct UpgradeRequest {
    manifest: CelldFleetManifest,
    from: String,
    to: String,
}
async fn upgrade(Json(request): Json<UpgradeRequest>) -> Response {
    match plan_upgrade(&request.manifest, &request.from, &request.to) {
        Ok(plan) => (StatusCode::OK, Json(json!({"qualified":true,"plan":plan}))).into_response(),
        Err(error) => error_response(error),
    }
}

fn require_client(state: &CelldApiState) -> Result<&CelldClient, Response> {
    state.client.as_ref().ok_or_else(|| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            state
                .status
                .unavailable_code
                .as_deref()
                .unwrap_or("celld.unavailable"),
            state
                .configuration_error
                .as_deref()
                .unwrap_or("Celld is disabled or unavailable"),
        )
    })
}
fn respond<T: serde::Serialize>(result: Result<T, crate::celld::client::ClientError>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error_value) => {
            let detail = public_client_error(&error_value);
            error(StatusCode::BAD_GATEWAY, "celld.upstream_failure", &detail)
        }
    }
}

fn public_client_error(error: &crate::celld::client::ClientError) -> String {
    match error {
        crate::celld::client::ClientError::Response { status, .. } => {
            format!("Celld returned HTTP {status}; response body withheld")
        }
        crate::celld::client::ClientError::Disabled => "Celld is disabled".into(),
        _ => "Celld upstream request failed; inspect redacted service diagnostics".into(),
    }
}
fn error_response(error_value: crate::celld::ValidationError) -> Response {
    error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "celld.validation_failed",
        &error_value.to_string(),
    )
}
fn error(status: StatusCode, code: &str, detail: &str) -> Response {
    (status, Json(json!({"error":{"code":code,"detail":detail}}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        celld::{auth::RequestSigner, client::ClientError, CellAction, EffectRecord, EffectStatus},
        http::operator_auth::OperatorRole,
    };
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    const CALLBACK_KEY: &[u8] = b"01234567890123456789012345678901";

    struct MockEffectDispatcher {
        calls: AtomicUsize,
    }

    struct DispatchedEffectDispatcher {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl EffectDispatcher for MockEffectDispatcher {
        async fn dispatch(
            &self,
            command: &CellCommand,
        ) -> crate::http::admin_v2::CelldProviderEffect {
            self.calls.fetch_add(1, Ordering::SeqCst);
            crate::http::admin_v2::CelldProviderEffect {
                status: EffectStatus::Succeeded,
                management_operation_id: Some(format!("management-{}", command.operation_id)),
                terminal_code: Some("provider.effect_succeeded".into()),
                result: Some(json!({"provider_effect": command.action})),
            }
        }

        fn lookup(
            &self,
            _management_operation_id: &str,
        ) -> Option<crate::http::admin_v2::CelldProviderEffect> {
            None
        }
    }

    #[async_trait]
    impl EffectDispatcher for DispatchedEffectDispatcher {
        async fn dispatch(
            &self,
            command: &CellCommand,
        ) -> crate::http::admin_v2::CelldProviderEffect {
            self.calls.fetch_add(1, Ordering::SeqCst);
            crate::http::admin_v2::CelldProviderEffect {
                status: EffectStatus::Dispatched,
                management_operation_id: Some(format!("management-{}", command.operation_id)),
                terminal_code: None,
                result: None,
            }
        }

        fn lookup(
            &self,
            _management_operation_id: &str,
        ) -> Option<crate::http::admin_v2::CelldProviderEffect> {
            None
        }
    }

    fn disabled_state() -> CelldApiState {
        CelldApiState {
            status: CelldStatus {
                enabled: false,
                configured: false,
                endpoint: None,
                celld_version: crate::celld::client::PINNED_CELLD_VERSION.into(),
                celld_commit: crate::celld::client::PINNED_CELLD_COMMIT.into(),
                protocol_version: "celld-internal-v1".into(),
                adapter_version: "test".into(),
                security_posture: "test".into(),
                unavailable_code: Some("celld.disabled".into()),
            },
            client: None,
            effect_ledger: None,
            callback_verifier: None,
            effect_dispatcher: None,
            qualification_dispatch_gate: None,
            callback_mtls_cn: None,
            configuration_error: None,
        }
    }

    #[tokio::test]
    async fn status_discovers_only_safe_capabilities_when_provider_is_disabled() {
        let body = status(State(disabled_state())).await.0;
        assert_eq!(body["schema_version"], "management.celld-capabilities/v1");
        let capabilities = body["capabilities"].as_array().unwrap();
        assert!(capabilities.iter().any(|value| value == "bundle.validate"));
        assert!(!capabilities.iter().any(|value| value == "cell.command"));
        assert!(!capabilities.iter().any(|value| value == "cell.reconcile"));
    }

    fn callback_state() -> (CelldApiState, Arc<MockEffectDispatcher>, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(MockEffectDispatcher {
            calls: AtomicUsize::new(0),
        });
        let state = CelldApiState {
            status: CelldStatus {
                enabled: true,
                configured: true,
                endpoint: Some("https://celld.internal".into()),
                celld_version: crate::celld::client::PINNED_CELLD_VERSION.into(),
                celld_commit: crate::celld::client::PINNED_CELLD_COMMIT.into(),
                protocol_version: "celld-internal-v1".into(),
                adapter_version: "test".into(),
                security_posture: "test".into(),
                unavailable_code: None,
            },
            client: None,
            effect_ledger: Some(Arc::new(
                EffectLedger::open(&directory.path().join("effects.db")).unwrap(),
            )),
            callback_verifier: Some(Arc::new(RequestVerifier::from_bytes(
                "active",
                CALLBACK_KEY,
                chrono::Duration::minutes(2),
            ))),
            effect_dispatcher: Some(dispatcher.clone()),
            qualification_dispatch_gate: None,
            callback_mtls_cn: None,
            configuration_error: None,
        };
        (state, dispatcher, directory)
    }

    #[tokio::test]
    async fn qualification_gate_publishes_the_exact_management_phase_and_waits_for_release() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("dispatch-gates");
        fs::create_dir(&root).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let operation_id = "op-qualified-during-dispatch";
        let operation_id_sha256 = hex::encode(Sha256::digest(operation_id.as_bytes()));
        let request_path = root.join(format!("{operation_id_sha256}.request.json"));
        fs::write(
            &request_path,
            serde_json::to_vec(&json!({
                "schema_version": QUALIFICATION_GATE_SCHEMA,
                "operation_id_sha256": operation_id_sha256,
                "phase": "during_dispatch",
            }))
            .unwrap(),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&request_path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let gate = QualificationDispatchGate::open(root.clone()).unwrap();
        let task = tokio::spawn(async move {
            gate.reach(operation_id, QualificationDispatchPhase::DuringDispatch)
                .await
        });
        let reached_path = root.join(format!("{operation_id_sha256}.reached.json"));
        for _ in 0..100 {
            if reached_path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let reached: serde_json::Value =
            serde_json::from_slice(&fs::read(&reached_path).unwrap()).unwrap();
        assert_eq!(reached["schema_version"], QUALIFICATION_GATE_SCHEMA);
        assert_eq!(reached["operation_id_sha256"], operation_id_sha256);
        assert_eq!(reached["phase"], "during_dispatch");
        assert_eq!(reached["management_pid"], std::process::id());
        assert!(reached["reached_at"].as_str().is_some());
        fs::remove_file(request_path).unwrap();
        assert!(task.await.unwrap().unwrap());
    }

    fn effect_for(command: &CellCommand) -> EffectRecord {
        EffectRecord {
            operation_id: command.operation_id.clone(),
            request_hash: command.request_hash.clone(),
            action: command.action,
            generation: command.generation,
            payload: command.payload.clone(),
            status: EffectStatus::Pending,
            attempts: 1,
            retry_at: None,
            terminal_code: None,
            management_operation_id: None,
        }
    }

    fn callback_request(command: &CellCommand) -> Request<Body> {
        let value = json!({
            "instance_id": command.instance_id,
            "generation": command.generation,
            "effect": effect_for(command),
        });
        let body = serde_json::to_vec(&value).unwrap();
        let signed = RequestSigner::from_bytes("active", CALLBACK_KEY)
            .sign(
                "POST",
                EFFECT_CALLBACK_PATH,
                &command.operation_id,
                command.generation,
                &body,
            )
            .unwrap();
        Request::builder()
            .method("POST")
            .uri(EFFECT_CALLBACK_PATH)
            .header("content-type", "application/json")
            .header("idempotency-key", &command.operation_id)
            .header("x-agentic-key-id", signed.key_id)
            .header("x-agentic-timestamp", signed.timestamp)
            .header("x-agentic-nonce", signed.nonce)
            .header("x-agentic-generation", signed.generation)
            .header("x-agentic-operation-id", signed.operation_id)
            .header("x-agentic-body-sha256", signed.body_sha256)
            .header("x-agentic-signature", signed.signature)
            .body(Body::from(body))
            .unwrap()
    }

    async fn callback_json(app: &Router, command: &CellCommand) -> (StatusCode, serde_json::Value) {
        let response = app
            .clone()
            .oneshot(callback_request(command))
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn callback_requires_the_configured_mtls_identity_before_dispatch() {
        let (mut state, dispatcher, _directory) = callback_state();
        state.callback_mtls_cn = Some("celld-fleet-a".into());
        let app = router(state);
        let command = CellCommand::new(
            "op-mtls",
            "instance-mtls",
            1,
            CellAction::Provision,
            json!({}),
        )
        .unwrap();

        let missing = app
            .clone()
            .oneshot(callback_request(&command))
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let mut wrong_request = callback_request(&command);
        wrong_request.extensions_mut().insert(MtlsIdentity {
            cn: "celld-fleet-b".into(),
        });
        let wrong = app.clone().oneshot(wrong_request).await.unwrap();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

        let mut valid_request = callback_request(&command);
        valid_request.extensions_mut().insert(MtlsIdentity {
            cn: "celld-fleet-a".into(),
        });
        let valid = app.oneshot(valid_request).await.unwrap();
        assert_eq!(valid.status(), StatusCode::OK);
        assert_eq!(dispatcher.calls.load(Ordering::SeqCst), 1);
    }

    fn operator_request(path: &str, body: serde_json::Value) -> Request<Body> {
        let mut request = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        request.extensions_mut().insert(OperatorRole::Operator);
        request
    }

    #[tokio::test]
    async fn command_and_reconcile_require_admin_role() {
        let app = router(disabled_state());
        let command =
            CellCommand::new("op-admin", "instance-a", 1, CellAction::Observe, json!({})).unwrap();
        let command_response = app
            .clone()
            .oneshot(operator_request(
                "/api/v2/celld/cells/instance-a/commands",
                serde_json::to_value(command).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(command_response.status(), StatusCode::FORBIDDEN);

        let reconcile_response = app
            .oneshot(operator_request(
                "/api/v2/celld/cells/instance-a/reconcile",
                json!({"management_generation":1}),
            ))
            .await
            .unwrap();
        assert_eq!(reconcile_response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn fixture_diagnose_route_is_read_only_and_does_not_claim_live_success() {
        let app = router(disabled_state());
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../../deploy/celld/fleet.example.json")).unwrap();
        let response = app
            .oneshot(operator_request(
                "/api/v2/celld/fleets/diagnose",
                json!({
                    "schema_version":"agentic-sandbox.celld-fleet-diagnose/v1",
                    "source":"fixture",
                    "observed_at":"2026-08-17T12:00:00Z",
                    "manifest":manifest,
                    "backend":{"substrate":"qemu","reachable":true},
                    "artifact":{
                        "celld_version":"v0.2.1",
                        "celld_artifact_sha256":"sha256:69554171c3b927d32b6d334475071d4b5fa7abd1a2bdd11cac78a6858a5b2923",
                        "worker_digest":"sha256:9057aa0debb402c9f8177d1df0c8c06de487c266029f0c5282a8e1cd1076322b"
                    },
                    "listeners":{
                        "public_listener":"0.0.0.0:443",
                        "internal_listener":"10.88.0.10:8124",
                        "advertised_addresses":["10.88.0.10:8124","10.88.0.11:8124","10.88.0.12:8124"]
                    },
                    "nodes":[
                        {"node_id":"node-1","advertised_address":"10.88.0.10:8124","ready":true},
                        {"node_id":"node-2","advertised_address":"10.88.0.11:8124","ready":true},
                        {"node_id":"node-3","advertised_address":"10.88.0.12:8124","ready":true}
                    ],
                    "membership":{"stable":true,"members":["node-1","node-2","node-3"]},
                    "store":{
                        "provider":"aws-s3",
                        "bucket":"replace-with-dedicated-celld-bucket",
                        "prefix":"celld-poc/cells",
                        "endpoint":null,
                        "reachable":true,
                        "startup_probe_passed":true
                    }
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["status"], "NOT_RUN");
        assert_eq!(body["mutating"], false);
        assert_eq!(body["live_qualification"], false);
        assert!(body.get("manifest").is_none());
    }

    #[tokio::test]
    async fn signed_effect_callback_dispatches_once_and_replays_stable_terminal_evidence() {
        let (state, dispatcher, _directory) = callback_state();
        let app = router(state);
        let command = CellCommand::new(
            "op-provision",
            "instance-a",
            1,
            CellAction::Provision,
            json!({"name":"instance-a","runtime":"docker"}),
        )
        .unwrap();

        let (first_status, first) = callback_json(&app, &command).await;
        assert_eq!(first_status, StatusCode::OK);
        assert_eq!(first["status"], "succeeded");
        assert_eq!(first["terminal_code"], "provider.effect_succeeded");
        assert_eq!(first["provider_dispatch_count"], 1);

        for _ in 0..100 {
            let (replay_status, replay) = callback_json(&app, &command).await;
            assert_eq!(replay_status, StatusCode::OK);
            assert_eq!(replay, first);
        }
        assert_eq!(dispatcher.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn callback_rejects_collision_and_stale_generation_before_provider_effect() {
        let (state, dispatcher, _directory) = callback_state();
        let app = router(state);
        let provision = CellCommand::new(
            "op-provision",
            "instance-a",
            1,
            CellAction::Provision,
            json!({"name":"instance-a","runtime":"docker"}),
        )
        .unwrap();
        assert_eq!(callback_json(&app, &provision).await.0, StatusCode::OK);

        let collision = CellCommand::new(
            "op-provision",
            "instance-a",
            1,
            CellAction::Destroy,
            json!({}),
        )
        .unwrap();
        let (_, collision_body) = callback_json(&app, &collision).await;
        assert_eq!(collision_body["error"]["code"], "celld.operation_collision");

        let destroy = CellCommand::new(
            "op-destroy",
            "instance-a",
            1,
            CellAction::Destroy,
            json!({}),
        )
        .unwrap();
        assert_eq!(callback_json(&app, &destroy).await.0, StatusCode::OK);
        let next = CellCommand::new(
            "op-next",
            "instance-a",
            2,
            CellAction::Provision,
            json!({"name":"instance-a","runtime":"docker"}),
        )
        .unwrap();
        assert_eq!(callback_json(&app, &next).await.0, StatusCode::OK);

        let stale =
            CellCommand::new("op-stale", "instance-a", 1, CellAction::Stop, json!({})).unwrap();
        let (_, stale_body) = callback_json(&app, &stale).await;
        assert_eq!(stale_body["error"]["code"], "celld.stale_generation_fenced");
        assert_eq!(stale_body["provider_dispatch_count_delta"], 0);

        let future =
            CellCommand::new("op-future", "instance-a", 4, CellAction::Stop, json!({})).unwrap();
        let (_, future_body) = callback_json(&app, &future).await;
        assert_eq!(
            future_body["error"]["code"],
            "celld.future_generation_fenced"
        );
        assert_eq!(future_body["provider_dispatch_count_delta"], 0);
        assert_eq!(dispatcher.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn callback_rejects_signature_replay_without_provider_effect() {
        let (state, dispatcher, _directory) = callback_state();
        let app = router(state);
        let command = CellCommand::new(
            "op-replay",
            "instance-a",
            1,
            CellAction::Provision,
            json!({"name":"instance-a","runtime":"docker"}),
        )
        .unwrap();
        let request = callback_request(&command);
        let (parts, body) = request.into_parts();
        let body = to_bytes(body, 1024 * 1024).await.unwrap();
        let mut duplicate = Request::builder()
            .method(parts.method.clone())
            .uri(parts.uri.clone())
            .body(Body::from(body.clone()))
            .unwrap();
        *duplicate.headers_mut() = parts.headers.clone();
        assert_eq!(
            app.clone()
                .oneshot(Request::from_parts(parts, Body::from(body)))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let response = app.oneshot(duplicate).await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(dispatcher.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn missing_management_operation_becomes_unknown_without_redispatch() {
        let (mut state, _dispatcher, _directory) = callback_state();
        let dispatcher = Arc::new(DispatchedEffectDispatcher {
            calls: AtomicUsize::new(0),
        });
        state.effect_dispatcher = Some(dispatcher.clone());
        let app = router(state);
        let command = CellCommand::new(
            "op-unknown",
            "instance-a",
            1,
            CellAction::Provision,
            json!({"name":"instance-a","runtime":"docker"}),
        )
        .unwrap();

        let (first_status, first) = callback_json(&app, &command).await;
        assert_eq!(first_status, StatusCode::ACCEPTED);
        assert_eq!(first["status"], "dispatched");
        assert_eq!(first["provider_dispatch_count"], 1);
        let (second_status, second) = callback_json(&app, &command).await;
        assert_eq!(second_status, StatusCode::ACCEPTED);
        assert_eq!(second["status"], "unknown");
        assert_eq!(second["management_operation_id"], "management-op-unknown");
        assert_eq!(second["provider_dispatch_count"], 1);
        assert_eq!(dispatcher.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn interrupted_no_effect_observe_resumes_from_the_durable_dispatch_claim() {
        let (state, dispatcher, _directory) = callback_state();
        let ledger = state.effect_ledger.as_ref().unwrap();
        let provision = CellCommand::new(
            "op-seed-interrupted-observe",
            "instance-a",
            1,
            CellAction::Provision,
            json!({"name":"instance-a","runtime":"docker"}),
        )
        .unwrap();
        ledger.claim_managed(&provision).unwrap();
        assert!(ledger.begin_dispatch(&provision.operation_id).unwrap());
        ledger
            .complete(
                &provision.operation_id,
                EffectStatus::Succeeded,
                Some(&json!({"runtime_id":"runtime-a"})),
            )
            .unwrap();
        let command = CellCommand::new(
            "op-interrupted-observe",
            "instance-a",
            1,
            CellAction::Observe,
            json!({}),
        )
        .unwrap();
        ledger.claim_managed(&command).unwrap();
        assert!(ledger.begin_dispatch(&command.operation_id).unwrap());

        let (status, body) = callback_json(&router(state), &command).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "succeeded");
        assert_eq!(body["provider_dispatch_count"], 1);
        assert_eq!(body["terminal_code"], "celld.observe_no_effect");
        assert_eq!(body["result"], json!({"effect":"none"}));
        assert_eq!(dispatcher.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn public_upstream_error_never_echoes_raw_body() {
        let secret = "AWS_SECRET_ACCESS_KEY=should-never-escape";
        let detail = public_client_error(&ClientError::Response {
            status: 502,
            body: secret.repeat(1_000),
        });
        assert_eq!(detail, "Celld returned HTTP 502; response body withheld");
        assert!(!detail.contains(secret));
        assert!(detail.len() < 128);
    }
}
