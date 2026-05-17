//! Outbound dispatch hook for `messages:send`.
//!
//! The executor crate persists every A2A message as a Task row in
//! `submitted` state, but it has no visibility into the backing runtime
//! (VM / container) that the message is meant to address. The
//! [`MessageDispatch`] trait is the seam: production builds inject an
//! implementation that forwards the message to the connected agent and
//! drives the task through `working → completed/failed`; tests and the
//! executor-only harness inject [`NoOpMessageDispatch`] which honestly
//! reports `unimplemented` so callers don't poll a phantom task forever.
//!
//! See `roctinam/agentic-sandbox#269` for the failure mode this hook
//! exists to fix: previously `messages:send` returned `202 Accepted`
//! with a `submitted` task that never transitioned because nothing was
//! forwarding work to the runtime.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use thiserror::Error;

use crate::instance::InstanceContext;

/// Outcome of an attempted dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// Work was accepted by the runtime. The task should transition to
    /// `working`; further progress is driven asynchronously by output
    /// observers wired by the dispatch implementation.
    Accepted,
}

/// Why a dispatch attempt failed.
///
/// The executor maps these onto HTTP status codes and RFC 7807 envelopes
/// so the operator gets a truthful response instead of a silent
/// `submitted` task. Variant choice matters: `NotImplemented` and
/// `RuntimeUnavailable` both map to 503 but carry different `code`
/// strings so dashboards can distinguish "the seam isn't wired" from
/// "the agent is offline".
#[derive(Debug, Error)]
pub enum DispatchError {
    /// No dispatch implementation is wired (test harness / executor-only
    /// build). The handler returns 503 with `code: dispatch.unimplemented`.
    #[error("message dispatch not wired for this build")]
    NotImplemented,

    /// The dispatch implementation is wired but the runtime agent for
    /// this instance is not reachable (not yet connected, just dropped).
    /// Maps to 503 + `code: runtime.unavailable`.
    #[error("runtime agent for instance {0} is not reachable: {1}")]
    RuntimeUnavailable(String, String),

    /// The dispatch implementation rejected the message (malformed,
    /// unsupported, validation failure). Maps to 502 + `code:
    /// dispatch.failed`.
    #[error("dispatch failed: {0}")]
    DispatchFailed(String),
}

/// Trait implemented by whatever knows how to forward an A2A
/// message to the backing runtime.
///
/// The executor crate ships [`NoOpMessageDispatch`] only; production
/// wiring lives in the `agentic-management` crate and uses the existing
/// `AgentRegistry` + `CommandDispatcher` plumbing.
#[async_trait]
pub trait MessageDispatch: Send + Sync + 'static {
    /// Forward `message` for `task_id` to the agent backing `instance`.
    ///
    /// The implementation is responsible for any state transitions
    /// beyond the initial `submitted → working` step (typically by
    /// wiring an output observer that updates the [`TaskStore`] on
    /// agent output / command completion).
    async fn dispatch(
        &self,
        instance: &InstanceContext,
        task_id: &str,
        message: &Value,
    ) -> Result<DispatchOutcome, DispatchError>;

    /// Whether this implementation is the no-op stub. Handlers use this
    /// to skip the post-persist transition to `working` when there's
    /// nothing actually doing work — the task stays in `submitted` and
    /// the response carries a clear "unimplemented" envelope.
    fn is_real(&self) -> bool {
        true
    }
}

/// Honest no-op implementation. Returns
/// [`DispatchError::NotImplemented`] so the handler can produce a 503
/// envelope instead of leaving a phantom `submitted` task indefinitely.
///
/// Tests that previously expected 202 Accepted from `messages:send`
/// continue to work because the executor's REST router defaults to
/// [`NoOpMessageDispatch`] only when explicitly constructed that way;
/// the production binary wires a real impl in `agentic-management`.
pub struct NoOpMessageDispatch;

#[async_trait]
impl MessageDispatch for NoOpMessageDispatch {
    async fn dispatch(
        &self,
        _instance: &InstanceContext,
        _task_id: &str,
        _message: &Value,
    ) -> Result<DispatchOutcome, DispatchError> {
        Err(DispatchError::NotImplemented)
    }

    fn is_real(&self) -> bool {
        false
    }
}

/// Convenience constructor returning the no-op as an `Arc<dyn ...>` so
/// callers can drop it straight into [`crate::bindings::rest::AppState`].
pub fn noop() -> Arc<dyn MessageDispatch> {
    Arc::new(NoOpMessageDispatch)
}

/// Test-only dispatch that always accepts. Lets tests exercise the
/// successful `submitted → working` path without standing up a real
/// agent. Production code must never use this.
pub struct AcceptingMessageDispatch;

#[async_trait]
impl MessageDispatch for AcceptingMessageDispatch {
    async fn dispatch(
        &self,
        _instance: &InstanceContext,
        _task_id: &str,
        _message: &Value,
    ) -> Result<DispatchOutcome, DispatchError> {
        Ok(DispatchOutcome::Accepted)
    }
}

/// Convenience constructor for the test-only accepting dispatch.
pub fn accepting() -> Arc<dyn MessageDispatch> {
    Arc::new(AcceptingMessageDispatch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::RuntimeKind;
    use serde_json::json;

    #[tokio::test]
    async fn noop_returns_not_implemented() {
        let dispatch = NoOpMessageDispatch;
        let ctx = InstanceContext::new_ephemeral(
            "inst-x".to_string(),
            RuntimeKind::Container,
            "default".to_string(),
            None,
            "host".to_string(),
        );
        let err = dispatch
            .dispatch(&ctx, "task-1", &json!({"role": "user"}))
            .await
            .unwrap_err();
        assert!(matches!(err, DispatchError::NotImplemented));
        assert!(!dispatch.is_real());
    }
}
