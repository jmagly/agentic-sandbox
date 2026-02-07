//! Health check endpoints for HTTP server

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};

use crate::http::server::AppState;

/// Health check response
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub uptime_seconds: u64,
    pub agent_count: usize,
    pub active_tasks: usize,
}

/// Simple health check - always returns 200 OK if service is running
pub async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

/// Detailed health check with metrics
pub async fn health_detailed(State(state): State<AppState>) -> impl IntoResponse {
    let agent_count = state.registry.count();
    let active_tasks = 0; // Simplified for now

    let response = HealthResponse {
        status: "healthy".to_string(),
        uptime_seconds: 0, // TODO: track actual uptime
        agent_count,
        active_tasks,
    };

    (StatusCode::OK, Json(response))
}

/// Readiness check - verifies all components are ready
pub async fn readiness(State(state): State<AppState>) -> impl IntoResponse {
    // Check if we have any agents connected
    let agent_count = state.registry.count();

    if agent_count == 0 {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "not_ready",
                "reason": "no_agents_connected"
            })),
        );
    }

    // All checks passed
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "ready",
            "agent_count": agent_count
        })),
    )
}

/// Liveness check - basic check that service is alive
pub async fn liveness() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({"status": "alive"})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::CommandDispatcher;
    use crate::output::OutputAggregator;
    use crate::registry::AgentRegistry;
    use std::sync::Arc;

    fn create_test_state_without_orchestrator() -> AppState {
        let registry = Arc::new(AgentRegistry::new());
        let output_agg = Arc::new(OutputAggregator::new(1000));
        let dispatcher = Arc::new(CommandDispatcher::new(registry.clone()));

        AppState {
            registry,
            output_agg,
            dispatcher,
            orchestrator: None,
            metrics: None,
            operation_store: None,
            secret_store: None,
        }
    }

    #[tokio::test]
    async fn test_health_check() {
        let response = health_check().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_liveness() {
        let response = liveness().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_health_detailed() {
        let state = create_test_state_without_orchestrator();
        let response = health_detailed(State(state)).await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_readiness_no_agents() {
        let state = create_test_state_without_orchestrator();
        let response = readiness(State(state)).await.into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
