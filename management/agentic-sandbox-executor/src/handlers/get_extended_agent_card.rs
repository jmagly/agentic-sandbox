//! A2A `agentCard:getExtended` handler (#210).
//!
//! `GET /agents/{instance_id}/v1/extendedAgentCard`
//!
//! Returns the JCS-canonicalized, JWS-signed AgentCard produced by
//! [`crate::instance::InstanceContext::signed_card`]. Inputs are derived
//! from the resolved [`InstanceContext`] (#212).

use axum::body::Body;
use axum::extract::Path;
use axum::http::header::{HeaderValue, CONTENT_TYPE};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::agent_card::{AgentCardInputs, RuntimeKind as CardRuntime};
use crate::bindings::rest::error_response;
use crate::instance::{InstanceExt, RuntimeKind};

/// Axum handler for `GET /agents/{instance_id}/v1/extendedAgentCard`.
pub async fn handler(
    Path((instance_id,)): Path<(String,)>,
    InstanceExt(ctx): InstanceExt,
) -> Response {
    let card_runtime = match ctx.runtime_kind {
        RuntimeKind::Vm => CardRuntime::Vm,
        RuntimeKind::Container => CardRuntime::Container,
    };

    let security_schemes = json!({
        "bearer": {
            "type": "http",
            "scheme": "bearer",
            "bearerFormat": "JWT",
        }
    });

    let skills: Vec<serde_json::Value> = Vec::new();

    let inputs = AgentCardInputs {
        instance_id: &ctx.instance_id,
        host: &ctx.host,
        runtime_kind: card_runtime,
        loadout: &ctx.loadout,
        image_ref: ctx.image_ref.as_deref(),
        security_schemes: &security_schemes,
        skills: &skills,
    };

    match ctx.signed_card(&inputs) {
        Ok(signed) => Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .body(Body::from(signed.card.to_string()))
            .unwrap()
            .into_response(),
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
