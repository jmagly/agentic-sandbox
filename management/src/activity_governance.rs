//! Tenant-scoped activity governance, retention, and integrity manifests.

use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub standard_days: i64,
    pub security_days: i64,
    pub restricted_days: i64,
    pub maximum_hold_days: i64,
}
impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            standard_days: 30,
            security_days: 90,
            restricted_days: 7,
            maximum_hold_days: 365,
        }
    }
}
impl RetentionPolicy {
    pub fn validate(&self) -> Result<(), GovernanceError> {
        if self.standard_days <= 0
            || self.security_days <= 0
            || self.restricted_days <= 0
            || self.maximum_hold_days <= 0
        {
            return Err(GovernanceError::Invalid(
                "retention days must be positive".into(),
            ));
        }
        Ok(())
    }
    pub fn deadline(
        &self,
        class: &str,
        created: DateTime<Utc>,
    ) -> Result<DateTime<Utc>, GovernanceError> {
        let days = match class {
            "standard" => self.standard_days,
            "security" => self.security_days,
            "restricted" => self.restricted_days,
            "ephemeral" => 1,
            _ => return Err(GovernanceError::Invalid("unknown retention class".into())),
        };
        Ok(created + Duration::days(days))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    MetadataReader,
    SecurityReader,
    ContentCustodian,
    Administrator,
}
#[derive(Debug, Clone)]
pub struct AccessContext {
    pub actor_id: String,
    pub tenant_id: String,
    pub role: Role,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExactScope {
    pub tenant_id: String,
    pub instance_ids: BTreeSet<String>,
    pub event_names: BTreeSet<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentGrant {
    pub grant_id: String,
    pub actor_id: String,
    pub case_id: String,
    pub reason: String,
    pub scope: ExactScope,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub retention_days: i64,
}
impl ContentGrant {
    pub fn issue(
        actor_id: &str,
        case_id: &str,
        reason: &str,
        scope: ExactScope,
        duration: Duration,
        retention_days: i64,
        now: DateTime<Utc>,
    ) -> Result<Self, GovernanceError> {
        if actor_id.trim().is_empty()
            || case_id.trim().is_empty()
            || reason.trim().is_empty()
            || scope.tenant_id.trim().is_empty()
            || scope.instance_ids.is_empty()
            || scope.event_names.is_empty()
        {
            return Err(GovernanceError::Invalid(
                "content grant requires actor, case, reason, and exact scope".into(),
            ));
        }
        if duration <= Duration::zero()
            || duration > Duration::days(7)
            || !(1..=7).contains(&retention_days)
        {
            return Err(GovernanceError::Invalid(
                "content grant duration and retention must be 1-7 days".into(),
            ));
        }
        Ok(Self {
            grant_id: Uuid::now_v7().to_string(),
            actor_id: actor_id.into(),
            case_id: case_id.into(),
            reason: reason.into(),
            scope,
            issued_at: now,
            expires_at: now + duration,
            retention_days,
        })
    }
    pub fn authorizes(
        &self,
        access: &AccessContext,
        instance: &str,
        event: &str,
        now: DateTime<Utc>,
    ) -> bool {
        access.role == Role::ContentCustodian
            && access.actor_id == self.actor_id
            && access.tenant_id == self.scope.tenant_id
            && now < self.expires_at
            && self.scope.instance_ids.contains(instance)
            && self.scope.event_names.contains(event)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Hold {
    pub hold_id: String,
    pub tenant_id: String,
    pub case_id: String,
    pub reason: String,
    pub scope: ExactScope,
    pub expires_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovernanceAudit {
    pub action: String,
    pub actor_id: String,
    pub tenant_id: String,
    pub object_id: String,
    pub occurred_at: DateTime<Utc>,
    pub outcome: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GovernedExport {
    pub tenant_id: String,
    pub events: Vec<Value>,
    pub manifest: BatchManifest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestrictedContentEnvelope {
    pub content_id: String,
    pub tenant_id: String,
    pub instance_id: String,
    pub event_name: String,
    pub grant_id: String,
    pub key_id: String,
    pub nonce: String,
    pub ciphertext: String,
    pub expires_at: DateTime<Utc>,
}

impl RestrictedContentEnvelope {
    pub fn validate(
        &self,
        grant: &ContentGrant,
        now: DateTime<Utc>,
    ) -> Result<(), GovernanceError> {
        if now >= grant.expires_at
            || self.tenant_id != grant.scope.tenant_id
            || !grant.scope.instance_ids.contains(&self.instance_id)
            || !grant.scope.event_names.contains(&self.event_name)
            || self.grant_id != grant.grant_id
            || self.key_id.trim().is_empty()
            || self.nonce.trim().is_empty()
            || self.ciphertext.trim().is_empty()
            || self.expires_at > grant.issued_at + Duration::days(grant.retention_days)
        {
            return Err(GovernanceError::Denied);
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct GovernanceLedger {
    audits: Vec<GovernanceAudit>,
    holds: BTreeMap<String, Hold>,
}

impl GovernanceLedger {
    pub fn record_authorized(
        &mut self,
        access: &AccessContext,
        tenant: &str,
        action: &str,
        object_id: &str,
        security: bool,
        now: DateTime<Utc>,
    ) -> Result<GovernanceAudit, GovernanceError> {
        let allowed = match action {
            "query" | "export" => authorize_metadata(access, tenant, security),
            "verification" => {
                access.tenant_id == tenant
                    && matches!(access.role, Role::SecurityReader | Role::Administrator)
            }
            "policy_change" | "deletion" => access_admin(access, tenant),
            "redaction_failure" => {
                access.tenant_id == tenant
                    && matches!(access.role, Role::SecurityReader | Role::Administrator)
            }
            _ => false,
        };
        self.finish_audit(access, tenant, action, object_id, allowed, now)
    }

    pub fn record_content_read(
        &mut self,
        access: &AccessContext,
        grant: &ContentGrant,
        instance: &str,
        event: &str,
        object_id: &str,
        now: DateTime<Utc>,
    ) -> Result<GovernanceAudit, GovernanceError> {
        let allowed = grant.authorizes(access, instance, event, now);
        self.finish_audit(
            access,
            &grant.scope.tenant_id,
            "content_read",
            object_id,
            allowed,
            now,
        )
    }

    pub fn export_metadata(
        &mut self,
        access: &AccessContext,
        tenant: &str,
        collector: &str,
        events: Vec<Value>,
        key_id: &str,
        signing_key: &[u8],
        now: DateTime<Utc>,
    ) -> Result<GovernedExport, GovernanceError> {
        let allowed = authorize_metadata(access, tenant, false)
            && !events.is_empty()
            && events.iter().all(|event| {
                event
                    .pointer("/correlation/tenant_id")
                    .and_then(Value::as_str)
                    == Some(tenant)
                    && event.get("sensitivity").and_then(Value::as_str) == Some("metadata")
                    && contains_prohibited_content(event).is_none()
            });
        if !allowed {
            self.finish_audit(access, tenant, "export", collector, false, now)?;
            return Err(GovernanceError::Denied);
        }
        let manifest = BatchManifest::sign(tenant, collector, &events, None, key_id, signing_key)?;
        self.finish_audit(access, tenant, "export", &manifest.batch_id, true, now)?;
        Ok(GovernedExport {
            tenant_id: tenant.into(),
            events,
            manifest,
        })
    }

    pub fn create_hold(
        &mut self,
        access: &AccessContext,
        case_id: &str,
        reason: &str,
        scope: ExactScope,
        duration: Duration,
        policy: &RetentionPolicy,
        now: DateTime<Utc>,
    ) -> Result<Hold, GovernanceError> {
        let allowed = access_admin(access, &scope.tenant_id)
            && !case_id.trim().is_empty()
            && !reason.trim().is_empty()
            && duration > Duration::zero()
            && duration <= Duration::days(policy.maximum_hold_days);
        if !allowed {
            self.finish_audit(access, &scope.tenant_id, "hold", case_id, false, now)?;
            return Err(GovernanceError::Denied);
        }
        let hold = Hold {
            hold_id: Uuid::now_v7().to_string(),
            tenant_id: scope.tenant_id.clone(),
            case_id: case_id.into(),
            reason: reason.into(),
            scope,
            expires_at: now + duration,
        };
        self.holds.insert(hold.hold_id.clone(), hold.clone());
        self.finish_audit(access, &hold.tenant_id, "hold", &hold.hold_id, true, now)?;
        Ok(hold)
    }

    pub fn deletion_allowed(&self, tenant: &str, instance: &str, now: DateTime<Utc>) -> bool {
        !self.holds.values().any(|hold| {
            hold.tenant_id == tenant
                && hold.expires_at > now
                && hold.scope.instance_ids.contains(instance)
        })
    }

    pub fn audits(&self) -> &[GovernanceAudit] {
        &self.audits
    }

    fn finish_audit(
        &mut self,
        access: &AccessContext,
        tenant: &str,
        action: &str,
        object_id: &str,
        allowed: bool,
        now: DateTime<Utc>,
    ) -> Result<GovernanceAudit, GovernanceError> {
        let audit = GovernanceAudit {
            action: action.into(),
            actor_id: access.actor_id.clone(),
            tenant_id: tenant.into(),
            object_id: object_id.into(),
            occurred_at: now,
            outcome: if allowed {
                "allowed".into()
            } else {
                "denied".into()
            },
        };
        self.audits.push(audit.clone());
        if allowed {
            Ok(audit)
        } else {
            Err(GovernanceError::Denied)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BatchManifest {
    pub batch_id: String,
    pub tenant_id: String,
    pub collector_id: String,
    pub event_count: usize,
    pub merkle_root: String,
    pub previous_root: Option<String>,
    pub key_id: String,
    pub signature: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnchorCheckpoint {
    pub anchor_id: String,
    pub batch_id: String,
    pub root: String,
    pub anchored_at: DateTime<Utc>,
}
impl BatchManifest {
    pub fn sign(
        tenant: &str,
        collector: &str,
        events: &[Value],
        previous_root: Option<String>,
        key_id: &str,
        key: &[u8],
    ) -> Result<Self, GovernanceError> {
        if events.is_empty() || key.len() < 32 {
            return Err(GovernanceError::Invalid(
                "non-empty batch and >=256-bit signing key required".into(),
            ));
        }
        let mut leaves: Vec<String> = events
            .iter()
            .map(|event| {
                hex::encode(Sha256::digest(
                    serde_json::to_vec(event).expect("JSON value serializes"),
                ))
            })
            .collect();
        leaves.sort();
        let root = merkle_root(leaves);
        let mut manifest = Self {
            batch_id: Uuid::now_v7().to_string(),
            tenant_id: tenant.into(),
            collector_id: collector.into(),
            event_count: events.len(),
            merkle_root: root,
            previous_root,
            key_id: key_id.into(),
            signature: String::new(),
        };
        manifest.signature = signature(&manifest, key)?;
        Ok(manifest)
    }
    pub fn verify(
        &self,
        events: &[Value],
        key: &[u8],
        anchor: &AnchorCheckpoint,
    ) -> Result<(), GovernanceError> {
        if events.len() != self.event_count
            || anchor.batch_id != self.batch_id
            || anchor.root != self.merkle_root
        {
            return Err(GovernanceError::Integrity);
        }
        let mut leaves: Vec<String> = events
            .iter()
            .map(|event| hex::encode(Sha256::digest(serde_json::to_vec(event).unwrap())))
            .collect();
        leaves.sort();
        if merkle_root(leaves) != self.merkle_root || !verify_signature(self, key)? {
            return Err(GovernanceError::Integrity);
        }
        Ok(())
    }
    pub fn verify_chain(&self, previous: &BatchManifest) -> Result<(), GovernanceError> {
        if self.tenant_id != previous.tenant_id
            || self.collector_id != previous.collector_id
            || self.previous_root.as_deref() != Some(previous.merkle_root.as_str())
        {
            return Err(GovernanceError::Integrity);
        }
        Ok(())
    }
    pub fn checkpoint(&self, anchor_id: &str, now: DateTime<Utc>) -> AnchorCheckpoint {
        AnchorCheckpoint {
            anchor_id: anchor_id.into(),
            batch_id: self.batch_id.clone(),
            root: self.merkle_root.clone(),
            anchored_at: now,
        }
    }
}

pub fn authorize_metadata(access: &AccessContext, tenant: &str, security: bool) -> bool {
    access.tenant_id == tenant
        && match access.role {
            Role::Administrator => true,
            Role::SecurityReader => true,
            Role::MetadataReader => !security,
            Role::ContentCustodian => false,
        }
}
pub fn contains_prohibited_content(value: &Value) -> Option<String> {
    const BLOCKED: &[&str] = &[
        "prompt",
        "keystroke",
        "environment",
        "file_content",
        "packet_payload",
        "token",
        "cookie",
        "private_key",
        "authorization",
    ];
    match value {
        Value::Object(map) => map.iter().find_map(|(key, value)| {
            let lower = key.to_ascii_lowercase();
            if BLOCKED.iter().any(|item| lower.contains(item)) {
                Some(key.clone())
            } else {
                contains_prohibited_content(value)
            }
        }),
        Value::Array(items) => items.iter().find_map(contains_prohibited_content),
        _ => None,
    }
}
pub fn cryptographic_erasure_record(
    actor: &AccessContext,
    tenant: &str,
    key_id: &str,
    now: DateTime<Utc>,
) -> Result<GovernanceAudit, GovernanceError> {
    if access_admin(actor, tenant) {
        Ok(GovernanceAudit {
            action: "cryptographic_erasure".into(),
            actor_id: actor.actor_id.clone(),
            tenant_id: tenant.into(),
            object_id: key_id.into(),
            occurred_at: now,
            outcome: "key_destroyed".into(),
        })
    } else {
        Err(GovernanceError::Denied)
    }
}
fn access_admin(access: &AccessContext, tenant: &str) -> bool {
    access.tenant_id == tenant && access.role == Role::Administrator
}
fn signature(manifest: &BatchManifest, key: &[u8]) -> Result<String, GovernanceError> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| GovernanceError::Invalid("invalid signing key".into()))?;
    mac.update(
        format!(
            "{}\0{}\0{}\0{}\0{}\0{}\0{}",
            manifest.batch_id,
            manifest.tenant_id,
            manifest.collector_id,
            manifest.event_count,
            manifest.merkle_root,
            manifest.previous_root.as_deref().unwrap_or(""),
            manifest.key_id
        )
        .as_bytes(),
    );
    Ok(hex::encode(mac.finalize().into_bytes()))
}
fn verify_signature(manifest: &BatchManifest, key: &[u8]) -> Result<bool, GovernanceError> {
    let supplied = hex::decode(&manifest.signature).map_err(|_| GovernanceError::Integrity)?;
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| GovernanceError::Invalid("invalid signing key".into()))?;
    mac.update(
        format!(
            "{}\0{}\0{}\0{}\0{}\0{}\0{}",
            manifest.batch_id,
            manifest.tenant_id,
            manifest.collector_id,
            manifest.event_count,
            manifest.merkle_root,
            manifest.previous_root.as_deref().unwrap_or(""),
            manifest.key_id
        )
        .as_bytes(),
    );
    Ok(mac.verify_slice(&supplied).is_ok())
}
fn merkle_root(mut nodes: Vec<String>) -> String {
    while nodes.len() > 1 {
        if nodes.len() % 2 == 1 {
            nodes.push(nodes.last().unwrap().clone());
        }
        nodes = nodes
            .chunks(2)
            .map(|pair| hex::encode(Sha256::digest(format!("{}{}", pair[0], pair[1]).as_bytes())))
            .collect();
    }
    format!("sha256:{}", nodes.pop().unwrap())
}

#[derive(Debug, thiserror::Error)]
pub enum GovernanceError {
    #[error("invalid governance request: {0}")]
    Invalid(String),
    #[error("access denied")]
    Denied,
    #[error("manifest integrity verification failed")]
    Integrity,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn defaults_and_boundaries_are_exact() {
        let p = RetentionPolicy::default();
        assert_eq!(
            (p.standard_days, p.security_days, p.restricted_days),
            (30, 90, 7)
        );
        let now = Utc::now();
        assert_eq!(
            p.deadline("standard", now).unwrap(),
            now + Duration::days(30)
        );
    }
    #[test]
    fn restricted_grant_requires_exact_scope_and_expires() {
        let now = Utc::now();
        let access = AccessContext {
            actor_id: "analyst".into(),
            tenant_id: "a".into(),
            role: Role::ContentCustodian,
        };
        let grant = ContentGrant::issue(
            "analyst",
            "case-1",
            "investigation",
            ExactScope {
                tenant_id: "a".into(),
                instance_ids: ["i-1".into()].into_iter().collect(),
                event_names: ["session.output".into()].into_iter().collect(),
            },
            Duration::hours(2),
            7,
            now,
        )
        .unwrap();
        assert!(grant.authorizes(&access, "i-1", "session.output", now));
        assert!(!grant.authorizes(&access, "i-2", "session.output", now));
        assert!(!grant.authorizes(&access, "i-1", "session.output", now + Duration::hours(3)));
    }
    #[test]
    fn prohibited_content_is_rejected_recursively() {
        for key in [
            "prompt",
            "keystrokes",
            "environment",
            "file_content",
            "packet_payload",
            "access_token",
            "cookie",
            "private_key",
            "authorization_header",
        ] {
            assert!(contains_prohibited_content(&json!({"nested":{key:"never-store"}})).is_some());
        }
    }
    #[test]
    fn manifest_detects_mutation_removal_and_wrong_anchor() {
        let key = [7u8; 32];
        let events = vec![json!({"event_id":"a"}), json!({"event_id":"b"})];
        let manifest = BatchManifest::sign("t", "c", &events, None, "k1", &key).unwrap();
        let anchor = manifest.checkpoint("external-worm-1", Utc::now());
        manifest.verify(&events, &key, &anchor).unwrap();
        assert!(manifest.verify(&events[..1], &key, &anchor).is_err());
        let mutated = vec![json!({"event_id":"a"}), json!({"event_id":"changed"})];
        assert!(manifest.verify(&mutated, &key, &anchor).is_err());
        let mut bad = anchor;
        bad.root = "sha256:bad".into();
        assert!(manifest.verify(&events, &key, &bad).is_err());
    }
    #[test]
    fn tenant_and_sensitivity_authorization_is_consistent() {
        let reader = AccessContext {
            actor_id: "r".into(),
            tenant_id: "a".into(),
            role: Role::MetadataReader,
        };
        assert!(authorize_metadata(&reader, "a", false));
        assert!(!authorize_metadata(&reader, "a", true));
        assert!(!authorize_metadata(&reader, "b", false));
    }

    #[test]
    fn every_governance_operation_is_audited_including_denials() {
        let now = Utc::now();
        let reader = AccessContext {
            actor_id: "reader".into(),
            tenant_id: "a".into(),
            role: Role::MetadataReader,
        };
        let mut ledger = GovernanceLedger::default();
        ledger
            .record_authorized(&reader, "a", "query", "query-1", false, now)
            .unwrap();
        assert!(ledger
            .record_authorized(&reader, "a", "deletion", "batch-1", false, now)
            .is_err());
        assert_eq!(ledger.audits().len(), 2);
        assert_eq!(ledger.audits()[1].outcome, "denied");
    }

    #[test]
    fn holds_expire_and_block_deletion_only_for_exact_scope() {
        let now = Utc::now();
        let admin = AccessContext {
            actor_id: "admin".into(),
            tenant_id: "a".into(),
            role: Role::Administrator,
        };
        let mut ledger = GovernanceLedger::default();
        ledger
            .create_hold(
                &admin,
                "case-1",
                "preserve",
                ExactScope {
                    tenant_id: "a".into(),
                    instance_ids: ["i-1".into()].into_iter().collect(),
                    event_names: ["process.exec".into()].into_iter().collect(),
                },
                Duration::days(10),
                &RetentionPolicy::default(),
                now,
            )
            .unwrap();
        assert!(!ledger.deletion_allowed("a", "i-1", now));
        assert!(ledger.deletion_allowed("a", "i-2", now));
        assert!(ledger.deletion_allowed("a", "i-1", now + Duration::days(11)));
    }

    #[test]
    fn restricted_envelope_cannot_outlive_grant_retention() {
        let now = Utc::now();
        let scope = ExactScope {
            tenant_id: "a".into(),
            instance_ids: ["i-1".into()].into_iter().collect(),
            event_names: ["session.output".into()].into_iter().collect(),
        };
        let grant = ContentGrant::issue(
            "custodian",
            "case",
            "reason",
            scope,
            Duration::hours(1),
            7,
            now,
        )
        .unwrap();
        let mut envelope = RestrictedContentEnvelope {
            content_id: "content-1".into(),
            tenant_id: "a".into(),
            instance_id: "i-1".into(),
            event_name: "session.output".into(),
            grant_id: grant.grant_id.clone(),
            key_id: "restricted-key-v1".into(),
            nonce: "base64-nonce".into(),
            ciphertext: "base64-ciphertext".into(),
            expires_at: now + Duration::days(7),
        };
        envelope.validate(&grant, now).unwrap();
        envelope.expires_at = now + Duration::days(8);
        assert!(envelope.validate(&grant, now).is_err());
    }

    #[test]
    fn manifest_chain_and_key_identity_are_authenticated() {
        let key = [9u8; 32];
        let first = BatchManifest::sign("t", "c", &[json!({"n":1})], None, "key-v1", &key).unwrap();
        let second = BatchManifest::sign(
            "t",
            "c",
            &[json!({"n":2})],
            Some(first.merkle_root.clone()),
            "key-v2",
            &key,
        )
        .unwrap();
        second.verify_chain(&first).unwrap();
        let anchor = second.checkpoint("external", Utc::now());
        let mut tampered = second.clone();
        tampered.key_id = "attacker-key".into();
        assert!(tampered.verify(&[json!({"n":2})], &key, &anchor).is_err());
    }

    #[test]
    fn export_rejects_cross_tenant_and_prohibited_content_and_audits_both() {
        let now = Utc::now();
        let admin = AccessContext {
            actor_id: "admin".into(),
            tenant_id: "a".into(),
            role: Role::Administrator,
        };
        let mut ledger = GovernanceLedger::default();
        let event = |tenant: &str| json!({"correlation":{"tenant_id":tenant},"sensitivity":"metadata","payload":{"safe":"value"}});
        let exported = ledger
            .export_metadata(
                &admin,
                "a",
                "collector",
                vec![event("a")],
                "key-v1",
                &[3u8; 32],
                now,
            )
            .unwrap();
        assert_eq!(exported.events.len(), 1);
        assert!(ledger
            .export_metadata(
                &admin,
                "a",
                "collector",
                vec![event("b")],
                "key-v1",
                &[3u8; 32],
                now
            )
            .is_err());
        assert!(ledger.export_metadata(&admin, "a", "collector", vec![json!({"correlation":{"tenant_id":"a"},"sensitivity":"metadata","payload":{"authorization_header":"never"}})], "key-v1", &[3u8;32], now).is_err());
        assert_eq!(
            ledger
                .audits()
                .iter()
                .filter(|audit| audit.action == "export")
                .count(),
            3
        );
    }
}
