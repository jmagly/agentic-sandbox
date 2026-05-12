//! `agentcard` — fetch and verify the executor's signed AgentCard.
//!
//! Endpoints:
//! - `GET /agents/{instance_id}/.well-known/agent-card.json`  ← `agentcard get`
//! - `GET /agents/{instance_id}/.well-known/jwks.json`         ← (loaded by `verify`)
//!
//! Verification reference: `agentic-sandbox-conformance/internal/spec/jws.go`.
//! Algorithm: EdDSA (Ed25519) only. JCS canonicalization per RFC 8785,
//! signatures field excluded.

use anyhow::{anyhow, Context, Result};
use base64::Engine as _;
use serde_json::Value;

use crate::client::http::HttpClient;

/// `agentcard get <instance_id>` — pretty-print the signed AgentCard.
pub async fn get(c: &HttpClient, instance_id: &str) -> Result<()> {
    let path = format!("/agents/{}/.well-known/agent-card.json", instance_id);
    let v = c.get_value(&path).await?;
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}

/// `agentcard verify <instance_id> --jwks <path-or-url>` — fetch the card,
/// load the JWKS, re-canonicalize the unsigned card, and verify the JWS
/// against the matching JWK by `kid`.
pub async fn verify(c: &HttpClient, instance_id: &str, jwks_source: &str) -> Result<()> {
    let card_path = format!("/agents/{}/.well-known/agent-card.json", instance_id);
    let card = c.get_value(&card_path).await?;

    let jwks = load_jwks(c, jwks_source).await?;

    match verify_agent_card(&card, &jwks) {
        Ok(meta) => {
            println!("✓ AgentCard signature verifies");
            println!("  kid: {}", meta.kid);
            println!("  alg: {}", meta.alg);
            println!("  canonical_bytes: {}", meta.canonical_len);
            Ok(())
        }
        Err(e) => {
            eprintln!("✗ AgentCard signature verification FAILED");
            Err(e)
        }
    }
}

/// Load a JWKS from either a local file path or an `http(s)://` URL.
/// HTTP loads reuse the same reqwest client as the rest of the CLI so
/// TLS settings and timeouts are consistent.
async fn load_jwks(c: &HttpClient, source: &str) -> Result<Value> {
    if source.starts_with("http://") || source.starts_with("https://") {
        let mut rb = c.inner_for_sse().get(source);
        if let Some(tok) = c.bearer_token() {
            rb = rb.bearer_auth(tok);
        }
        let resp = rb.send().await.context("fetch JWKS")?;
        if !resp.status().is_success() {
            return Err(anyhow!("fetch JWKS: HTTP {}", resp.status()));
        }
        let text = resp.text().await.context("read JWKS body")?;
        let v: Value = serde_json::from_str(&text).context("parse JWKS JSON")?;
        Ok(v)
    } else {
        let s = std::fs::read_to_string(source)
            .with_context(|| format!("read JWKS file {}", source))?;
        let v: Value = serde_json::from_str(&s).context("parse JWKS JSON")?;
        Ok(v)
    }
}

/// Result of a successful verification — surfaced to the operator.
#[derive(Debug)]
pub struct VerifyMeta {
    pub kid: String,
    pub alg: String,
    pub canonical_len: usize,
}

/// Verify the JWS Compact signature on an AgentCard.
///
/// Minimal verifier covering EdDSA (Ed25519) over an OKP JWK only. The
/// signed payload MUST equal the JCS-canonicalized form of the card with
/// the `signatures` field stripped (RFC 8785). This mirrors the reference
/// Go implementation in `conformance/internal/spec/jws.go`.
pub fn verify_agent_card(card: &Value, jwks: &Value) -> Result<VerifyMeta> {
    let signatures = card
        .get("signatures")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("agent card has no 'signatures' field"))?;
    let first = signatures
        .first()
        .ok_or_else(|| anyhow!("'signatures' array is empty"))?;
    let compact = first
        .get("signature")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("signatures[0].signature missing or not a string"))?;
    let kid = first
        .get("header")
        .and_then(|h| h.get("kid"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("signatures[0].header.kid missing"))?;

    let keys = jwks
        .get("keys")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("jwks missing 'keys' array"))?;
    let jwk = keys
        .iter()
        .find(|k| k.get("kid").and_then(|v| v.as_str()) == Some(kid))
        .ok_or_else(|| anyhow!("no JWK with kid={}", kid))?;

    // Re-canonicalize the card sans `signatures`.
    let canonical = canonicalize_unsigned(card)?;

    // Parse the JWS Compact form. Three segments.
    let parts: Vec<&str> = compact.split('.').collect();
    if parts.len() != 3 {
        return Err(anyhow!(
            "JWS Compact: expected 3 segments, got {}",
            parts.len()
        ));
    }
    let header_b64 = parts[0];
    let payload_b64 = parts[1];
    let sig_b64 = parts[2];

    let header_raw = base64_url_decode(header_b64).context("decode JWS header")?;
    let header_v: Value = serde_json::from_slice(&header_raw).context("parse JWS header JSON")?;
    let alg = header_v
        .get("alg")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("JWS header missing 'alg'"))?;
    if alg != "EdDSA" {
        return Err(anyhow!(
            "unsupported JWS alg `{}` (only EdDSA supported)",
            alg
        ));
    }
    let kty = jwk.get("kty").and_then(|x| x.as_str()).unwrap_or("");
    let crv = jwk.get("crv").and_then(|x| x.as_str()).unwrap_or("");
    if kty != "OKP" || crv != "Ed25519" {
        return Err(anyhow!(
            "EdDSA: JWK is not OKP/Ed25519 (kty={} crv={})",
            kty,
            crv
        ));
    }

    let payload = base64_url_decode(payload_b64).context("decode JWS payload")?;
    if payload != canonical {
        return Err(anyhow!(
            "JWS payload does not match canonicalized agent card (got {} bytes, want {})",
            payload.len(),
            canonical.len()
        ));
    }

    let sig = base64_url_decode(sig_b64).context("decode JWS signature")?;
    let pub_b64 = jwk
        .get("x")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("JWK missing 'x'"))?;
    let pub_bytes = base64_url_decode(pub_b64).context("decode JWK x")?;
    if pub_bytes.len() != 32 {
        return Err(anyhow!(
            "Ed25519 public key wrong length: {} (want 32)",
            pub_bytes.len()
        ));
    }
    let pub_arr: [u8; 32] = pub_bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("Ed25519 public key not 32 bytes"))?;
    let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&pub_arr)
        .map_err(|e| anyhow!("invalid Ed25519 public key: {}", e))?;
    if sig.len() != 64 {
        return Err(anyhow!(
            "Ed25519 signature wrong length: {} (want 64)",
            sig.len()
        ));
    }
    let sig_arr: [u8; 64] = sig
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("Ed25519 signature not 64 bytes"))?;
    let signature = ed25519_dalek::Signature::from_bytes(&sig_arr);

    // The signing input is the ASCII bytes of `<header_b64>.<payload_b64>`.
    let signing_input = format!("{}.{}", header_b64, payload_b64);

    use ed25519_dalek::Verifier;
    verifying_key
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|e| anyhow!("Ed25519 signature verification failed: {}", e))?;

    Ok(VerifyMeta {
        kid: kid.to_string(),
        alg: alg.to_string(),
        canonical_len: canonical.len(),
    })
}

/// JCS-canonicalize the card with the `signatures` field stripped.
fn canonicalize_unsigned(card: &Value) -> Result<Vec<u8>> {
    let mut unsigned = card.clone();
    if let Some(obj) = unsigned.as_object_mut() {
        obj.remove("signatures");
    }
    serde_jcs::to_vec(&unsigned).context("JCS canonicalize agent card")
}

/// RFC 4648 base64url decoder (no padding tolerated, but we strip any
/// trailing `=` first for forgiving inputs).
fn base64_url_decode(s: &str) -> Result<Vec<u8>> {
    let trimmed = s.trim_end_matches('=');
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(trimmed)
        .map_err(|e| anyhow!("base64url decode: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_client(url: &str) -> HttpClient {
        use crate::config::ContextEntry;
        HttpClient::new(&ContextEntry {
            server: url.to_string(),
            token: "".into(),
            role: "operator".into(),
        })
        .unwrap()
    }

    /// Build an unsigned card, sign it with a freshly-generated keypair,
    /// and return (signed card, jwks) suitable for verify().
    fn make_signed_card_and_jwks(kid: &str) -> (Value, Value, SigningKey) {
        let mut csprng = rand::rngs::OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying = signing_key.verifying_key();

        let card_unsigned = json!({
            "name": "test-agent",
            "version": "1.0",
            "url": "https://example.test/agents/inst-1",
            "preferredTransport": "JSONRPC",
        });

        let canonical = serde_jcs::to_vec(&card_unsigned).unwrap();

        // Build JWS Compact (non-detached).
        let header = json!({"alg": "EdDSA", "kid": kid, "typ": "JWT"});
        let header_b64 = B64URL.encode(serde_jcs::to_vec(&header).unwrap());
        let payload_b64 = B64URL.encode(&canonical);
        let signing_input = format!("{}.{}", header_b64, payload_b64);
        let sig = signing_key.sign(signing_input.as_bytes());
        let sig_b64 = B64URL.encode(sig.to_bytes());
        let compact = format!("{}.{}.{}", header_b64, payload_b64, sig_b64);

        // Attach signatures[0] to the card.
        let mut signed = card_unsigned.clone();
        signed.as_object_mut().unwrap().insert(
            "signatures".to_string(),
            json!([{
                "header": {"alg": "EdDSA", "kid": kid},
                "signature": compact,
            }]),
        );

        // Build JWKS.
        let x_b64 = B64URL.encode(verifying.to_bytes());
        let jwks = json!({
            "keys": [{
                "kty": "OKP",
                "crv": "Ed25519",
                "x": x_b64,
                "kid": kid,
                "alg": "EdDSA",
                "use": "sig",
            }]
        });
        (signed, jwks, signing_key)
    }

    #[tokio::test]
    async fn agentcard_get_pretty_prints() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/agents/inst-1/.well-known/agent-card.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "test",
                "signatures": []
            })))
            .mount(&server)
            .await;
        let c = test_client(&server.uri());
        assert!(get(&c, "inst-1").await.is_ok());
    }

    #[test]
    fn agentcard_verify_passes_with_valid_jwks() {
        let (card, jwks, _) = make_signed_card_and_jwks("kid-1");
        let meta = verify_agent_card(&card, &jwks).expect("verify");
        assert_eq!(meta.kid, "kid-1");
        assert_eq!(meta.alg, "EdDSA");
        assert!(meta.canonical_len > 0);
    }

    #[test]
    fn agentcard_verify_fails_with_tampered_signature() {
        let (mut card, jwks, _) = make_signed_card_and_jwks("kid-1");
        // Flip a bit in the signature.
        let compact = card["signatures"][0]["signature"]
            .as_str()
            .unwrap()
            .to_string();
        let mut bytes = compact.into_bytes();
        // Mutate a byte in the third segment (the signature).
        if let Some(last_dot) = bytes.iter().rposition(|&b| b == b'.') {
            if let Some(b) = bytes.get_mut(last_dot + 1) {
                *b = if *b == b'A' { b'B' } else { b'A' };
            }
        }
        let tampered = String::from_utf8(bytes).unwrap();
        card["signatures"][0]["signature"] = Value::String(tampered);
        let r = verify_agent_card(&card, &jwks);
        assert!(r.is_err());
    }

    #[test]
    fn agentcard_verify_fails_with_tampered_payload() {
        let (mut card, jwks, _) = make_signed_card_and_jwks("kid-1");
        // Tamper a card field.
        card["name"] = Value::String("not-the-original-name".into());
        let r = verify_agent_card(&card, &jwks);
        assert!(r.is_err(), "expected verify to fail after payload tamper");
    }

    #[test]
    fn agentcard_verify_fails_with_missing_kid() {
        let (card, _, _) = make_signed_card_and_jwks("kid-1");
        let bad_jwks =
            json!({"keys": [{"kty": "OKP", "crv": "Ed25519", "x": "AAAA", "kid": "other-kid"}]});
        let r = verify_agent_card(&card, &bad_jwks);
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("kid"));
    }
}
