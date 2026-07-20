//! Certificate renewal scheduling and expiry-gate policy.
//!
//! This module is deliberately independent of certificate storage and CA
//! providers. Callers supply parsed validity timestamps and a stable,
//! non-secret identity seed. This keeps renewal behavior consistent across the
//! embedded CA and out-of-process enterprise providers.

use anyhow::{bail, Result};
use sha2::{Digest, Sha256};
use std::io::BufReader;

const SECONDS_PER_DAY: i64 = 24 * 60 * 60;

/// A deterministic renewal point in the second half of a leaf's lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenewalSchedule {
    pub renew_at_unix: i64,
    pub jitter_seconds: i64,
}

/// Operational expiry gates for CA and intermediate certificates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpiryGate {
    Healthy,
    Attention30Days,
    Warning7Days,
    Critical1Day,
    Expired,
}

impl ExpiryGate {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Attention30Days => "attention_30_days",
            Self::Warning7Days => "warning_7_days",
            Self::Critical1Day => "critical_1_day",
            Self::Expired => "expired",
        }
    }
}

/// Parse the first PEM certificate's validity interval as Unix timestamps.
pub fn certificate_validity_from_pem(pem: &[u8]) -> Result<(i64, i64)> {
    let mut reader = BufReader::new(pem);
    let cert = rustls_pemfile::certs(&mut reader)
        .next()
        .transpose()?
        .ok_or_else(|| anyhow::anyhow!("certificate PEM contains no certificate"))?;
    let (_, parsed) = x509_parser::parse_x509_certificate(cert.as_ref())
        .map_err(|error| anyhow::anyhow!("parsing certificate validity: {error}"))?;
    Ok((
        parsed.validity().not_before.timestamp(),
        parsed.validity().not_after.timestamp(),
    ))
}

/// Schedule renewal at 50% of the certificate lifetime plus up to 10% jitter.
///
/// `identity_seed` should be stable for the workload and certificate
/// generation (for example, SPIFFE ID plus certificate serial). It is hashed
/// locally and must not contain private-key material.
pub fn renewal_schedule(
    not_before_unix: i64,
    not_after_unix: i64,
    identity_seed: &[u8],
) -> Result<RenewalSchedule> {
    if not_after_unix <= not_before_unix {
        bail!("certificate validity interval must be positive");
    }

    let lifetime = not_after_unix - not_before_unix;
    let half_life = lifetime / 2;
    let jitter_window = lifetime / 10;
    let jitter_seconds = if jitter_window == 0 {
        0
    } else {
        let digest = Sha256::digest(identity_seed);
        let sample = u64::from_be_bytes(digest[..8].try_into().expect("fixed digest slice"));
        i64::try_from(sample % (jitter_window as u64 + 1)).expect("jitter is bounded by i64")
    };

    // A positive interval always leaves at least one second after this point.
    let renew_at_unix = not_before_unix
        .saturating_add(half_life)
        .saturating_add(jitter_seconds)
        .min(not_after_unix - 1);

    Ok(RenewalSchedule {
        renew_at_unix,
        jitter_seconds,
    })
}

/// Classify a CA or intermediate certificate against the 30/7/1-day gates.
pub fn expiry_gate(now_unix: i64, not_after_unix: i64) -> ExpiryGate {
    let remaining = not_after_unix.saturating_sub(now_unix);
    if remaining <= 0 {
        ExpiryGate::Expired
    } else if remaining <= SECONDS_PER_DAY {
        ExpiryGate::Critical1Day
    } else if remaining <= 7 * SECONDS_PER_DAY {
        ExpiryGate::Warning7Days
    } else if remaining <= 30 * SECONDS_PER_DAY {
        ExpiryGate::Attention30Days
    } else {
        ExpiryGate::Healthy
    }
}

/// Return whether a leaf has reached its deterministic renewal point.
pub fn leaf_renewal_due(
    now_unix: i64,
    not_before_unix: i64,
    not_after_unix: i64,
    identity_seed: &[u8],
) -> Result<bool> {
    Ok(now_unix >= renewal_schedule(not_before_unix, not_after_unix, identity_seed)?.renew_at_unix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertificateParams, KeyPair};

    #[test]
    fn renewal_occurs_between_half_life_and_sixty_percent() {
        let schedule = renewal_schedule(1_000, 4_600, b"spiffe://example/agent/a:42").unwrap();
        assert!(schedule.renew_at_unix >= 2_800);
        assert!(schedule.renew_at_unix <= 3_160);
        assert_eq!(schedule.renew_at_unix, 2_800 + schedule.jitter_seconds);
    }

    #[test]
    fn renewal_jitter_is_stable_for_same_identity() {
        let first = renewal_schedule(0, 3_600, b"workload-a:serial-1").unwrap();
        let second = renewal_schedule(0, 3_600, b"workload-a:serial-1").unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn renewal_rejects_invalid_validity_interval() {
        assert!(renewal_schedule(10, 10, b"identity").is_err());
        assert!(renewal_schedule(11, 10, b"identity").is_err());
    }

    #[test]
    fn short_lifetime_schedule_stays_before_expiry() {
        let schedule = renewal_schedule(100, 101, b"identity").unwrap();
        assert_eq!(schedule.renew_at_unix, 100);
        assert_eq!(schedule.jitter_seconds, 0);
    }

    #[test]
    fn leaf_due_uses_computed_schedule_boundary() {
        let schedule = renewal_schedule(0, 3_600, b"identity").unwrap();
        assert!(!leaf_renewal_due(schedule.renew_at_unix - 1, 0, 3_600, b"identity").unwrap());
        assert!(leaf_renewal_due(schedule.renew_at_unix, 0, 3_600, b"identity").unwrap());
    }

    #[test]
    fn expiry_gate_boundaries_are_inclusive() {
        let now = 1_000_000;
        assert_eq!(expiry_gate(now, now), ExpiryGate::Expired);
        assert_eq!(
            expiry_gate(now, now + SECONDS_PER_DAY),
            ExpiryGate::Critical1Day
        );
        assert_eq!(
            expiry_gate(now, now + SECONDS_PER_DAY + 1),
            ExpiryGate::Warning7Days
        );
        assert_eq!(
            expiry_gate(now, now + 7 * SECONDS_PER_DAY),
            ExpiryGate::Warning7Days
        );
        assert_eq!(
            expiry_gate(now, now + 7 * SECONDS_PER_DAY + 1),
            ExpiryGate::Attention30Days
        );
        assert_eq!(
            expiry_gate(now, now + 30 * SECONDS_PER_DAY),
            ExpiryGate::Attention30Days
        );
        assert_eq!(
            expiry_gate(now, now + 30 * SECONDS_PER_DAY + 1),
            ExpiryGate::Healthy
        );
    }

    #[test]
    fn parses_pem_validity_interval() {
        let key = KeyPair::generate().unwrap();
        let cert = CertificateParams::new(vec!["localhost".to_string()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let (not_before, not_after) = certificate_validity_from_pem(cert.pem().as_bytes()).unwrap();
        assert!(not_after > not_before);
    }
}
