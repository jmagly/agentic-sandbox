//! Bootstrap smoke test (#208).
//!
//! Verifies the public surface compiles and key types are reachable. Real
//! behavioral tests land alongside the issues that fill in each module.

use agentic_sandbox_executor as executor;

#[test]
fn public_surface_exists() {
    // Type names exist at the documented paths.
    let _ = std::any::type_name::<executor::instance::InstanceContext>();
    let _ = std::any::type_name::<executor::instance::InstanceRegistry>();
    let _ = std::any::type_name::<executor::server::ExecutorServer>();
    let _ = std::any::type_name::<executor::agent_card::AgentCard>();

    // Re-exports at the crate root resolve.
    let _ = std::any::type_name::<executor::InstanceContext>();
    let _ = std::any::type_name::<executor::ExecutorServer>();

    // Constructors are callable.
    let ctx = executor::InstanceContext::new("inst-1", "test-instance");
    assert_eq!(ctx.id, "inst-1");
    assert_eq!(ctx.name, "test-instance");

    let _registry = executor::instance::InstanceRegistry::new();
    let _server = executor::ExecutorServer::new();
    let _card = executor::agent_card::AgentCard::stub("test-instance");

    // Storage re-exports resolve to the management crate.
    let _ = std::any::type_name::<executor::store::task_store::TaskStore>();
}
