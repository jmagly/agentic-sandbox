//! A2A `agentCard:getExtended` handler (#210).
//!
//! `GET /agents/{instance_id}/v1/extendedAgentCard`
//!
//! Returns the JCS-canonicalized, JWS-signed AgentCard produced by
//! [`crate::instance::InstanceContext::signed_card`]. Inputs are derived
//! from the resolved [`InstanceContext`] (#212).

use axum::body::Body;
use axum::extract::Path;
use axum::http::header::{HeaderValue, CACHE_CONTROL, CONTENT_TYPE, ETAG, LAST_MODIFIED};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::agent_card::{
    build_v1_agent_card, sign_agent_card, AgentCardInputs, RuntimeKind as CardRuntime,
};
use crate::bindings::rest::error_response;
use crate::instance::{InstanceExt, RuntimeKind};

/// Axum handler for `GET /agents/{instance_id}/v1/extendedAgentCard`.
pub async fn handler(
    Path((instance_id,)): Path<(String,)>,
    InstanceExt(ctx): InstanceExt,
    headers: HeaderMap,
) -> Response {
    let card_runtime = match ctx.runtime_kind {
        RuntimeKind::Vm => CardRuntime::Vm,
        RuntimeKind::Container => CardRuntime::Container,
        RuntimeKind::Host => CardRuntime::Host,
    };

    let security_schemes = json!({
        "bearer": {
            "type": "http",
            "scheme": "bearer",
            "bearerFormat": "JWT",
        }
    });

    let skills = vec![json!({
        "id": "agentic-sandbox-execution",
        "name": "Sandbox task execution",
        "description": "Execute agent tasks in the instance's isolated runtime.",
        "tags": ["sandbox", "agent", "execution"],
        "inputModes": ["text/plain", "application/json"],
        "outputModes": ["text/plain", "application/json"],
    })];

    let inputs = AgentCardInputs {
        instance_id: &ctx.instance_id,
        host: &ctx.host,
        runtime_kind: card_runtime,
        loadout: &ctx.loadout,
        image_ref: ctx.image_ref.as_deref(),
        runtime_provider: ctx.runtime_provider.as_deref(),
        runtime_capabilities: &ctx.runtime_capabilities,
        adapter_command_supported: ctx.adapter_command_supported(),
        security_schemes: &security_schemes,
        skills: &skills,
    };

    let negotiated_v1 = headers
        .get(crate::protocol::A2A_VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
        == Some("1.0");
    let signed = if negotiated_v1 {
        sign_agent_card(build_v1_agent_card(&inputs), &ctx.signing_key)
    } else {
        ctx.signed_card(&inputs)
    };

    match signed {
        Ok(signed) => {
            let etag = format!(
                "\"{}\"",
                hex::encode(Sha256::digest(&signed.canonical_bytes))
            );
            let last_modified = signed
                .signed_at
                .format("%a, %d %b %Y %H:%M:%S GMT")
                .to_string();
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
                .header(
                    CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=300"),
                )
                .header(ETAG, etag)
                .header(LAST_MODIFIED, last_modified)
                .body(Body::from(signed.card.to_string()))
                .unwrap()
                .into_response()
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "https://agentic-sandbox.aiwg.io/errors/internal",
            "Internal server error",
            format!("Failed to sign agent card: {e}"),
            "internal.error",
            None,
            Some(&instance_id),
        ),
    }
}
