use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use super::{validation::invalid, CelldFleetManifest, ValidationError};

pub const FLEET_DIAGNOSE_SCHEMA: &str = "agentic-sandbox.celld-fleet-diagnose/v1";
pub const FLEET_DIAGNOSE_REPORT_SCHEMA: &str = "agentic-sandbox.celld-fleet-diagnose-report/v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackendObservation {
    pub substrate: String,
    pub reachable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactObservation {
    pub celld_version: String,
    pub celld_artifact_sha256: String,
    pub worker_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ListenerObservation {
    pub public_listener: String,
    pub internal_listener: String,
    pub advertised_addresses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NodeObservation {
    pub node_id: String,
    pub advertised_address: String,
    pub ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MembershipObservation {
    pub stable: bool,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StoreObservation {
    pub provider: String,
    pub bucket: String,
    pub prefix: String,
    pub endpoint: Option<String>,
    pub reachable: bool,
    pub startup_probe_passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FleetDiagnoseRequest {
    pub schema_version: String,
    pub source: String,
    pub observed_at: DateTime<Utc>,
    pub manifest: CelldFleetManifest,
    pub backend: BackendObservation,
    pub artifact: ArtifactObservation,
    pub listeners: ListenerObservation,
    pub nodes: Vec<NodeObservation>,
    pub membership: MembershipObservation,
    pub store: StoreObservation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiagnosticCheck {
    pub id: String,
    pub status: String,
    pub reason_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiagnosticObservations {
    pub backend: BackendObservation,
    pub artifact: ArtifactObservation,
    pub listeners: ListenerObservation,
    pub nodes: Vec<NodeObservation>,
    pub membership: MembershipObservation,
    pub store: StoreObservation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FleetDiagnoseReport {
    pub schema_version: String,
    pub fleet_id: String,
    pub source: String,
    pub observed_at: DateTime<Utc>,
    pub status: String,
    pub reason_code: String,
    pub mutating: bool,
    pub live_qualification: bool,
    pub checks: Vec<DiagnosticCheck>,
    pub observations: DiagnosticObservations,
}

pub fn diagnose_fleet(
    request: FleetDiagnoseRequest,
) -> Result<FleetDiagnoseReport, ValidationError> {
    if request.schema_version != FLEET_DIAGNOSE_SCHEMA {
        return Err(invalid(
            "schema_version",
            "unsupported fleet diagnose contract",
        ));
    }
    if !["fixture", "local"].contains(&request.source.as_str()) {
        return Err(invalid(
            "source",
            "must be fixture or local; live collectors are not implemented",
        ));
    }
    request.manifest.validate()?;

    let expected_addresses = request
        .manifest
        .network
        .advertised_addresses
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let observed_addresses = request
        .listeners
        .advertised_addresses
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let node_ids = request
        .nodes
        .iter()
        .map(|node| node.node_id.clone())
        .collect::<BTreeSet<_>>();
    let member_ids = request
        .membership
        .members
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let node_addresses = request
        .nodes
        .iter()
        .map(|node| node.advertised_address.clone())
        .collect::<BTreeSet<_>>();

    let mut checks = Vec::new();
    push_check(
        &mut checks,
        "celld.diagnose.backend",
        request.backend.reachable && request.backend.substrate == request.manifest.nodes.substrate,
    );
    push_check(
        &mut checks,
        "celld.diagnose.artifact",
        request.artifact.celld_version == request.manifest.celld.version
            && request.artifact.celld_artifact_sha256 == request.manifest.celld.digest
            && request.artifact.worker_digest == request.manifest.application_bundle,
    );
    push_check(
        &mut checks,
        "celld.diagnose.listeners",
        request.listeners.public_listener == request.manifest.network.public_listener
            && request.listeners.internal_listener == request.manifest.network.internal_listener
            && observed_addresses == expected_addresses,
    );
    push_check(
        &mut checks,
        "celld.diagnose.nodes",
        request.nodes.len() == request.manifest.nodes.count as usize
            && node_ids.len() == request.nodes.len()
            && request.nodes.iter().all(|node| node.ready)
            && node_addresses == expected_addresses,
    );
    push_check(
        &mut checks,
        "celld.diagnose.membership",
        request.membership.stable && member_ids == node_ids,
    );
    push_check(
        &mut checks,
        "celld.diagnose.store",
        request.store.reachable
            && request.store.startup_probe_passed
            && request.store.provider == request.manifest.storage.provider
            && request.store.bucket == request.manifest.storage.bucket
            && request.store.prefix == request.manifest.storage.prefix
            && request.store.endpoint == request.manifest.storage.endpoint,
    );

    let failed = checks.iter().any(|check| check.status == "FAIL");
    let (status, reason_code) = if failed {
        ("FAIL", "celld.diagnose.observation_mismatch")
    } else {
        ("NOT_RUN", "celld.diagnose.live_observation_missing")
    };

    Ok(FleetDiagnoseReport {
        schema_version: FLEET_DIAGNOSE_REPORT_SCHEMA.into(),
        fleet_id: request.manifest.fleet_id,
        source: request.source,
        observed_at: request.observed_at,
        status: status.into(),
        reason_code: reason_code.into(),
        mutating: false,
        live_qualification: false,
        checks,
        observations: DiagnosticObservations {
            backend: request.backend,
            artifact: request.artifact,
            listeners: request.listeners,
            nodes: request.nodes,
            membership: request.membership,
            store: request.store,
        },
    })
}

fn push_check(checks: &mut Vec<DiagnosticCheck>, id: &str, passed: bool) {
    checks.push(DiagnosticCheck {
        id: id.into(),
        status: if passed { "PASS" } else { "FAIL" }.into(),
        reason_code: if passed {
            format!("{id}.matched")
        } else {
            format!("{id}.mismatch")
        },
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(source: &str) -> FleetDiagnoseRequest {
        serde_json::from_value(json!({
            "schema_version": FLEET_DIAGNOSE_SCHEMA,
            "source": source,
            "observed_at": "2026-08-17T12:00:00Z",
            "manifest": {
                "api_version":"celld-fleet.agentic-sandbox/v1","fleet_id":"poc","trust_domain":"poc.internal",
                "celld":{"version":"v0.2.1","artifact":"https://example.invalid/celld.tar.gz","digest":format!("sha256:{}", "a".repeat(64)),"protocol_version":"celld-internal-v1"},
                "application_bundle":format!("sha256:{}", "b".repeat(64)),
                "nodes":{"count":3,"substrate":"qemu","architecture":"x86_64","reserve":1},
                "storage":{"provider":"qualified-s3-compatible","bucket":"poc","prefix":"poc/cells","endpoint":"https://store.internal","region":"us-east-1","credential_ref":"broker://celld/poc","retain_on_destroy":true},
                "network":{"public_listener":"0.0.0.0:443","internal_listener":"127.0.0.1:8124","advertised_addresses":["10.0.0.1:8124","10.0.0.2:8124","10.0.0.3:8124"],"encrypted_overlay":true,"public_ingress_excludes_internal":true},
                "resources":{"cpu_cores":2.0,"memory_mb":1024,"max_resident_cells":100,"max_rss_mb":768},
                "telemetry":{"metrics":true,"structured_logs":true,"trace_propagation":"w3c","redaction_profile":"celld-v1"},
                "retention":{"cell_tombstone_days":30,"audit_days":90,"backup_rpo_seconds":300},
                "rollout":{"max_unavailable":1,"drain_timeout_seconds":60,"compatible_from":["v0.2.1"]}
            },
            "backend":{"substrate":"qemu","reachable":true},
            "artifact":{"celld_version":"v0.2.1","celld_artifact_sha256":format!("sha256:{}", "a".repeat(64)),"worker_digest":format!("sha256:{}", "b".repeat(64))},
            "listeners":{"public_listener":"0.0.0.0:443","internal_listener":"127.0.0.1:8124","advertised_addresses":["10.0.0.1:8124","10.0.0.2:8124","10.0.0.3:8124"]},
            "nodes":[
                {"node_id":"node-1","advertised_address":"10.0.0.1:8124","ready":true},
                {"node_id":"node-2","advertised_address":"10.0.0.2:8124","ready":true},
                {"node_id":"node-3","advertised_address":"10.0.0.3:8124","ready":true}
            ],
            "membership":{"stable":true,"members":["node-1","node-2","node-3"]},
            "store":{"provider":"qualified-s3-compatible","bucket":"poc","prefix":"poc/cells","endpoint":"https://store.internal","reachable":true,"startup_probe_passed":true}
        })).unwrap()
    }

    #[test]
    fn fixture_observations_validate_shape_without_claiming_live_success() {
        let report = diagnose_fleet(request("fixture")).unwrap();
        assert_eq!(report.status, "NOT_RUN");
        assert!(!report.live_qualification);
        assert!(!report.mutating);
        assert_eq!(report.checks.len(), 6);
        assert!(report.checks.iter().all(|check| check.status == "PASS"));
    }

    #[test]
    fn mismatched_observation_fails_even_when_source_is_not_live() {
        let mut request = request("local");
        request.store.startup_probe_passed = false;
        let report = diagnose_fleet(request).unwrap();
        assert_eq!(report.status, "FAIL");
        assert!(!report.live_qualification);
        assert_eq!(report.checks.last().unwrap().status, "FAIL");
    }

    #[test]
    fn rejects_unknown_fields_and_sources() {
        let mut value = serde_json::to_value(request("fixture")).unwrap();
        value["self_declared_pass"] = json!(true);
        assert!(serde_json::from_value::<FleetDiagnoseRequest>(value).is_err());
        assert!(diagnose_fleet(request("claimed-pass")).is_err());
    }
}
