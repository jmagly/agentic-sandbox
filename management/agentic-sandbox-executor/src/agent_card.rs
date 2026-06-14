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
//!    plus the active agentic-sandbox extensions.
//! 2. [`sign_agent_card`] strips any existing `signatures` field, runs JCS
//!    canonicalization (RFC 8785) over the result, signs with EdDSA
//!    (Ed25519) using the supplied [`SigningKey`], and re-attaches the JWS
//!    compact serialization under `signatures[0].signature`.
//! 3. [`verify_agent_card`] reverses the process: extracts signatures,
//!    re-canonicalizes the unsigned card, and verifies via the matching
//!    JWK in the supplied JWKS.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use josekit::jwk::alg::ed::EdKeyPair;
use josekit::jwk::{Jwk, KeyPair};
use josekit::jws::alg::eddsa::EddsaJwsAlgorithm;
use josekit::jws::{self, JwsHeader};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

// --- Public types -----------------------------------------------------------

/// Runtime substrate for an agent instance, surfaced via `runtime/v1`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeKind {
    Vm,
    Container,
    Host,
}

impl RuntimeKind {
    fn as_str(self) -> &'static str {
        match self {
            RuntimeKind::Vm => "vm",
            RuntimeKind::Container => "container",
            RuntimeKind::Host => "host",
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
    pub adapter_command_supported: bool,
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
        Self::from_key_pair(key_pair, kid)
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

    /// Build a `SigningKey` from a parsed `EdKeyPair`, tagging it with `kid`
    /// and `alg = "EdDSA"`.
    fn from_key_pair(key_pair: EdKeyPair, kid: String) -> Result<Self> {
        let mut private_jwk = key_pair.to_jwk_key_pair();
        private_jwk.set_key_id(&kid);
        private_jwk.set_algorithm("EdDSA");

        let mut public_only = key_pair.to_jwk_public_key();
        public_only.set_key_id(&kid);
        public_only.set_algorithm("EdDSA");
        let public_jwk_value =
            serde_json::to_value(public_only.as_ref()).context("serialize public jwk")?;

        Ok(Self {
            kid,
            alg: "EdDSA".to_string(),
            private_jwk,
            public_jwk_value,
        })
    }

    /// Load an Ed25519 keypair from `<dir>/signing.pem` if present, else
    /// generate fresh and persist to the directory.
    ///
    /// Layout written:
    /// - `<dir>/signing.pem` — PKCS#8 PEM (mode 0600)
    /// - `<dir>/signing.jwk.json` — public JWK
    ///
    /// On load: the persisted public JWK's `kid` is validated against the
    /// supplied `kid`. Mismatch returns an error (signals tampering or a
    /// renamed instance).
    pub fn load_or_generate(dir: &Path, kid: String) -> Result<Self> {
        let pem_path = dir.join("signing.pem");
        match std::fs::read(&pem_path) {
            Ok(pem_bytes) => {
                // Warn if private-key file is group/other-readable.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(meta) = std::fs::metadata(&pem_path) {
                        let mode = meta.permissions().mode() & 0o777;
                        if mode != 0o600 {
                            tracing::warn!(
                                path = %pem_path.display(),
                                mode = format!("{:o}", mode),
                                "signing.pem permissions are not 0600 — secret may be exposed"
                            );
                        }
                    }
                }

                let alg = EddsaJwsAlgorithm::Eddsa;
                let key_pair = alg
                    .key_pair_from_pem(&pem_bytes)
                    .map_err(|e| anyhow!("parse signing.pem: {e}"))?;

                // Sanity-check against the persisted JWK kid, if present.
                let jwk_path = dir.join("signing.jwk.json");
                if let Ok(jwk_bytes) = std::fs::read(&jwk_path) {
                    let persisted: Value =
                        serde_json::from_slice(&jwk_bytes).with_context(|| {
                            format!("parse persisted JWK at {}", jwk_path.display())
                        })?;
                    let persisted_kid = persisted.get("kid").and_then(|v| v.as_str()).unwrap_or("");
                    if persisted_kid != kid {
                        return Err(anyhow!(
                            "signing key kid mismatch: persisted {:?}, requested {:?}",
                            persisted_kid,
                            kid
                        ));
                    }
                }

                Self::from_key_pair(key_pair, kid)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Generate fresh and persist.
                let key = Self::generate_ed25519(kid)?;
                key.persist(dir)
                    .with_context(|| format!("persist new signing key to {}", dir.display()))?;
                Ok(key)
            }
            Err(e) => Err(anyhow!("read {}: {}", pem_path.display(), e)),
        }
    }

    /// Persist this signing key to `<dir>/signing.pem` (private, mode 0600)
    /// and `<dir>/signing.jwk.json` (public JWK). Creates `dir` if missing.
    pub fn persist(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("create signing key dir {}", dir.display()))?;

        // Re-parse the private JWK back into an EdKeyPair so we can render
        // PKCS#8 PEM. josekit's KeyPair trait exposes to_pem_private_key()
        // on EdKeyPair but not directly on Jwk.
        let key_pair = EdKeyPair::from_jwk(&self.private_jwk)
            .map_err(|e| anyhow!("rebuild key pair from jwk: {e}"))?;
        let pem = key_pair.to_pem_private_key();

        let pem_path = dir.join("signing.pem");
        std::fs::write(&pem_path, &pem).with_context(|| format!("write {}", pem_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&pem_path)
                .with_context(|| format!("stat {}", pem_path.display()))?
                .permissions();
            perms.set_mode(0o600);
            std::fs::set_permissions(&pem_path, perms)
                .with_context(|| format!("chmod 0600 {}", pem_path.display()))?;
        }

        let jwk_path = dir.join("signing.jwk.json");
        let jwk_json =
            serde_json::to_vec_pretty(&self.public_jwk_value).context("serialize public jwk")?;
        std::fs::write(&jwk_path, jwk_json)
            .with_context(|| format!("write {}", jwk_path.display()))?;

        Ok(())
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
const EXT_ADAPTER_COMMAND: &str = "https://agentic-sandbox.aiwg.io/extensions/adapter-command/v1";
const PTY_REPLAY_BUFFER_FRAMES: usize = 1000;
const PTY_REPLAY_RETENTION_SECONDS: u64 = 86_400;
const PTY_DEFAULT_COLS: u16 = 120;
const PTY_DEFAULT_ROWS: u16 = 30;

/// Build an AgentCard JSON value (without signatures) from `inputs`.
///
/// The card includes the core agentic-sandbox extensions (`runtime/v1`,
/// `idempotency/v1`, `hitl-prompt/v1`, `multi-tenant/v1`,
/// `pty-extensions/v1`), conditionally includes `adapter-command/v1`,
/// and reports `supportedInterfaces` for REST + PTY/WS.
pub fn build_agent_card(inputs: &AgentCardInputs) -> Value {
    // Field names per docs/contracts/extensions/runtime/v1/params.schema.json:
    // `runtime` (not "kind"), `image_ref` (not "imageRef"), `instance_id`
    // (not "instanceId"). Conformance test
    // `agent_card_runtime_params_have_required_fields` checks for this set.
    let runtime_params = if let Some(image) = inputs.image_ref {
        json!({
            "runtime": inputs.runtime_kind.as_str(),
            "loadout": inputs.loadout,
            "image_ref": image,
            "instance_id": inputs.instance_id,
        })
    } else {
        json!({
            "runtime": inputs.runtime_kind.as_str(),
            "loadout": inputs.loadout,
            "instance_id": inputs.instance_id,
        })
    };

    let mut extensions = vec![
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
            "description": "Interactive PTY sessions: controller/observer roles, replay buffer, Keyframe snapshots.",
            "required": false,
            "params": {
                "max_controllers": 1,
                "max_observers": 32,
                "replay_buffer_frames": PTY_REPLAY_BUFFER_FRAMES,
                "replay_buffer_retention_seconds": PTY_REPLAY_RETENTION_SECONDS,
                "default_cols": PTY_DEFAULT_COLS,
                "default_rows": PTY_DEFAULT_ROWS,
            },
        }),
    ];

    if inputs.adapter_command_supported {
        extensions.push(json!({
            "uri": EXT_ADAPTER_COMMAND,
            "required": false,
            "params": {
                "adapters": ["sandbox-agent-runner"],
                "modes": ["plan", "assess"],
            },
        }));
    }

    let base_url = format!("https://{}", inputs.host);
    let pty_url = format!(
        "wss://{}/agents/{}/sessions/{{session_id}}/attach",
        inputs.host, inputs.instance_id
    );

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
    signed_card.as_object_mut().unwrap().insert(
        "signatures".to_string(),
        Value::Array(vec![signature_entry]),
    );

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

    let (payload, _header) =
        jws::deserialize_compact(compact, &verifier).map_err(|e| anyhow!("JWS verify: {e}"))?;

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
            adapter_command_supported: true,
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
    fn extensions_contain_supported_set() {
        let card = build_sample_card();
        let exts = card["capabilities"]["extensions"].as_array().unwrap();
        let uris: Vec<&str> = exts.iter().map(|e| e["uri"].as_str().unwrap()).collect();
        assert!(uris.contains(&EXT_RUNTIME));
        assert!(uris.contains(&EXT_IDEMPOTENCY));
        assert!(uris.contains(&EXT_HITL));
        assert!(uris.contains(&EXT_MULTI_TENANT));
        assert!(uris.contains(&EXT_PTY));
        assert!(uris.contains(&EXT_ADAPTER_COMMAND));
        assert_eq!(exts.len(), 6);
    }

    #[test]
    fn pty_extension_advertises_real_attach_interface_and_limits() {
        let card = build_sample_card();
        let exts = card["capabilities"]["extensions"].as_array().unwrap();
        let pty = exts
            .iter()
            .find(|ext| ext["uri"] == EXT_PTY)
            .expect("pty extension should be present");

        assert_eq!(pty["params"]["max_controllers"], 1);
        assert_eq!(
            pty["params"]["replay_buffer_frames"],
            PTY_REPLAY_BUFFER_FRAMES
        );
        assert_eq!(
            pty["params"]["replay_buffer_retention_seconds"],
            PTY_REPLAY_RETENTION_SECONDS
        );
        assert_eq!(pty["params"]["default_cols"], PTY_DEFAULT_COLS);
        assert_eq!(pty["params"]["default_rows"], PTY_DEFAULT_ROWS);

        let interfaces = card["supportedInterfaces"].as_array().unwrap();
        let pty_interface = interfaces
            .iter()
            .find(|iface| iface["extension"] == EXT_PTY)
            .expect("pty interface should be advertised");

        assert_eq!(pty_interface["transport"], "WebSocket");
        assert_eq!(
            pty_interface["url"],
            "wss://agent-01.example.test/agents/agent-01/sessions/{session_id}/attach"
        );
    }

    #[test]
    fn adapter_command_extension_advertises_runner_modes() {
        let card = build_sample_card();
        let exts = card["capabilities"]["extensions"].as_array().unwrap();
        let adapter = exts
            .iter()
            .find(|ext| ext["uri"] == EXT_ADAPTER_COMMAND)
            .expect("adapter-command extension should be present");

        assert_eq!(
            adapter["params"]["adapters"],
            json!(["sandbox-agent-runner"])
        );
        assert_eq!(adapter["params"]["modes"], json!(["plan", "assess"]));
    }

    #[test]
    fn adapter_command_extension_can_be_suppressed() {
        let (security, skills) = sample_inputs();
        let inputs = AgentCardInputs {
            instance_id: "container-01",
            host: "container-01.example.test",
            runtime_kind: RuntimeKind::Container,
            loadout: "agentic-dev",
            image_ref: Some("agentic/codex:latest"),
            adapter_command_supported: false,
            security_schemes: &security,
            skills: &skills,
        };
        let card = build_agent_card(&inputs);
        let exts = card["capabilities"]["extensions"].as_array().unwrap();
        let uris: Vec<&str> = exts.iter().map(|e| e["uri"].as_str().unwrap()).collect();
        assert!(!uris.contains(&EXT_ADAPTER_COMMAND));
        assert_eq!(exts.len(), 5);
    }

    #[test]
    fn runtime_extension_params_support_host_without_image_ref() {
        let (security, skills) = sample_inputs();
        let inputs = AgentCardInputs {
            instance_id: "host-01",
            host: "host-01.example.test",
            runtime_kind: RuntimeKind::Host,
            loadout: "agentic-dev",
            image_ref: None,
            adapter_command_supported: true,
            security_schemes: &security,
            skills: &skills,
        };
        let card = build_agent_card(&inputs);
        let runtime = card["capabilities"]["extensions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|ext| ext["uri"] == EXT_RUNTIME)
            .expect("runtime extension");

        assert_eq!(runtime["params"]["runtime"], "host");
        assert_eq!(runtime["params"]["instance_id"], "host-01");
        assert!(runtime["params"].get("image_ref").is_none());
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
                EXT_HITL | EXT_MULTI_TENANT | EXT_PTY | EXT_ADAPTER_COMMAND => {
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

    // --- Persistence tests (#253) -------------------------------------------

    #[test]
    fn signing_key_load_or_generate_creates_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("inst-a");
        let _key =
            SigningKey::load_or_generate(&dir, "inst-a".to_string()).expect("generate fresh");

        let pem_path = dir.join("signing.pem");
        let jwk_path = dir.join("signing.jwk.json");
        assert!(pem_path.exists(), "signing.pem should be written");
        assert!(jwk_path.exists(), "signing.jwk.json should be written");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&pem_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "signing.pem must be mode 0600, got {:o}", mode);
        }

        // Public JWK should carry the kid.
        let jwk_bytes = std::fs::read(&jwk_path).unwrap();
        let jwk: Value = serde_json::from_slice(&jwk_bytes).unwrap();
        assert_eq!(jwk["kid"].as_str(), Some("inst-a"));
        assert_eq!(jwk["alg"].as_str(), Some("EdDSA"));
        // Public JWK must NOT contain the private `d` field.
        assert!(jwk.get("d").is_none(), "public JWK must not include `d`");
    }

    #[test]
    fn signing_key_load_or_generate_reuses_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("inst-reuse");
        let key1 = SigningKey::load_or_generate(&dir, "inst-reuse".to_string()).expect("generate");
        let pub1 = key1.public_jwk().unwrap();

        let key2 = SigningKey::load_or_generate(&dir, "inst-reuse".to_string()).expect("reload");
        let pub2 = key2.public_jwk().unwrap();

        assert_eq!(
            pub1["x"], pub2["x"],
            "reload should produce identical public key bytes"
        );
        assert_eq!(pub1["kid"], pub2["kid"]);
    }

    #[test]
    fn signing_key_load_or_generate_kid_mismatch_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("inst-mismatch");
        let _key1 = SigningKey::load_or_generate(&dir, "kid-a".to_string()).expect("first persist");

        let result = SigningKey::load_or_generate(&dir, "kid-b".to_string());
        let err = result.err().expect("kid mismatch must error");
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("kid mismatch"),
            "error should mention kid mismatch, got: {msg}"
        );
    }

    #[test]
    fn signing_key_persist_writes_pem_and_jwk() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("nested").join("dirs");
        let key = SigningKey::generate_ed25519("inst-persist".to_string()).unwrap();
        key.persist(&dir).expect("persist creates dir + files");

        let pem = std::fs::read(dir.join("signing.pem")).unwrap();
        let pem_str = String::from_utf8(pem).unwrap();
        assert!(
            pem_str.contains("PRIVATE KEY"),
            "PEM must be a PKCS#8 private key"
        );

        let jwk_str = std::fs::read_to_string(dir.join("signing.jwk.json")).unwrap();
        let jwk: Value = serde_json::from_str(&jwk_str).unwrap();
        assert_eq!(jwk["kty"], "OKP");
        assert_eq!(jwk["crv"], "Ed25519");
    }

    #[test]
    fn signing_key_signed_card_stable_across_load() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("inst-stable");

        let card = build_sample_card();

        let key1 = SigningKey::load_or_generate(&dir, "inst-stable".to_string()).expect("generate");
        let signed1 = sign_agent_card(card.clone(), &key1).expect("sign 1");
        let sig1 = signed1.card["signatures"][0]["signature"]
            .as_str()
            .unwrap()
            .to_string();
        drop(key1);

        let key2 = SigningKey::load_or_generate(&dir, "inst-stable".to_string()).expect("reload");
        let signed2 = sign_agent_card(card, &key2).expect("sign 2");
        let sig2 = signed2.card["signatures"][0]["signature"]
            .as_str()
            .unwrap()
            .to_string();

        // Ed25519 is deterministic — identical inputs and identical keys
        // must produce identical compact signatures.
        assert_eq!(
            sig1, sig2,
            "deterministic Ed25519 signature must match across reloads"
        );
    }
}
