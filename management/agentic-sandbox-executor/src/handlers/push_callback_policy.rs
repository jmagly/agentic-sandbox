//! Outbound callback destination policy for A2A push notifications.
//!
//! Callback URLs are attacker-controlled network destinations.  The policy is
//! intentionally fail-closed: only public HTTPS endpoints on port 443 are
//! accepted, DNS is re-resolved before every attempt, every returned address
//! must be public, and the selected address is pinned into the HTTP client so a
//! second resolver lookup cannot redirect the connection.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use reqwest::Url;

pub(crate) const MAX_CONFIGS_PER_TASK: usize = 16;
pub(crate) const MAX_CONCURRENT_DELIVERIES: usize = 4;
const MAX_CALLBACK_URL_BYTES: usize = 2_048;

#[derive(Debug, Clone)]
pub(crate) struct ValidatedCallbackUrl {
    url: Url,
    host: String,
    port: u16,
}

impl ValidatedCallbackUrl {
    pub(crate) fn canonical_url(&self) -> String {
        self.url.to_string()
    }

    pub(crate) fn url(&self) -> &Url {
        &self.url
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CallbackPolicyError {
    #[error("callback URL is malformed")]
    Malformed,
    #[error("callback URL exceeds the length limit")]
    TooLong,
    #[error("callback URL must use HTTPS")]
    RequiresHttps,
    #[error("callback URL must not contain credentials")]
    Credentials,
    #[error("callback URL must not contain a fragment")]
    Fragment,
    #[error("callback URL must use port 443")]
    DisallowedPort,
    #[error("callback URL must contain an unambiguous host")]
    MissingHost,
    #[error("callback host did not resolve")]
    ResolutionFailed,
    #[error("callback host resolved to no addresses")]
    NoAddresses,
    #[error("callback destination is not a public address")]
    NonPublicAddress,
    #[error("callback HTTP client could not be constructed")]
    ClientBuild,
}

/// Parse and canonicalize the configuration-time URL surface.
pub(crate) fn validate_callback_url(
    raw: &str,
) -> Result<ValidatedCallbackUrl, CallbackPolicyError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(CallbackPolicyError::Malformed);
    }
    if raw.len() > MAX_CALLBACK_URL_BYTES {
        return Err(CallbackPolicyError::TooLong);
    }

    let url = Url::parse(raw).map_err(|_| CallbackPolicyError::Malformed)?;
    if url.scheme() != "https" {
        return Err(CallbackPolicyError::RequiresHttps);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(CallbackPolicyError::Credentials);
    }
    if url.fragment().is_some() {
        return Err(CallbackPolicyError::Fragment);
    }
    if url.port().is_some_and(|port| port != 443) {
        return Err(CallbackPolicyError::DisallowedPort);
    }

    let host = url
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or(CallbackPolicyError::MissingHost)?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();

    // The URL parser canonicalizes legacy integer/octal/hex IPv4 spellings.
    // Applying the address policy to the canonical host therefore rejects
    // encoded loopback and metadata forms as well as ordinary literals.
    if let Ok(ip) = host.parse::<IpAddr>() {
        require_public_ip(ip)?;
    }

    Ok(ValidatedCallbackUrl {
        url,
        host,
        port: 443,
    })
}

/// Resolve a callback immediately before one delivery attempt.
pub(crate) async fn resolve_public_destination(
    target: &ValidatedCallbackUrl,
    attempt: u32,
) -> Result<SocketAddr, CallbackPolicyError> {
    let addresses = if let Ok(ip) = target.host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, target.port)]
    } else {
        tokio::net::lookup_host((target.host.as_str(), target.port))
            .await
            .map_err(|_| CallbackPolicyError::ResolutionFailed)?
            .collect()
    };
    select_public_destination(addresses, attempt)
}

/// Require every DNS answer to be public, then choose one deterministically.
/// Rejecting mixed public/private answer sets closes rebinding and split-horizon
/// bypasses rather than merely selecting the first apparently safe address.
pub(crate) fn select_public_destination(
    mut addresses: Vec<SocketAddr>,
    attempt: u32,
) -> Result<SocketAddr, CallbackPolicyError> {
    if addresses.is_empty() {
        return Err(CallbackPolicyError::NoAddresses);
    }
    for address in &addresses {
        require_public_ip(address.ip())?;
    }
    addresses.sort_unstable_by_key(|address| (address.ip().to_string(), address.port()));
    addresses.dedup();
    let selected = attempt as usize % addresses.len();
    Ok(addresses[selected])
}

/// Build a one-attempt client whose resolver is pinned to the address that was
/// just validated.  TLS SNI and the HTTP Host header still use the configured
/// hostname.  Redirects are never followed and response bodies are never read.
pub(crate) fn build_pinned_client(
    target: &ValidatedCallbackUrl,
    address: SocketAddr,
) -> Result<reqwest::Client, CallbackPolicyError> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .pool_max_idle_per_host(0);
    if target.host.parse::<IpAddr>().is_err() {
        builder = builder.resolve(&target.host, address);
    }
    builder
        .build()
        .map_err(|_| CallbackPolicyError::ClientBuild)
}

fn require_public_ip(ip: IpAddr) -> Result<(), CallbackPolicyError> {
    if is_public_ip(ip) {
        Ok(())
    } else {
        Err(CallbackPolicyError::NonPublicAddress)
    }
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => is_public_ipv6(ip),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _d] = ip.octets();
    !(
        // Current network, RFC1918, shared carrier space, loopback.
        a == 0
            || a == 10
            || a == 127
            || (a == 100 && (64..=127).contains(&b))
            || (a == 172 && (16..=31).contains(&b))
            || (a == 192 && b == 168)
            // Link-local and IANA protocol/documentation assignments.
            || (a == 169 && b == 254)
            || (a == 192 && b == 0 && c == 0)
            || (a == 192 && b == 0 && c == 2)
            || (a == 192 && b == 88 && c == 99)
            || (a == 198 && (b == 18 || b == 19))
            || (a == 198 && b == 51 && c == 100)
            || (a == 203 && b == 0 && c == 113)
            // Multicast, reserved, and limited broadcast.
            || a >= 224
    )
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    // This also catches IPv4-compatible and IPv4-mapped forms.
    if let Some(v4) = ip.to_ipv4() {
        return is_public_ipv4(v4);
    }

    let segments = ip.segments();
    let first = segments[0];
    let second = segments[1];

    // Public IPv6 unicast is allocated from 2000::/3.  Exclude special-use
    // subranges inside it, plus local/multicast space outside it.
    if first & 0xe000 != 0x2000 {
        return false;
    }
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return false;
    }
    // 2001::/23 (Teredo, benchmarking, ORCHID and protocol assignments).
    if first == 0x2001 && second <= 0x01ff {
        return false;
    }
    // 2001:db8::/32 documentation prefix.
    if first == 0x2001 && second == 0x0db8 {
        return false;
    }
    // 6to4 embeds an IPv4 destination in 2002::/16. Deny the transition
    // prefix outright so private or special IPv4 routes cannot be tunneled.
    if first == 0x2002 {
        return false;
    }
    // 64:ff9b::/96 and 64:ff9b:1::/48 are outside 2000::/3 and are already
    // denied, preventing NAT64 encodings of private IPv4 destinations.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_malformed_insecure_credentialed_fragmented_and_port_urls() {
        for raw in [
            "not a url",
            "http://example.com/hook",
            "https://user:secret@example.com/hook",
            "https://example.com/hook#fragment",
            "https://example.com:8443/hook",
        ] {
            assert!(validate_callback_url(raw).is_err(), "accepted {raw}");
        }
    }

    #[test]
    fn rejects_encoded_ipv4_and_ipv4_mapped_ipv6() {
        for raw in [
            "https://127.0.0.1/hook",
            "https://2130706433/hook",
            "https://0177.0.0.1/hook",
            "https://0x7f000001/hook",
            "https://[::1]/hook",
            "https://[::ffff:127.0.0.1]/hook",
            "https://169.254.169.254/latest/meta-data/",
            "https://[fd00:ec2::254]/latest/meta-data/",
        ] {
            assert!(validate_callback_url(raw).is_err(), "accepted {raw}");
        }
    }

    #[test]
    fn accepts_normal_public_https_callbacks() {
        let named = validate_callback_url("https://Webhooks.Example.COM/a2a").unwrap();
        assert_eq!(named.canonical_url(), "https://webhooks.example.com/a2a");
        let literal = validate_callback_url("https://93.184.216.34/hook").unwrap();
        assert_eq!(literal.canonical_url(), "https://93.184.216.34/hook");
    }

    #[test]
    fn rejects_private_and_mixed_dns_results() {
        let public: SocketAddr = "93.184.216.34:443".parse().unwrap();
        let private: SocketAddr = "10.0.0.8:443".parse().unwrap();
        assert_eq!(select_public_destination(vec![public], 0).unwrap(), public);
        assert_eq!(
            select_public_destination(vec![public, private], 0),
            Err(CallbackPolicyError::NonPublicAddress)
        );
        // A rebind from public to private is denied on the next resolution.
        assert_eq!(
            select_public_destination(vec![private], 1),
            Err(CallbackPolicyError::NonPublicAddress)
        );
    }

    #[test]
    fn rejects_special_ipv4_and_ipv6_ranges() {
        for address in [
            "0.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "172.16.0.1",
            "192.168.0.1",
            "198.18.0.1",
            "192.0.2.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "2001:db8::1",
            "2001::1",
            "2002:0a00:0001::1",
            "fc00::1",
            "fe80::1",
            "ff02::1",
            "64:ff9b::a9fe:a9fe",
        ] {
            let ip: IpAddr = address.parse().unwrap();
            assert!(!is_public_ip(ip), "accepted {address}");
        }
    }
}
