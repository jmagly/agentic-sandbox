use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use super::WORKER_CELLD_CAPABILITIES;

pub const SUPPORTED_CELLD_VERSION: &str = "v0.2.1";
pub const SUPPORTED_PROTOCOL_VERSION: &str = "celld-internal-v1";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("{field}: {message}")]
    Field {
        field: &'static str,
        message: String,
    },
}

fn invalid(field: &'static str, message: impl Into<String>) -> ValidationError {
    ValidationError::Field {
        field,
        message: message.into(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkerBundleManifest {
    pub api_version: String,
    pub name: String,
    pub version: String,
    pub entrypoint: String,
    pub digest: String,
    #[serde(default)]
    pub signature: Option<BundleSignature>,
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub outbound_http: Option<OutboundHttp>,
    pub limits: WorkerLimits,
    pub tenancy: WorkerTenancy,
    #[serde(default)]
    pub compatibility: Option<WorkerCompatibility>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BundleSignature {
    pub kind: String,
    pub identity: String,
    pub bundle: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OutboundHttp {
    pub allowed_origins: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkerLimits {
    pub cpu_ms_per_request: u64,
    pub memory_mb: u64,
    pub requests_per_minute: u64,
    pub storage_mb: u64,
    pub resident_cells: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkerTenancy {
    pub trust_domain: String,
    pub hostile_multi_tenant: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkerCompatibility {
    #[serde(default)]
    pub celld_version: Option<String>,
    #[serde(default)]
    pub compatibility_date: Option<String>,
}

impl WorkerBundleManifest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.api_version != "worker-celld.agentic-sandbox/v1" {
            return Err(invalid("api_version", "unsupported bundle contract"));
        }
        if !valid_slug(&self.name) {
            return Err(invalid("name", "must be a DNS-style slug"));
        }
        if self.version.is_empty() || self.version.len() > 64 {
            return Err(invalid("version", "must contain 1..64 characters"));
        }
        if self.entrypoint.starts_with('/')
            || self.entrypoint.split('/').any(|part| part == "..")
            || !(self.entrypoint.ends_with(".js")
                || self.entrypoint.ends_with(".mjs")
                || self.entrypoint.ends_with(".wasm"))
        {
            return Err(invalid(
                "entrypoint",
                "must be a relative JS or Wasm path without traversal",
            ));
        }
        validate_digest("digest", &self.digest)?;
        if let Some(signature) = &self.signature {
            if !["sigstore", "ed25519"].contains(&signature.kind.as_str())
                || signature.identity.trim().is_empty()
                || signature.bundle.trim().is_empty()
            {
                return Err(invalid(
                    "signature",
                    "requires a supported kind, identity, and verification bundle",
                ));
            }
        }
        let allowed = WORKER_CELLD_CAPABILITIES
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let mut seen = BTreeSet::new();
        for capability in &self.capabilities {
            if !allowed.contains(capability.as_str()) {
                return Err(invalid(
                    "capabilities",
                    format!("unsupported capability {capability}"),
                ));
            }
            if !seen.insert(capability) {
                return Err(invalid(
                    "capabilities",
                    format!("duplicate capability {capability}"),
                ));
            }
        }
        if self.tenancy.hostile_multi_tenant {
            return Err(invalid(
                "tenancy.hostile_multi_tenant",
                "Celld is not approved for hostile multi-tenancy",
            ));
        }
        if self.tenancy.trust_domain.trim().is_empty() {
            return Err(invalid("tenancy.trust_domain", "is required"));
        }
        if [
            self.limits.cpu_ms_per_request,
            self.limits.memory_mb,
            self.limits.requests_per_minute,
            self.limits.storage_mb,
            self.limits.resident_cells,
        ]
        .contains(&0)
        {
            return Err(invalid("limits", "all resource limits must be positive"));
        }
        if let Some(http) = &self.outbound_http {
            let mut origins = BTreeSet::new();
            for origin in &http.allowed_origins {
                if !origins.insert(origin) {
                    return Err(invalid(
                        "outbound_http.allowed_origins",
                        "origins must be unique",
                    ));
                }
                let parsed = reqwest::Url::parse(origin).map_err(|_| {
                    invalid("outbound_http.allowed_origins", "contains an invalid URL")
                })?;
                let loopback = parsed.host_str().is_some_and(|host| {
                    host == "localhost"
                        || host
                            .parse::<std::net::IpAddr>()
                            .is_ok_and(|ip| ip.is_loopback())
                });
                if parsed.scheme() != "https" && !loopback {
                    return Err(invalid(
                        "outbound_http.allowed_origins",
                        "non-loopback origins must use https",
                    ));
                }
            }
        }
        if self
            .compatibility
            .as_ref()
            .and_then(|c| c.celld_version.as_deref())
            .is_some_and(|v| v != SUPPORTED_CELLD_VERSION)
        {
            return Err(invalid(
                "compatibility.celld_version",
                format!("only {SUPPORTED_CELLD_VERSION} is qualified"),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CelldFleetManifest {
    pub api_version: String,
    pub fleet_id: String,
    pub trust_domain: String,
    pub celld: CelldArtifact,
    #[serde(default)]
    pub application_bundle: Option<String>,
    pub nodes: FleetNodes,
    pub storage: FleetStorage,
    pub network: FleetNetwork,
    pub resources: FleetResources,
    pub telemetry: FleetTelemetry,
    pub retention: FleetRetention,
    pub rollout: FleetRollout,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CelldArtifact {
    pub version: String,
    pub artifact: String,
    pub digest: String,
    pub protocol_version: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FleetNodes {
    pub count: u32,
    pub substrate: String,
    pub architecture: String,
    pub reserve: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FleetStorage {
    pub provider: String,
    pub bucket: String,
    pub prefix: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    pub credential_ref: String,
    pub retain_on_destroy: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FleetNetwork {
    pub public_listener: String,
    pub internal_listener: String,
    pub advertised_addresses: Vec<String>,
    pub encrypted_overlay: bool,
    pub public_ingress_excludes_internal: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FleetResources {
    pub cpu_cores: f64,
    pub memory_mb: u64,
    pub max_resident_cells: u64,
    pub max_rss_mb: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FleetTelemetry {
    pub metrics: bool,
    pub structured_logs: bool,
    pub trace_propagation: String,
    pub redaction_profile: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FleetRetention {
    pub cell_tombstone_days: u64,
    pub audit_days: u64,
    pub backup_rpo_seconds: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FleetRollout {
    pub max_unavailable: u32,
    pub drain_timeout_seconds: u64,
    pub compatible_from: Vec<String>,
}

impl CelldFleetManifest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.api_version != "celld-fleet.agentic-sandbox/v1" {
            return Err(invalid("api_version", "unsupported fleet contract"));
        }
        if !valid_slug(&self.fleet_id) {
            return Err(invalid("fleet_id", "must be a DNS-style slug"));
        }
        if self.trust_domain.trim().is_empty() {
            return Err(invalid("trust_domain", "is required"));
        }
        if self.celld.version != SUPPORTED_CELLD_VERSION {
            return Err(invalid(
                "celld.version",
                format!("only {SUPPORTED_CELLD_VERSION} is qualified"),
            ));
        }
        if self.celld.protocol_version != SUPPORTED_PROTOCOL_VERSION {
            return Err(invalid(
                "celld.protocol_version",
                format!("must be {SUPPORTED_PROTOCOL_VERSION}"),
            ));
        }
        validate_digest("celld.digest", &self.celld.digest)?;
        reqwest::Url::parse(&self.celld.artifact)
            .map_err(|_| invalid("celld.artifact", "must be an absolute URL"))?;
        if let Some(digest) = &self.application_bundle {
            validate_digest("application_bundle", digest)?;
        }
        if self.nodes.count < 2 || self.nodes.reserve == 0 || self.nodes.reserve >= self.nodes.count
        {
            return Err(invalid(
                "nodes",
                "requires at least two nodes and 1 <= reserve < count",
            ));
        }
        if !["qemu", "docker", "host"].contains(&self.nodes.substrate.as_str()) {
            return Err(invalid("nodes.substrate", "must be qemu, docker, or host"));
        }
        if !["x86_64", "aarch64"].contains(&self.nodes.architecture.as_str()) {
            return Err(invalid("nodes.architecture", "must be x86_64 or aarch64"));
        }
        if !self
            .storage
            .prefix
            .trim_matches('/')
            .starts_with(&self.fleet_id)
        {
            return Err(invalid("storage.prefix", "must be scoped beneath fleet_id"));
        }
        if !["aws-s3", "google-cloud-storage", "qualified-s3-compatible"]
            .contains(&self.storage.provider.as_str())
        {
            return Err(invalid("storage.provider", "provider is not qualified"));
        }
        if self.storage.credential_ref.trim().is_empty()
            || self.storage.credential_ref.contains('=')
        {
            return Err(invalid(
                "storage.credential_ref",
                "must be an opaque broker reference, never inline credentials",
            ));
        }
        if self.network.public_listener == self.network.internal_listener
            || !self.network.public_ingress_excludes_internal
        {
            return Err(invalid(
                "network",
                "public and internal listeners must be distinct and ingress-separated",
            ));
        }
        if self.network.advertised_addresses.len() < self.nodes.count as usize {
            return Err(invalid(
                "network.advertised_addresses",
                "must advertise every node",
            ));
        }
        if self
            .network
            .advertised_addresses
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != self.network.advertised_addresses.len()
        {
            return Err(invalid(
                "network.advertised_addresses",
                "addresses must be unique",
            ));
        }
        if !self.network.encrypted_overlay
            && self
                .network
                .advertised_addresses
                .iter()
                .any(|address| is_public_address(address))
        {
            return Err(invalid(
                "network.advertised_addresses",
                "public addresses require an encrypted overlay",
            ));
        }
        if self.rollout.max_unavailable >= self.nodes.count - self.nodes.reserve {
            return Err(invalid(
                "rollout.max_unavailable",
                "would violate the node reserve",
            ));
        }
        if !self
            .rollout
            .compatible_from
            .iter()
            .any(|version| version == SUPPORTED_CELLD_VERSION)
        {
            return Err(invalid(
                "rollout.compatible_from",
                "must include the currently qualified version",
            ));
        }
        if self.resources.cpu_cores <= 0.0
            || self.resources.memory_mb < 256
            || self.resources.max_resident_cells == 0
            || self.resources.max_rss_mb == 0
        {
            return Err(invalid("resources", "resource ceilings are invalid"));
        }
        if !self.telemetry.metrics
            || !self.telemetry.structured_logs
            || self.telemetry.trace_propagation != "w3c"
            || self.telemetry.redaction_profile.trim().is_empty()
        {
            return Err(invalid(
                "telemetry",
                "metrics, structured logs, W3C trace propagation, and redaction are required",
            ));
        }
        if self.retention.cell_tombstone_days == 0
            || self.retention.audit_days == 0
            || self.retention.backup_rpo_seconds == 0
        {
            return Err(invalid(
                "retention",
                "retention and RPO values must be positive",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BucketPreflightEvidence {
    pub conditional_create: bool,
    pub conditional_overwrite: bool,
    pub read_after_write: bool,
    pub cleanup_verified: bool,
    pub latency_ms_p99: u64,
}

pub fn preflight_bucket(evidence: &BucketPreflightEvidence) -> Result<(), ValidationError> {
    if !(evidence.conditional_create
        && evidence.conditional_overwrite
        && evidence.read_after_write
        && evidence.cleanup_verified)
    {
        return Err(invalid(
            "storage.preflight",
            "conditional writes, read-after-write, and cleanup must all pass",
        ));
    }
    if evidence.latency_ms_p99 > 250 {
        return Err(invalid(
            "storage.preflight.latency_ms_p99",
            "must be <= 250 ms",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpgradePlan {
    pub from: String,
    pub to: String,
    pub node_count: u32,
    pub batch_size: u32,
    pub batches: u32,
    pub minimum_available: u32,
}

pub fn plan_upgrade(
    manifest: &CelldFleetManifest,
    from: &str,
    to: &str,
) -> Result<UpgradePlan, ValidationError> {
    manifest.validate()?;
    if to != SUPPORTED_CELLD_VERSION
        || !manifest
            .rollout
            .compatible_from
            .iter()
            .any(|version| version == from)
    {
        return Err(invalid(
            "rollout",
            "unqualified version pair; rolling update refused",
        ));
    }
    let batch_size = manifest.rollout.max_unavailable.max(1);
    Ok(UpgradePlan {
        from: from.into(),
        to: to.into(),
        node_count: manifest.nodes.count,
        batch_size,
        batches: manifest.nodes.count.div_ceil(batch_size),
        minimum_available: manifest.nodes.count - batch_size,
    })
}

fn validate_digest(field: &'static str, value: &str) -> Result<(), ValidationError> {
    let hex = value
        .strip_prefix("sha256:")
        .ok_or_else(|| invalid(field, "must use sha256"))?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(invalid(
            field,
            "must contain 64 lowercase hexadecimal characters",
        ));
    }
    Ok(())
}
fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .as_bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
fn is_public_address(value: &str) -> bool {
    value.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
        std::net::IpAddr::V4(ip) => !ip.is_loopback() && !ip.is_private() && !ip.is_link_local(),
        std::net::IpAddr::V6(ip) => {
            !ip.is_loopback() && !ip.is_unique_local() && !ip.is_unicast_link_local()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fleet() -> CelldFleetManifest {
        serde_json::from_value(serde_json::json!({
        "api_version":"celld-fleet.agentic-sandbox/v1","fleet_id":"poc","trust_domain":"poc.internal",
        "celld":{"version":"v0.2.1","artifact":"https://github.com/denoland/celld","digest":format!("sha256:{}", "a".repeat(64)),"protocol_version":"celld-internal-v1"},
        "nodes":{"count":3,"substrate":"qemu","architecture":"x86_64","reserve":1},
        "storage":{"provider":"aws-s3","bucket":"poc","prefix":"poc/cells","credential_ref":"broker://celld/poc","retain_on_destroy":true},
        "network":{"public_listener":"0.0.0.0:443","internal_listener":"10.0.0.1:8124","advertised_addresses":["10.0.0.1","10.0.0.2","10.0.0.3"],"encrypted_overlay":true,"public_ingress_excludes_internal":true},
        "resources":{"cpu_cores":2.0,"memory_mb":1024,"max_resident_cells":100,"max_rss_mb":768},
        "telemetry":{"metrics":true,"structured_logs":true,"trace_propagation":"w3c","redaction_profile":"celld-v1"},
        "retention":{"cell_tombstone_days":30,"audit_days":90,"backup_rpo_seconds":300},
        "rollout":{"max_unavailable":1,"drain_timeout_seconds":60,"compatible_from":["v0.2.1"]}
    })).unwrap()
    }
    #[test]
    fn accepts_qualified_fleet_and_plans_rollout() {
        let fleet = fleet();
        assert!(fleet.validate().is_ok());
        assert_eq!(
            plan_upgrade(&fleet, "v0.2.1", "v0.2.1")
                .unwrap()
                .minimum_available,
            2
        );
    }
    #[test]
    fn rejects_inline_credential() {
        let mut fleet = fleet();
        fleet.storage.credential_ref = "AWS_SECRET_ACCESS_KEY=x".into();
        assert!(fleet.validate().is_err());
    }
    #[test]
    fn rejects_os_capabilities_and_hostile_tenancy() {
        let mut manifest: WorkerBundleManifest = serde_json::from_value(serde_json::json!({
            "api_version":"worker-celld.agentic-sandbox/v1","name":"agent","version":"1",
            "entrypoint":"worker.mjs","digest":format!("sha256:{}", "a".repeat(64)),
            "capabilities":["task.exec"],
            "limits":{"cpu_ms_per_request":1,"memory_mb":1,"requests_per_minute":1,"storage_mb":1,"resident_cells":1},
            "tenancy":{"trust_domain":"test","hostile_multi_tenant":false}
        })).unwrap();
        assert!(manifest.validate().is_err());
        manifest.capabilities = vec!["worker.fetch".into()];
        manifest.tenancy.hostile_multi_tenant = true;
        assert!(manifest.validate().is_err());
    }
    #[test]
    fn refuses_incompatible_upgrade() {
        assert!(plan_upgrade(&fleet(), "v0.1.0", "v0.2.1").is_err());
    }
    #[test]
    fn preflight_requires_object_store_semantics() {
        assert!(preflight_bucket(&BucketPreflightEvidence {
            conditional_create: true,
            conditional_overwrite: false,
            read_after_write: true,
            cleanup_verified: true,
            latency_ms_p99: 10
        })
        .is_err());
    }
}
