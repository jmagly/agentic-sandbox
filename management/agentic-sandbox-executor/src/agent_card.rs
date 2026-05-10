//! AgentCard generation, JCS canonicalization (RFC 8785), and JWS signing
//! (RFC 7515).
//!
//! Each instance publishes a signed AgentCard at
//! `/.well-known/agent-card.json` describing its capabilities, supported
//! extensions, bindings (REST, SSE, WebSocket), and a JWS signature over
//! the JCS-canonicalized form of the card (signatures field excluded).
//!
//! # Pipeline
//!
//! 1. [`build_agent_card`] assembles a JSON value from [`AgentCardInputs`]
//!    plus the five agentic-sandbox extensions.
//! 2. [`sign_agent_card`] strips any existing `signatures` field, runs JCS
//!    canonicalization (RFC 8785) over the result, signs with EdDSA
//!    (Ed25519) using the supplied [`SigningKey`], and re-attaches the JWS
//!    compact serialization under `signatures[0].signature`.
//! 3. [`verify_agent_card`] reverses the process: extracts signatures,
//!    re-canonicalizes the unsigned card, and verifies via the matching
//!    JWK in the supplied JWKS.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use josekit::jwk::Jwk;
use josekit::jws::alg::eddsa::EddsaJwsAlgorithm;
use josekit::jws::{self, JwsHeader};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// --- Public types -----------------------------------------------------------

/// Runtime substrate for an agent instance, surfaced via `runtime/v1`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeKind {
    Vm,
    Container,
}

impl RuntimeKind {
    fn as_str(self) -> &'static str {
        match self {
            RuntimeKind::Vm => "vm",
            RuntimeKind::Container => "container",
        }
    }
}

/// Inputs required to assemble an AgentCard for a single instance.
pub struct AgentCardInputs<'a> {
    pub instance_id: &'a str,
    pub host: &'a str,
    pub runtime_kind: RuntimeKind,
    pub loadout: &'a str,
    pub image_ref: Option<&'a str>,
    pub security_schemes: &'a Value,
    pub skills: &'a [Value],
}

/// An Ed25519 signing key bound to a stable `kid`.
///
/// Holds the private JWK in memory; callers should keep instances of this
/// type away from logs and serialized state.
pub struct SigningKey {
    kid: String,
    alg: String,
    private_jwk: Jwk,
    public_jwk_value: Value,
}

impl SigningKey {
    /// Generate a fresh Ed25519 keypair and wrap it as a `SigningKey` with
    /// the given `kid` and `alg = "EdDSA"`.
    pub fn generate_ed25519(kid: String) -> Result<Self> {
        let alg = EddsaJwsAlgorithm::Eddsa;
        let key_pair = alg
            .generate_key_pair(josekit::jwk::alg::ed::EdCurve::Ed25519)
            .map_err(|e| anyhow!("ed25519 key generation failed: {e}"))?;
        // Use the full keypair JWK so both `d` (private) and `x` (public)
        // are present. josekit's `signer_from_jwk` and `verifier_from_jwk`
        // both consult `x`.
        let mut private_jwk = key_pair.to_jwk_key_pair();
        private_jwk.set_key_id(&kid);
        private_jwk.set_algorithm("EdDSA");

        let mut public_only = key_pair.to_jwk_public_key();
        public_only.set_key_id(&kid);
        public_only.set_algorithm("EdDSA");
        let public_jwk_value = serde_json::to_value(public_only.as_ref())
            .context("serialize public jwk")?;

        Ok(Self {
            kid,
            alg: "EdDSA".to_string(),
            private_jwk,
            public_jwk_value,
        })
    }

    /// Stable `kid` for this key.
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// JWS algorithm identifier (`"EdDSA"` for Ed25519).
    pub fn alg(&self) -> &str {
        &self.alg
    }

    /// Public-only JWK (suitable for distribution in a JWKS).
    pub fn public_jwk(&self) -> Result<Value> {
        Ok(self.public_jwk_value.clone())
    }
}

/// A signed AgentCard plus the canonical bytes that were signed.
pub struct SignedAgentCard {
    /// The card JSON, including a populated `signatures` array.
    pub card: Value,
    /// JCS-canonicalized bytes of the card with `signatures` field removed.
    pub canonical_bytes: Vec<u8>,
    /// Wall-clock instant the signature was produced.
    pub signed_at: DateTime<Utc>,
}

impl Clone for SignedAgentCard {
    fn clone(&self) -> Self {
        Self {
            card: self.card.clone(),
            canonical_bytes: self.canonical_bytes.clone(),
            signed_at: self.signed_at,
        }
    }
}

// --- Build pipeline ---------------------------------------------------------

const EXT_RUNTIME: &str = "https://agentic-sandbox.aiwg.io/extensions/runtime/v1";
const EXT_IDEMPOTENCY: &str = "https://agentic-sandbox.aiwg.io/extensions/idempotency/v1";
const EXT_HITL: &str = "https://agentic-sandbox.aiwg.io/extensions/hitl-prompt/v1";
const EXT_MULTI_TENANT: &str = "https://agentic-sandbox.aiwg.io/extensions/multi-tenant/v1";
const EXT_PTY: &str = "https://agentic-sandbox.aiwg.io/extensions/pty-extensions/v1";

/// Build an AgentCard JSON value (without signatures) from `inputs`.
///
/// The card includes the five agentic-sandbox extensions (`runtime/v1`,
/// `idempotency/v1`, `hitl-prompt/v1`, `multi-tenant/v1`,
/// `pty-extensions/v1`) and `supportedInterfaces` for REST + PTY/WS.
pub fn build_agent_card(inputs: &AgentCardInputs) -> Value {
    let runtime_params = if let Some(image) = inputs.image_ref {
        json!({
            "kind": inputs.runtime_kind.as_str(),
            "loadout": inputs.loadout,
            "imageRef": image,
            "instanceId": inputs.instance_id,
        })
    } else {
        json!({
            "kind": inputs.runtime_kind.as_str(),
            "loadout": inputs.loadout,
            "instanceId": inputs.instance_id,
        })
    };

    let extensions = vec![
        json!({
            "uri": EXT_RUNTIME,
            "required": true,
            "params": runtime_params,
        }),
        json!({
            "uri": EXT_IDEMPOTENCY,
            "required": true,
        }),
        json!({
            "uri": EXT_HITL,
            "required": false,
        }),
        json!({
            "uri": EXT_MULTI_TENANT,
            "required": false,
        }),
        json!({
            "uri": EXT_PTY,
            "required": false,
        }),
    ];

    let base_url = format!("https://{}", inputs.host);
    let pty_url = format!("wss://{}/pty", inputs.host);

    json!({
        "protocolVersion": "0.3.0",
        "name": inputs.instance_id,
        "description": format!(
            "agentic-sandbox executor instance {} ({})",
            inputs.instance_id, inputs.runtime_kind.as_str()
        ),
        "url": base_url,
        "preferredTransport": "JSONRPC",
        "version": "2.0.0",
        "capabilities": {
            "streaming": true,
            "pushNotifications": true,
            "extensions": extensions,
        },
        "defaultInputModes": ["text/plain", "application/json"],
        "defaultOutputModes": ["text/plain", "application/json"],
        "skills": inputs.skills.to_vec(),
        "securitySchemes": inputs.security_schemes.clone(),
        "supportedInterfaces": [
            {
                "url": base_url,
                "transport": "JSONRPC",
            },
            {
                "url": format!("{}/v1", base_url),
                "transport": "HTTP+JSON",
            },
            {
                "url": pty_url,
                "transport": "WebSocket",
                "extension": EXT_PTY,
            }
        ],
    })
}

// --- Sign / verify ----------------------------------------------------------

/// Canonicalize `card` (with the `signatures` field removed) per RFC 8785.
fn canonicalize_unsigned(card: &Value) -> Result<(Value, Vec<u8>)> {
    let mut unsigned = card.clone();
    if let Some(obj) = unsigned.as_object_mut() {
        obj.remove("signatures");
    }
    let canonical = serde_jcs::to_vec(&unsigned).context("JCS canonicalize agent card")?;
    Ok((unsigned, canonical))
}

/// Sign `card` with `key`, attaching the JWS compact signature under
/// `signatures[0]`.
pub fn sign_agent_card(card: Value, key: &SigningKey) -> Result<SignedAgentCard> {
    let (_unsigned, canonical_bytes) = canonicalize_unsigned(&card)?;

    let alg = EddsaJwsAlgorithm::Eddsa;
    let signer = alg
        .signer_from_jwk(&key.private_jwk)
        .map_err(|e| anyhow!("build EdDSA signer: {e}"))?;
    let mut header = JwsHeader::new();
    header.set_algorithm(&key.alg);
    header.set_key_id(&key.kid);

    let compact = jws::serialize_compact(&canonical_bytes, &header, &signer)
        .map_err(|e| anyhow!("JWS sign: {e}"))?;

    let signature_entry = json!({
        "header": {
            "alg": key.alg,
            "kid": key.kid,
        },
        "signature": compact,
    });

    let mut signed_card = card;
    if !signed_card.is_object() {
        return Err(anyhow!("agent card must be a JSON object"));
    }
    signed_card
        .as_object_mut()
        .unwrap()
        .insert("signatures".to_string(), Value::Array(vec![signature_entry]));

    Ok(SignedAgentCard {
        card: signed_card,
        canonical_bytes,
        signed_at: Utc::now(),
    })
}

/// Verify the JWS signature on `card` using a key from `jwks`.
///
/// `jwks` must be `{ "keys": [ <jwk>, ... ] }`. The card's first signature
/// header `kid` selects the verifying key.
pub fn verify_agent_card(card: &Value, jwks: &Value) -> Result<()> {
    let signatures = card
        .get("signatures")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("agent card has no signatures"))?;
    let first = signatures
        .first()
        .ok_or_else(|| anyhow!("signatures array is empty"))?;
    let compact = first
        .get("signature")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("signature entry missing 'signature' field"))?;
    let kid = first
        .get("header")
        .and_then(|h| h.get("kid"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("signature entry missing 'header.kid'"))?;

    let keys = jwks
        .get("keys")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("jwks missing 'keys' array"))?;
    let matching = keys
        .iter()
        .find(|k| k.get("kid").and_then(|v| v.as_str()) == Some(kid))
        .ok_or_else(|| anyhow!("no JWK with kid={}", kid))?;

    let jwk_bytes = serde_json::to_vec(matching).context("serialize matching jwk")?;
    let jwk = Jwk::from_bytes(&jwk_bytes).map_err(|e| anyhow!("parse jwk: {e}"))?;
    let alg = EddsaJwsAlgorithm::Eddsa;
    let verifier = alg
        .verifier_from_jwk(&jwk)
        .map_err(|e| anyhow!("build EdDSA verifier: {e}"))?;

    // Recompute canonical bytes the way the signer did.
    let (_unsigned, expected_canonical) = canonicalize_unsigned(card)?;

    let (payload, _header) = jws::deserialize_compact(compact, &verifier)
        .map_err(|e| anyhow!("JWS verify: {e}"))?;

    if payload != expected_canonical {
        return Err(anyhow!(
            "JWS payload does not match canonicalized agent card"
        ));
    }
    Ok(())
}

// --- Backwards-compat placeholder type -------------------------------------

/// Legacy stub kept so the bootstrap smoke test (#208) still compiles.
/// New code should use [`build_agent_card`] + [`sign_agent_card`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCard {
    pub version: String,
    pub name: String,
}

impl AgentCard {
    pub fn stub(name: impl Into<String>) -> Self {
        Self {
            version: "0.0.0-skeleton".to_string(),
            name: name.into(),
        }
    }
}

// --- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_inputs() -> (Value, Vec<Value>) {
        let security = json!({
            "bearer": {
                "type": "http",
                "scheme": "bearer",
                "bearerFormat": "JWT",
            }
        });
        let skills = vec![json!({
            "id": "echo",
            "name": "Echo",
            "description": "Echoes input back as output",
            "tags": ["demo"],
        })];
        (security, skills)
    }

    fn build_sample_card() -> Value {
        let (security, skills) = sample_inputs();
        let inputs = AgentCardInputs {
            instance_id: "agent-01",
            host: "agent-01.example.test",
            runtime_kind: RuntimeKind::Vm,
            loadout: "agentic-dev",
            image_ref: Some("agentic-sandbox:2026.05"),
            security_schemes: &security,
            skills: &skills,
        };
        build_agent_card(&inputs)
    }

    #[test]
    fn build_agent_card_shape() {
        let card = build_sample_card();
        assert_eq!(card["protocolVersion"], "0.3.0");
        assert_eq!(card["name"], "agent-01");
        assert!(card["description"].as_str().unwrap().contains("agent-01"));
        assert_eq!(card["preferredTransport"], "JSONRPC");
        assert!(card["url"].as_str().unwrap().starts_with("https://"));
        assert!(card["capabilities"]["streaming"].as_bool().unwrap());
        assert!(card["capabilities"]["pushNotifications"].as_bool().unwrap());
        assert!(card["skills"].is_array());
        assert!(card["securitySchemes"].is_object());
        assert!(card["supportedInterfaces"].is_array());
        assert!(card["defaultInputModes"].is_array());
        assert!(card["defaultOutputModes"].is_array());
    }

    #[test]
    fn extensions_contain_all_five() {
        let card = build_sample_card();
        let exts = card["capabilities"]["extensions"].as_array().unwrap();
        let uris: Vec<&str> = exts
            .iter()
            .map(|e| e["uri"].as_str().unwrap())
            .collect();
        assert!(uris.contains(&EXT_RUNTIME));
        assert!(uris.contains(&EXT_IDEMPOTENCY));
        assert!(uris.contains(&EXT_HITL));
        assert!(uris.contains(&EXT_MULTI_TENANT));
        assert!(uris.contains(&EXT_PTY));
        assert_eq!(exts.len(), 5);
    }

    #[test]
    fn required_flags_correct() {
        let card = build_sample_card();
        let exts = card["capabilities"]["extensions"].as_array().unwrap();
        for ext in exts {
            let uri = ext["uri"].as_str().unwrap();
            let required = ext["required"].as_bool().unwrap();
            match uri {
                EXT_RUNTIME | EXT_IDEMPOTENCY => assert!(required, "{uri} must be required"),
                EXT_HITL | EXT_MULTI_TENANT | EXT_PTY => {
                    assert!(!required, "{uri} must NOT be required")
                }
                other => panic!("unexpected extension uri: {other}"),
            }
        }
    }

    #[test]
    fn jcs_deterministic() {
        let a = build_sample_card();
        let b = build_sample_card();
        let (_, ba) = canonicalize_unsigned(&a).unwrap();
        let (_, bb) = canonicalize_unsigned(&b).unwrap();
        assert_eq!(ba, bb);
    }

    #[test]
    fn jws_round_trip() {
        let card = build_sample_card();
        let key = SigningKey::generate_ed25519("test-kid-1".to_string()).unwrap();
        let signed = sign_agent_card(card, &key).expect("sign");
        let pub_jwk = key.public_jwk().unwrap();
        let jwks = json!({ "keys": [pub_jwk] });
        verify_agent_card(&signed.card, &jwks).expect("verify");
    }

    #[test]
    fn tampered_signature_rejected() {
        let card = build_sample_card();
        let key = SigningKey::generate_ed25519("test-kid-2".to_string()).unwrap();
        let signed = sign_agent_card(card, &key).expect("sign");
        let pub_jwk = key.public_jwk().unwrap();
        let jwks = json!({ "keys": [pub_jwk] });

        // Tamper: flip a character in the JWS compact signature.
        let mut tampered = signed.card.clone();
        let sig_str = tampered["signatures"][0]["signature"]
            .as_str()
            .unwrap()
            .to_string();
        let mut bytes = sig_str.into_bytes();
        // Flip one byte in the middle (avoiding the dots that separate
        // header/payload/signature in compact form).
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0x01;
        let tampered_sig = String::from_utf8_lossy(&bytes).to_string();
        tampered["signatures"][0]["signature"] = Value::String(tampered_sig);

        let result = verify_agent_card(&tampered, &jwks);
        assert!(result.is_err(), "tampered signature should fail verify");
    }
}
