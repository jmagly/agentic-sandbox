//! `agentic-sandbox/runtime` extension. Filled in by #213.
//!
//! Surfaces VM/container runtime metadata (CPU, memory, hypervisor) on the
//! AgentCard and adds runtime-scoped fields to task results.

/// Runtime-extension marker. Real configuration lands in #213.
pub struct RuntimeExtension;
