//! Optional Celld integration contracts and compatibility adapter.
//!
//! Celld is not a VM provider.  This module keeps its three roles separate:
//! durable `InstanceCell` coordination, constrained Worker/Wasm validation,
//! and fleet deployment validation.  The integration is disabled unless
//! `AGENTIC_CELLD_ENABLED=true` is explicitly configured.

pub mod auth;
pub mod client;
pub mod diagnostics;
pub mod effect_ledger;
pub mod model;
pub mod validation;

pub use client::{CelldClient, CelldConfig, CelldStatus};
pub use diagnostics::{diagnose_fleet, FleetDiagnoseReport, FleetDiagnoseRequest};
pub use effect_ledger::{EffectClaim, EffectLedger, EffectLedgerError, EffectLedgerRecord};
pub use model::{
    CellAction, CellCommand, CellError, CellEvent, CellEventKind, CommandDisposition, EffectRecord,
    EffectStatus, InstanceCell, LifecycleState, ManagementObservation, ReconcileClassification,
    ReconcileResult,
};
pub use validation::{
    plan_upgrade, preflight_bucket, BucketPreflightEvidence, CelldFleetManifest, UpgradePlan,
    ValidationError, WorkerBundleManifest, SUPPORTED_CELLD_VERSION,
};

/// Capabilities that `worker-celld` may advertise after bundle validation.
pub const WORKER_CELLD_CAPABILITIES: &[&str] = &[
    "worker.fetch",
    "worker.rpc",
    "durable.storage",
    "durable.alarm",
    "websocket.inbound",
    "network.outbound.fetch",
    "wasm.module",
    "assets.static",
];

/// OS/runtime capabilities that the constrained Worker runtime must never
/// claim.  Keeping this as executable product data prevents documentation and
/// discovery from drifting apart.
pub const WORKER_CELLD_EXCLUSIONS: &[&str] = &[
    "task.exec",
    "session.pty",
    "workspace.bind",
    "process.spawn",
    "network.raw_tcp",
    "network.ssh",
    "runtime.vm",
    "runtime.container",
    "agentshare.mount",
];
