//! Per-instance context and routing registry.
//!
//! Filled in by W3.5 (#212). Each running agent instance is represented by
//! an [`InstanceContext`] holding its identity, capabilities, AgentCard,
//! TaskStore handle, and outbound push-notification client. The
//! [`InstanceRegistry`] maps instance IDs to contexts so the HTTP layer can
//! route inbound A2A requests to the correct instance.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot_placeholder::RwLockPlaceholder;

// We avoid pulling parking_lot into the skeleton; the real registry in #212
// will pick a synchronization primitive once the access pattern is clear.
mod parking_lot_placeholder {
    /// Placeholder for the eventual concurrent map; #212 picks the real type.
    pub struct RwLockPlaceholder<T>(pub T);
}

/// Stable identifier for an executor instance (one running agent).
pub type InstanceId = String;

/// Per-instance context. Filled in by #212.
pub struct InstanceContext {
    /// Stable instance ID (usually matches the sandbox/agent ID upstream).
    pub id: InstanceId,
    /// Display name surfaced in the AgentCard.
    pub name: String,
}

impl InstanceContext {
    /// Construct a context with the given ID and name. Real construction
    /// (with TaskStore, AgentCard, push client, etc.) lands in #212.
    pub fn new(id: impl Into<InstanceId>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
        }
    }
}

/// Registry mapping instance IDs to contexts.
///
/// Filled in by #212. The skeleton uses a plain `HashMap` so the type
/// surface compiles; the production version will wrap a concurrent map and
/// expose async-friendly accessors.
pub struct InstanceRegistry {
    inner: RwLockPlaceholder<HashMap<InstanceId, Arc<InstanceContext>>>,
}

impl InstanceRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            inner: RwLockPlaceholder(HashMap::new()),
        }
    }
}

impl Default for InstanceRegistry {
    fn default() -> Self {
        Self::new()
    }
}
