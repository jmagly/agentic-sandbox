use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fs, path::Path, sync::Mutex};
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

pub struct RequestVerifier {
    key_id: String,
    key: Zeroizing<Vec<u8>>,
    max_skew: Duration,
    nonces: Mutex<HashMap<String, DateTime<Utc>>>,
}

impl RequestVerifier {
    pub fn from_file(
        key_id: impl Into<String>,
        path: &Path,
        max_skew: Duration,
    ) -> Result<Self, AuthError> {
        verify_private_permissions(path)?;
        let key =
            Zeroizing::new(fs::read(path).map_err(|error| AuthError::KeyRead(error.to_string()))?);
        if key.len() < 32 {
            return Err(AuthError::WeakKey);
        }
        Ok(Self {
            key_id: key_id.into(),
            key,
            max_skew,
            nonces: Mutex::new(HashMap::new()),
        })
    }

    #[cfg(test)]
    pub(crate) fn from_bytes(key_id: &str, key: &[u8], max_skew: Duration) -> Self {
        Self {
            key_id: key_id.into(),
            key: Zeroizing::new(key.to_vec()),
            max_skew,
            nonces: Mutex::new(HashMap::new()),
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
        if signed.key_id != self.key_id {
            return Err(AuthError::InvalidSignature);
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
        let mut mac = HmacSha256::new_from_slice(&self.key).expect("HMAC accepts any key length");
        mac.update(canonical.as_bytes());
        mac.verify_slice(&signature)
            .map_err(|_| AuthError::InvalidSignature)?;

        let mut nonces = self.nonces.lock().expect("nonce lock poisoned");
        nonces.retain(|_, seen| now.signed_duration_since(*seen) <= self.max_skew);
        if nonces.insert(signed.nonce.clone(), timestamp).is_some() {
            return Err(AuthError::Replay);
        }
        Ok(())
    }
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
fn verify_private_permissions(path: &Path) -> Result<(), AuthError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::metadata(path).map_err(|error| AuthError::KeyRead(error.to_string()))?;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(AuthError::KeyPermissions);
    }
    Ok(())
}
#[cfg(not(unix))]
fn verify_private_permissions(path: &Path) -> Result<(), AuthError> {
    fs::metadata(path).map_err(|error| AuthError::KeyRead(error.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const KEY: &[u8] = b"01234567890123456789012345678901";
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
}
