//! Normalized agent-plane peer identity.
//!
//! Phase 1 of the transport-security track introduces a single identity
//! keyspace before any listener defaults change. UDS peer credentials, vsock
//! CIDs, and mTLS URI-SANs all normalize to the same SPIFFE-shaped id.

use std::collections::HashMap;
use std::fmt;

use thiserror::Error;
use uuid::Uuid;

const AGENT_PATH_PREFIX: &str = "/agent/";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransportIdentityError {
    #[error("trust domain is empty")]
    EmptyTrustDomain,
    #[error("trust domain contains invalid characters: {0}")]
    InvalidTrustDomain(String),
    #[error("instance id is not a UUID: {0}")]
    InvalidInstanceId(String),
    #[error("SPIFFE id must start with spiffe://")]
    InvalidScheme,
    #[error("SPIFFE id must use /agent/<instance_id> path")]
    InvalidAgentPath,
    #[error("unknown UDS uid: {0}")]
    UnknownUdsUid(u32),
    #[error("unknown vsock CID: {0}")]
    UnknownVsockCid(u32),
    #[error("vsock CID must be a positive integer: {0}")]
    InvalidVsockCid(u32),
    #[error("duplicate vsock CID in map: {0}")]
    DuplicateVsockCid(u32),
    #[error("duplicate instance id in map: {0}")]
    DuplicateTransportInstanceId(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrustDomain(String);

impl TrustDomain {
    pub fn new(value: impl Into<String>) -> Result<Self, TransportIdentityError> {
        let value = value.into();
        if value.is_empty() {
            return Err(TransportIdentityError::EmptyTrustDomain);
        }
        if !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
        {
            return Err(TransportIdentityError::InvalidTrustDomain(value));
        }
        Ok(Self(value))
    }

    pub fn local_from_sandbox_identity(
        sandbox_identity: &str,
    ) -> Result<Self, TransportIdentityError> {
        Uuid::parse_str(sandbox_identity)
            .map_err(|_| TransportIdentityError::InvalidInstanceId(sandbox_identity.to_string()))?;
        Self::new(format!("sandbox-{sandbox_identity}.agentic.local"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TrustDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SpiffeId {
    uri: String,
    trust_domain: TrustDomain,
    instance_id: String,
}

impl SpiffeId {
    pub fn for_agent(
        trust_domain: TrustDomain,
        instance_id: impl Into<String>,
    ) -> Result<Self, TransportIdentityError> {
        let instance_id = instance_id.into();
        Uuid::parse_str(&instance_id)
            .map_err(|_| TransportIdentityError::InvalidInstanceId(instance_id.clone()))?;
        let uri = format!("spiffe://{trust_domain}{AGENT_PATH_PREFIX}{instance_id}");
        Ok(Self {
            uri,
            trust_domain,
            instance_id,
        })
    }

    pub fn parse(uri: impl Into<String>) -> Result<Self, TransportIdentityError> {
        let uri = uri.into();
        let rest = uri
            .strip_prefix("spiffe://")
            .ok_or(TransportIdentityError::InvalidScheme)?;
        let (trust_domain, path) = rest
            .split_once('/')
            .ok_or(TransportIdentityError::InvalidAgentPath)?;
        let instance_id = path
            .strip_prefix("agent/")
            .ok_or(TransportIdentityError::InvalidAgentPath)?;
        if instance_id.is_empty() || instance_id.contains('/') {
            return Err(TransportIdentityError::InvalidAgentPath);
        }
        let trust_domain = TrustDomain::new(trust_domain.to_string())?;
        Self::for_agent(trust_domain, instance_id.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.uri
    }

    pub fn trust_domain(&self) -> &TrustDomain {
        &self.trust_domain
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }
}

impl fmt::Display for SpiffeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.uri)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerIdentityEvidence {
    UdsPeerCred { uid: u32 },
    VsockCid { cid: u32 },
    MtlsUriSan { uri: String },
}

#[derive(Debug, Default, Clone)]
pub struct PeerIdentityMap {
    uds_uid_to_instance: HashMap<u32, String>,
    vsock_cid_to_instance: HashMap<u32, String>,
}

impl PeerIdentityMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_uds_uid(
        &mut self,
        uid: u32,
        instance_id: impl Into<String>,
    ) -> Result<(), TransportIdentityError> {
        let instance_id = instance_id.into();
        Uuid::parse_str(&instance_id)
            .map_err(|_| TransportIdentityError::InvalidInstanceId(instance_id.clone()))?;
        self.uds_uid_to_instance.insert(uid, instance_id);
        Ok(())
    }

    pub fn register_vsock_cid(
        &mut self,
        cid: u32,
        instance_id: impl Into<String>,
    ) -> Result<(), TransportIdentityError> {
        let instance_id = instance_id.into();
        if cid == 0 {
            return Err(TransportIdentityError::InvalidVsockCid(cid));
        }
        Uuid::parse_str(&instance_id)
            .map_err(|_| TransportIdentityError::InvalidInstanceId(instance_id.clone()))?;
        if self.vsock_cid_to_instance.contains_key(&cid) {
            return Err(TransportIdentityError::DuplicateVsockCid(cid));
        }
        if self.vsock_cid_to_instance.values().any(|id| id == &instance_id) {
            return Err(TransportIdentityError::DuplicateTransportInstanceId(
                instance_id,
            ));
        }
        self.vsock_cid_to_instance.insert(cid, instance_id);
        Ok(())
    }

    pub fn unregister_vsock_cid(&mut self, cid: u32) -> Option<String> {
        self.vsock_cid_to_instance.remove(&cid)
    }

    pub fn unregister_vsock_instance(&mut self, instance_id: &str) -> Option<u32> {
        let mut entry = None;
        for (key, value) in &self.vsock_cid_to_instance {
            if value == instance_id {
                entry = Some(*key);
                break;
            }
        }
        entry.and_then(|key| {
            self.vsock_cid_to_instance.remove(&key);
            Some(key)
        })
    }

    /// Build + validate a fresh vsock CID→instance map from `cid=instance`
    /// pairs, reusing `register_vsock_cid` validation (non-zero CID, valid
    /// instance UUID, no duplicate CID, no duplicate instance). Returns the
    /// validated map without mutating `self`, so a caller can validate before
    /// committing an atomic swap (used by the SIGHUP reload path, #577).
    pub fn build_vsock_map<I, S>(
        entries: I,
    ) -> Result<HashMap<u32, String>, TransportIdentityError>
    where
        I: IntoIterator<Item = (u32, S)>,
        S: Into<String>,
    {
        let mut tmp = PeerIdentityMap::new();
        for (cid, instance) in entries {
            tmp.register_vsock_cid(cid, instance)?;
        }
        Ok(tmp.vsock_cid_to_instance)
    }

    /// Atomically replace the vsock CID→instance entries. UDS and mTLS identity
    /// entries are left untouched. The caller must pass a pre-validated map
    /// (see `build_vsock_map`).
    pub fn replace_vsock_map(&mut self, vsock_cid_to_instance: HashMap<u32, String>) {
        self.vsock_cid_to_instance = vsock_cid_to_instance;
    }

    pub fn peer_identity(
        &self,
        evidence: PeerIdentityEvidence,
        trust_domain: &TrustDomain,
    ) -> Result<SpiffeId, TransportIdentityError> {
        match evidence {
            PeerIdentityEvidence::UdsPeerCred { uid } => {
                let instance_id = self
                    .uds_uid_to_instance
                    .get(&uid)
                    .ok_or(TransportIdentityError::UnknownUdsUid(uid))?;
                SpiffeId::for_agent(trust_domain.clone(), instance_id.clone())
            }
            PeerIdentityEvidence::VsockCid { cid } => {
                let instance_id = self
                    .vsock_cid_to_instance
                    .get(&cid)
                    .ok_or(TransportIdentityError::UnknownVsockCid(cid))?;
                SpiffeId::for_agent(trust_domain.clone(), instance_id.clone())
            }
            PeerIdentityEvidence::MtlsUriSan { uri } => SpiffeId::parse(uri),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SANDBOX_ID: &str = "018fb9f0-0a3e-7c1d-8a42-6b2c2bb4c3ad";
    const INSTANCE_ID: &str = "018fb9f1-3291-7a73-b261-c7de8a2af4d1";
    const OTHER_INSTANCE_ID: &str = "018fb9f2-94a1-7c2d-b0c4-01fd58bb5ec1";

    #[test]
    fn local_trust_domain_is_derived_from_sandbox_identity() {
        let trust_domain = TrustDomain::local_from_sandbox_identity(SANDBOX_ID).unwrap();
        assert_eq!(
            trust_domain.as_str(),
            "sandbox-018fb9f0-0a3e-7c1d-8a42-6b2c2bb4c3ad.agentic.local"
        );
    }

    #[test]
    fn spiffe_id_round_trips_and_exposes_instance_id() {
        let trust_domain = TrustDomain::local_from_sandbox_identity(SANDBOX_ID).unwrap();
        let id = SpiffeId::for_agent(trust_domain.clone(), INSTANCE_ID).unwrap();

        assert_eq!(
            id.as_str(),
            "spiffe://sandbox-018fb9f0-0a3e-7c1d-8a42-6b2c2bb4c3ad.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1"
        );
        assert_eq!(id.trust_domain(), &trust_domain);
        assert_eq!(id.instance_id(), INSTANCE_ID);
        assert_eq!(SpiffeId::parse(id.as_str()).unwrap(), id);
    }

    #[test]
    fn rejects_invalid_spiffe_uri_shapes() {
        assert_eq!(
            SpiffeId::parse("https://sandbox.example/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1")
                .unwrap_err(),
            TransportIdentityError::InvalidScheme
        );
        assert_eq!(
            SpiffeId::parse(
                "spiffe://sandbox.example/not-agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1"
            )
            .unwrap_err(),
            TransportIdentityError::InvalidAgentPath
        );
        assert_eq!(
            SpiffeId::parse("spiffe://sandbox.example/agent/not-a-uuid").unwrap_err(),
            TransportIdentityError::InvalidInstanceId("not-a-uuid".to_string())
        );
    }

    #[test]
    fn uds_and_vsock_sources_normalize_to_same_agent_identity() {
        let trust_domain = TrustDomain::local_from_sandbox_identity(SANDBOX_ID).unwrap();
        let mut map = PeerIdentityMap::new();
        map.register_uds_uid(1001, INSTANCE_ID).unwrap();
        map.register_vsock_cid(42, INSTANCE_ID).unwrap();

        let uds_id = map
            .peer_identity(
                PeerIdentityEvidence::UdsPeerCred { uid: 1001 },
                &trust_domain,
            )
            .unwrap();
        let vsock_id = map
            .peer_identity(PeerIdentityEvidence::VsockCid { cid: 42 }, &trust_domain)
            .unwrap();

        assert_eq!(uds_id, vsock_id);
        assert_eq!(uds_id.instance_id(), INSTANCE_ID);
    }

    #[test]
    fn rejects_zero_vsock_cid() {
        let mut map = PeerIdentityMap::new();

        assert_eq!(
            map.register_vsock_cid(0, INSTANCE_ID).unwrap_err(),
            TransportIdentityError::InvalidVsockCid(0)
        );
    }

    #[test]
    fn rejects_duplicate_vsock_cid_entries() {
        let mut map = PeerIdentityMap::new();

        map.register_vsock_cid(42, INSTANCE_ID).unwrap();

        assert_eq!(
            map.register_vsock_cid(42, OTHER_INSTANCE_ID).unwrap_err(),
            TransportIdentityError::DuplicateVsockCid(42)
        );
    }

    #[test]
    fn rejects_duplicate_vsock_instance_ids() {
        let mut map = PeerIdentityMap::new();

        map.register_vsock_cid(42, INSTANCE_ID).unwrap();

        assert_eq!(
            map.register_vsock_cid(43, INSTANCE_ID).unwrap_err(),
            TransportIdentityError::DuplicateTransportInstanceId(INSTANCE_ID.to_string())
        );
    }

    #[test]
    fn build_vsock_map_validates_and_replace_swaps_atomically() {
        let trust_domain = TrustDomain::local_from_sandbox_identity(SANDBOX_ID).unwrap();

        // A map with a UDS entry and an initial vsock entry.
        let mut map = PeerIdentityMap::new();
        map.register_uds_uid(1000, INSTANCE_ID).unwrap();
        map.register_vsock_cid(7, INSTANCE_ID).unwrap();

        // build_vsock_map enforces the same validation as register.
        assert_eq!(
            PeerIdentityMap::build_vsock_map([(0u32, INSTANCE_ID)]).unwrap_err(),
            TransportIdentityError::InvalidVsockCid(0)
        );
        assert_eq!(
            PeerIdentityMap::build_vsock_map([(3u32, INSTANCE_ID), (4u32, INSTANCE_ID)])
                .unwrap_err(),
            TransportIdentityError::DuplicateTransportInstanceId(INSTANCE_ID.to_string())
        );

        // A valid fresh map swaps in atomically, replacing the old vsock CID.
        let fresh =
            PeerIdentityMap::build_vsock_map([(9u32, OTHER_INSTANCE_ID)]).unwrap();
        map.replace_vsock_map(fresh);

        // Old vsock CID 7 is gone; new CID 9 resolves to the new instance.
        assert!(map
            .peer_identity(PeerIdentityEvidence::VsockCid { cid: 7 }, &trust_domain)
            .is_err());
        let resolved = map
            .peer_identity(PeerIdentityEvidence::VsockCid { cid: 9 }, &trust_domain)
            .unwrap();
        assert_eq!(resolved.instance_id(), OTHER_INSTANCE_ID);

        // UDS identity is untouched by the vsock swap.
        let uds = map
            .peer_identity(PeerIdentityEvidence::UdsPeerCred { uid: 1000 }, &trust_domain)
            .unwrap();
        assert_eq!(uds.instance_id(), INSTANCE_ID);
    }

    #[test]
    fn different_transport_peers_do_not_collide() {
        let trust_domain = TrustDomain::local_from_sandbox_identity(SANDBOX_ID).unwrap();
        let mut map = PeerIdentityMap::new();
        map.register_uds_uid(1001, INSTANCE_ID).unwrap();
        map.register_vsock_cid(43, OTHER_INSTANCE_ID).unwrap();

        let uds_id = map
            .peer_identity(
                PeerIdentityEvidence::UdsPeerCred { uid: 1001 },
                &trust_domain,
            )
            .unwrap();
        let vsock_id = map
            .peer_identity(PeerIdentityEvidence::VsockCid { cid: 43 }, &trust_domain)
            .unwrap();

        assert_ne!(uds_id, vsock_id);
    }

    #[test]
    fn unknown_kernel_peer_identity_is_rejected() {
        let trust_domain = TrustDomain::local_from_sandbox_identity(SANDBOX_ID).unwrap();
        let map = PeerIdentityMap::new();

        assert_eq!(
            map.peer_identity(
                PeerIdentityEvidence::UdsPeerCred { uid: 1001 },
                &trust_domain
            )
            .unwrap_err(),
            TransportIdentityError::UnknownUdsUid(1001)
        );
        assert_eq!(
            map.peer_identity(PeerIdentityEvidence::VsockCid { cid: 42 }, &trust_domain)
                .unwrap_err(),
            TransportIdentityError::UnknownVsockCid(42)
        );
    }

    #[test]
    fn mtls_uri_san_is_normalized_verbatim() {
        let uri = "spiffe://fleet.agentic.example/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";
        let map = PeerIdentityMap::new();
        let trust_domain = TrustDomain::local_from_sandbox_identity(SANDBOX_ID).unwrap();

        let id = map
            .peer_identity(
                PeerIdentityEvidence::MtlsUriSan {
                    uri: uri.to_string(),
                },
                &trust_domain,
            )
            .unwrap();

        assert_eq!(id.as_str(), uri);
        assert_eq!(id.trust_domain().as_str(), "fleet.agentic.example");
    }
}
