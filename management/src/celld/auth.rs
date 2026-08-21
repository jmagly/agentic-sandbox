use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::Path,
    sync::Mutex,
};
use zeroize::Zeroizing;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("authentication key file could not be read: {0}")]
    KeyRead(String),
    #[error("authentication key file must not be group/world accessible")]
    KeyPermissions,
    #[error("authentication key must contain at least 32 bytes")]
    WeakKey,
    #[error("invalid request signature")]
    InvalidSignature,
    #[error("request timestamp is outside the freshness window")]
    Stale,
    #[error("request nonce was already used")]
    Replay,
    #[error("request generation must be positive")]
    InvalidGeneration,
    #[error("invalid request timestamp")]
    InvalidTimestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SignedRequest {
    pub key_id: String,
    pub timestamp: String,
    pub nonce: String,
    pub generation: u64,
    pub operation_id: String,
    pub body_sha256: String,
    pub signature: String,
}

pub struct RequestSigner {
    key_id: String,
    key: Zeroizing<Vec<u8>>,
}

impl RequestSigner {
    pub fn from_file(key_id: impl Into<String>, path: &Path) -> Result<Self, AuthError> {
        verify_private_permissions(path)?;
        let key =
            Zeroizing::new(fs::read(path).map_err(|error| AuthError::KeyRead(error.to_string()))?);
        if key.len() < 32 {
            return Err(AuthError::WeakKey);
        }
        Ok(Self {
            key_id: key_id.into(),
            key,
        })
    }

    #[cfg(test)]
    pub(crate) fn from_bytes(key_id: &str, key: &[u8]) -> Self {
        Self {
            key_id: key_id.into(),
            key: Zeroizing::new(key.to_vec()),
        }
    }

    pub fn sign(
        &self,
        method: &str,
        path: &str,
        operation_id: &str,
        generation: u64,
        body: &[u8],
    ) -> Result<SignedRequest, AuthError> {
        if generation == 0 {
            return Err(AuthError::InvalidGeneration);
        }
        let timestamp = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let nonce = uuid::Uuid::now_v7().simple().to_string();
        let body_sha256 = hex::encode(Sha256::digest(body));
        let canonical = canonical(
            method,
            path,
            operation_id,
            generation,
            &timestamp,
            &nonce,
            &body_sha256,
        );
        let signature = sign_bytes(&self.key, canonical.as_bytes());
        Ok(SignedRequest {
            key_id: self.key_id.clone(),
            timestamp,
            nonce,
            generation,
            operation_id: operation_id.into(),
            body_sha256,
            signature,
        })
    }
}

struct VerificationKey {
    key_id: String,
    key: Zeroizing<Vec<u8>>,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
}

pub(crate) struct PreviousKeyFile<'a> {
    pub key_id: &'a str,
    pub path: &'a Path,
    pub valid_from: DateTime<Utc>,
    pub valid_until: DateTime<Utc>,
}

pub struct RequestVerifier {
    keys: Vec<VerificationKey>,
    max_skew: Duration,
    nonces: Mutex<NonceCache>,
}

#[derive(Default)]
struct NonceCache {
    seen: HashMap<String, DateTime<Utc>>,
    expiry_order: VecDeque<(DateTime<Utc>, String)>,
}

impl NonceCache {
    fn consume(
        &mut self,
        nonce: &str,
        observed_at: DateTime<Utc>,
        max_age: Duration,
    ) -> Result<(), AuthError> {
        while self
            .expiry_order
            .front()
            .is_some_and(|(seen_at, _)| observed_at.signed_duration_since(*seen_at) > max_age)
        {
            let (seen_at, expired) = self.expiry_order.pop_front().expect("front exists");
            if self.seen.get(&expired) == Some(&seen_at) {
                self.seen.remove(&expired);
            }
        }
        if self.seen.contains_key(nonce) {
            return Err(AuthError::Replay);
        }
        self.seen.insert(nonce.to_string(), observed_at);
        self.expiry_order
            .push_back((observed_at, nonce.to_string()));
        Ok(())
    }
}

impl RequestVerifier {
    pub(crate) fn from_files(
        key_id: impl Into<String>,
        path: &Path,
        previous: Option<PreviousKeyFile<'_>>,
        max_skew: Duration,
    ) -> Result<Self, AuthError> {
        let mut keys = vec![VerificationKey {
            key_id: key_id.into(),
            key: read_private_key(path)?,
            valid_from: None,
            valid_until: None,
        }];
        if let Some(previous) = previous {
            keys.push(VerificationKey {
                key_id: previous.key_id.into(),
                key: read_private_key(previous.path)?,
                valid_from: Some(previous.valid_from),
                valid_until: Some(previous.valid_until),
            });
        }
        Ok(Self {
            keys,
            max_skew,
            nonces: Mutex::new(NonceCache::default()),
        })
    }

    #[cfg(test)]
    pub(crate) fn from_bytes(key_id: &str, key: &[u8], max_skew: Duration) -> Self {
        Self::from_bytes_with_previous(key_id, key, None, max_skew)
    }

    #[cfg(test)]
    pub(crate) fn from_bytes_with_previous(
        key_id: &str,
        key: &[u8],
        previous: Option<(&str, &[u8], DateTime<Utc>, DateTime<Utc>)>,
        max_skew: Duration,
    ) -> Self {
        let mut keys = vec![VerificationKey {
            key_id: key_id.into(),
            key: Zeroizing::new(key.to_vec()),
            valid_from: None,
            valid_until: None,
        }];
        if let Some((key_id, key, valid_from, valid_until)) = previous {
            keys.push(VerificationKey {
                key_id: key_id.into(),
                key: Zeroizing::new(key.to_vec()),
                valid_from: Some(valid_from),
                valid_until: Some(valid_until),
            });
        }
        Self {
            keys,
            max_skew,
            nonces: Mutex::new(NonceCache::default()),
        }
    }

    pub fn verify(
        &self,
        signed: &SignedRequest,
        method: &str,
        path: &str,
        body: &[u8],
        now: DateTime<Utc>,
    ) -> Result<(), AuthError> {
        if signed.generation == 0 {
            return Err(AuthError::InvalidGeneration);
        }
        let timestamp = DateTime::parse_from_rfc3339(&signed.timestamp)
            .map_err(|_| AuthError::InvalidTimestamp)?
            .with_timezone(&Utc);
        if now.signed_duration_since(timestamp).abs() > self.max_skew {
            return Err(AuthError::Stale);
        }
        let body_sha256 = hex::encode(Sha256::digest(body));
        if body_sha256 != signed.body_sha256 {
            return Err(AuthError::InvalidSignature);
        }
        let canonical = canonical(
            method,
            path,
            &signed.operation_id,
            signed.generation,
            &signed.timestamp,
            &signed.nonce,
            &signed.body_sha256,
        );
        let signature = hex::decode(&signed.signature).map_err(|_| AuthError::InvalidSignature)?;
        let key = self
            .keys
            .iter()
            .find(|key| {
                key.key_id == signed.key_id
                    && key.valid_from.is_none_or(|valid_from| now >= valid_from)
                    && key.valid_until.is_none_or(|valid_until| now <= valid_until)
            })
            .ok_or(AuthError::InvalidSignature)?;
        let mut mac = HmacSha256::new_from_slice(&key.key).expect("HMAC accepts any key length");
        mac.update(canonical.as_bytes());
        mac.verify_slice(&signature)
            .map_err(|_| AuthError::InvalidSignature)?;

        self.nonces
            .lock()
            .expect("nonce lock poisoned")
            .consume(&signed.nonce, now, self.max_skew)
    }
}

fn read_private_key(path: &Path) -> Result<Zeroizing<Vec<u8>>, AuthError> {
    verify_private_permissions(path)?;
    let key =
        Zeroizing::new(fs::read(path).map_err(|error| AuthError::KeyRead(error.to_string()))?);
    if key.len() < 32 {
        return Err(AuthError::WeakKey);
    }
    Ok(key)
}

fn sign_bytes(key: &[u8], value: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(value);
    hex::encode(mac.finalize().into_bytes())
}

fn canonical(
    method: &str,
    path: &str,
    operation_id: &str,
    generation: u64,
    timestamp: &str,
    nonce: &str,
    body_sha256: &str,
) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}",
        method.to_ascii_uppercase(),
        path,
        operation_id,
        generation,
        timestamp,
        nonce,
        body_sha256
    )
}

#[cfg(unix)]
pub(crate) fn verify_private_permissions(path: &Path) -> Result<(), AuthError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::metadata(path).map_err(|error| AuthError::KeyRead(error.to_string()))?;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(AuthError::KeyPermissions);
    }
    Ok(())
}
#[cfg(not(unix))]
pub(crate) fn verify_private_permissions(path: &Path) -> Result<(), AuthError> {
    fs::metadata(path).map_err(|error| AuthError::KeyRead(error.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const KEY: &[u8] = b"01234567890123456789012345678901";
    const PREVIOUS_KEY: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEF";
    #[test]
    fn authenticates_once_and_rejects_replay() {
        let signer = RequestSigner::from_bytes("active", KEY);
        let verifier = RequestVerifier::from_bytes("active", KEY, Duration::minutes(2));
        let signed = signer.sign("POST", "/cell", "op-1", 4, b"{}").unwrap();
        let now = DateTime::parse_from_rfc3339(&signed.timestamp)
            .unwrap()
            .with_timezone(&Utc);
        assert!(verifier
            .verify(&signed, "POST", "/cell", b"{}", now)
            .is_ok());
        assert!(matches!(
            verifier.verify(&signed, "POST", "/cell", b"{}", now),
            Err(AuthError::Replay)
        ));
    }
    #[test]
    fn rejects_tampering_stale_requests_and_zero_generation() {
        let signer = RequestSigner::from_bytes("active", KEY);
        let verifier = RequestVerifier::from_bytes("active", KEY, Duration::seconds(30));
        let signed = signer.sign("POST", "/cell", "op-1", 4, b"{}").unwrap();
        let issued = DateTime::parse_from_rfc3339(&signed.timestamp)
            .unwrap()
            .with_timezone(&Utc);
        assert!(matches!(
            verifier.verify(&signed, "POST", "/cell", b"tampered", issued),
            Err(AuthError::InvalidSignature)
        ));
        assert!(matches!(
            verifier.verify(
                &signed,
                "POST",
                "/cell",
                b"{}",
                issued + Duration::minutes(2)
            ),
            Err(AuthError::Stale)
        ));
        assert!(matches!(
            signer.sign("POST", "/cell", "op", 0, b"{}"),
            Err(AuthError::InvalidGeneration)
        ));
    }

    #[test]
    fn previous_key_is_accepted_only_inside_the_rotation_window() {
        let previous_signer = RequestSigner::from_bytes("previous", PREVIOUS_KEY);
        let signed = previous_signer
            .sign("POST", "/cell", "op-rotation", 4, b"{}")
            .unwrap();
        let issued = DateTime::parse_from_rfc3339(&signed.timestamp)
            .unwrap()
            .with_timezone(&Utc);
        let verifier = RequestVerifier::from_bytes_with_previous(
            "active",
            KEY,
            Some((
                "previous",
                PREVIOUS_KEY,
                issued - Duration::seconds(1),
                issued + Duration::seconds(30),
            )),
            Duration::minutes(2),
        );

        assert!(verifier
            .verify(&signed, "POST", "/cell", b"{}", issued)
            .is_ok());

        let outside_window = previous_signer
            .sign("POST", "/cell", "op-after-rotation", 4, b"{}")
            .unwrap();
        assert!(matches!(
            verifier.verify(
                &outside_window,
                "POST",
                "/cell",
                b"{}",
                issued + Duration::seconds(31)
            ),
            Err(AuthError::InvalidSignature)
        ));
    }

    #[test]
    fn one_thousand_attempts_per_negative_class_fail_closed() {
        const WRONG_KEY: &[u8] = b"wrong-key-01234567890123456789012";
        const ATTEMPTS: usize = 1_000;
        let signer = RequestSigner::from_bytes("active", KEY);
        let wrong_signer = RequestSigner::from_bytes("active", WRONG_KEY);
        let verifier = RequestVerifier::from_bytes("active", KEY, Duration::minutes(2));

        for attempt in 0..ATTEMPTS {
            let operation_id = format!("op-negative-{attempt}");

            let signed = signer
                .sign("POST", "/cell", &operation_id, 1, b"{}")
                .unwrap();
            let issued = DateTime::parse_from_rfc3339(&signed.timestamp)
                .unwrap()
                .with_timezone(&Utc);
            assert!(matches!(
                verifier.verify(&signed, "POST", "/cell", b"{\"tampered\":true}", issued),
                Err(AuthError::InvalidSignature)
            ));

            assert!(matches!(
                verifier.verify(
                    &signed,
                    "POST",
                    "/cell",
                    b"{}",
                    issued + Duration::seconds(121)
                ),
                Err(AuthError::Stale)
            ));

            let wrong_key = wrong_signer
                .sign("POST", "/cell", &operation_id, 1, b"{}")
                .unwrap();
            assert!(matches!(
                verifier.verify(&wrong_key, "POST", "/cell", b"{}", issued),
                Err(AuthError::InvalidSignature)
            ));

            let mut zero_generation = signed.clone();
            zero_generation.generation = 0;
            assert!(matches!(
                verifier.verify(&zero_generation, "POST", "/cell", b"{}", issued),
                Err(AuthError::InvalidGeneration)
            ));

            let mut wrong_generation = signed.clone();
            wrong_generation.generation = 2;
            assert!(matches!(
                verifier.verify(&wrong_generation, "POST", "/cell", b"{}", issued),
                Err(AuthError::InvalidSignature)
            ));

            assert!(verifier
                .verify(&signed, "POST", "/cell", b"{}", issued)
                .is_ok());
            assert!(matches!(
                verifier.verify(&signed, "POST", "/cell", b"{}", issued),
                Err(AuthError::Replay)
            ));
        }
    }

    #[test]
    fn nonce_cache_handles_qualification_volume_without_full_map_scans() {
        let mut cache = NonceCache::default();
        let started = Utc::now();
        for attempt in 0..80_000 {
            cache
                .consume(
                    &format!("qualification-{attempt}"),
                    started + Duration::milliseconds(attempt),
                    Duration::minutes(2),
                )
                .unwrap();
        }
        assert!(cache.seen.len() <= 120_001);
        assert!(matches!(
            cache.consume(
                "qualification-79999",
                started + Duration::seconds(80),
                Duration::minutes(2)
            ),
            Err(AuthError::Replay)
        ));
        cache
            .consume(
                "after-expiry",
                started + Duration::minutes(4),
                Duration::minutes(2),
            )
            .unwrap();
        assert_eq!(cache.seen.len(), 1);
        assert_eq!(cache.expiry_order.len(), 1);
    }
}
