//! HITL (Human-in-the-Loop) request store.
//!
//! Tracks pending HITL requests generated when the PTY heuristic detects that
//! an agent is waiting for human input. Responses are injected back into the
//! agent's PTY stdin via the command dispatcher.
//!
//! When an `AiwgServeHandle` is wired in, a `hitl.input_required` event is
//! pushed to the aiwg serve dashboard the moment a new request is created.

use chrono::Utc;
use dashmap::DashMap;
use serde::Serialize;
use uuid::Uuid;

use crate::aiwg_serve::{AiwgServeHandle, SandboxEvent};

/// A pending HITL request — agent is waiting for human input.
#[derive(Debug, Clone, Serialize)]
pub struct HitlRequest {
    pub hitl_id: String,
    pub agent_id: String,
    /// The active command/session ID whose PTY stdin will receive the response.
    pub session_id: String,
    /// The prompt text detected (last visible line).
    pub prompt: String,
    /// Recent PTY output context (last N lines passed by the caller).
    pub context: String,
    /// Unix timestamp (ms) when the request was created.
    pub created_at_ms: i64,
}

/// Shared in-memory store for pending HITL requests.
pub struct HitlStore {
    pending: DashMap<String, HitlRequest>,
    /// Tracks which session_ids already have a pending request (for deduplication).
    active_sessions: DashMap<String, String>,
    /// Optional handle to push events to aiwg serve.
    aiwg: Option<AiwgServeHandle>,
    /// Optional mission store — when present, HITL requests for sessions
    /// belonging to a mission also emit `mission.hitl_required` (#193 pass 2).
    mission_store: Option<crate::aiwg_serve::MissionStore>,
}

impl HitlStore {
    pub fn new() -> Self {
        Self {
            pending: DashMap::new(),
            active_sessions: DashMap::new(),
            aiwg: None,
            mission_store: None,
        }
    }

    /// Attach an aiwg serve handle so `hitl.input_required` events are pushed
    /// to the dashboard when a new HITL request is created.
    pub fn with_aiwg_serve(mut self, handle: AiwgServeHandle) -> Self {
        self.aiwg = Some(handle);
        self
    }

    /// Attach a mission store for HITL → `mission.hitl_required` translation.
    pub fn with_mission_store(mut self, store: crate::aiwg_serve::MissionStore) -> Self {
        self.mission_store = Some(store);
        self
    }

    /// Register a new HITL request.
    ///
    /// Returns `None` if there is already a pending request for `session_id`
    /// (prevents duplicate notifications while the human is thinking). Returns
    /// `Some(hitl_id)` otherwise.
    ///
    /// When an `AiwgServeHandle` is present, emits a `hitl.input_required`
    /// event to aiwg serve immediately.
    pub fn create(
        &self,
        agent_id: String,
        session_id: String,
        prompt: String,
        context: String,
    ) -> Option<String> {
        // Deduplicate — one pending request per session at a time.
        if self.active_sessions.contains_key(&session_id) {
            return None;
        }

        let hitl_id = Uuid::new_v4().to_string();
        self.active_sessions
            .insert(session_id.clone(), hitl_id.clone());
        self.pending.insert(
            hitl_id.clone(),
            HitlRequest {
                hitl_id: hitl_id.clone(),
                agent_id: agent_id.clone(),
                session_id: session_id.clone(),
                prompt: prompt.clone(),
                context: context.clone(),
                created_at_ms: Utc::now().timestamp_millis(),
            },
        );

        // Push to aiwg serve dashboard (fire-and-forget).
        if let Some(ref h) = self.aiwg {
            h.emit(SandboxEvent::HitlInputRequired {
                agent_id: agent_id.clone(),
                session_id: session_id.clone(),
                hitl_id: hitl_id.clone(),
                prompt: prompt.clone(),
                context: context.clone(),
            });

            // Mission translation (#193 pass 2): if this HITL request belongs
            // to a mission, also emit the executor-contract event.
            if let (Some(ref store), Some(executor_id)) =
                (self.mission_store.as_ref(), h.executor_id())
            {
                if let Some(mission_id) = store.find_by_session(&session_id) {
                    h.emit_executor(crate::aiwg_serve::ExecutorEvent::mission_hitl_required(
                        &executor_id,
                        &mission_id,
                        &hitl_id,
                        &prompt,
                        &context,
                    ));
                    store.update_state(&mission_id, crate::aiwg_serve::MissionState::HitlRequired);
                }
            }
        }

        Some(hitl_id)
    }

    /// Retrieve a pending HITL request without removing it.
    pub fn get(&self, hitl_id: &str) -> Option<HitlRequest> {
        self.pending.get(hitl_id).map(|r| r.clone())
    }

    /// Remove and return a HITL request (called when a response is submitted).
    /// Clears the session deduplication slot so future prompts can be captured.
    pub fn resolve(&self, hitl_id: &str) -> Option<HitlRequest> {
        let (_, req) = self.pending.remove(hitl_id)?;
        self.active_sessions.remove(&req.session_id);
        Some(req)
    }

    /// List all pending requests.
    pub fn list(&self) -> Vec<HitlRequest> {
        let mut reqs: Vec<HitlRequest> = self.pending.iter().map(|r| r.clone()).collect();
        reqs.sort_by_key(|r| r.created_at_ms);
        reqs
    }
}
