//! Bootstrap smoke test (#208), updated for #212 routing-layer API.
//!
//! Verifies the public surface compiles and key types are reachable. Real
//! behavioral tests live in their respective modules.

use agentic_sandbox_executor as executor;

#[test]
fn public_surface_exists() {
    // Type names exist at the documented paths.
    let _ = std::any::type_name::<executor::instance::InstanceContext>();
    let _ = std::any::type_name::<executor::instance::InstanceRegistry>();
    let _ = std::any::type_name::<executor::instance::InstanceLayer>();
    let _ = std::any::type_name::<executor::instance::InstanceExt>();
    let _ = std::any::type_name::<executor::server::ExecutorServer>();
    let _ = std::any::type_name::<executor::agent_card::AgentCard>();

    // Re-exports at the crate root resolve.
    let _ = std::any::type_name::<executor::InstanceContext>();
    let _ = std::any::type_name::<executor::ExecutorServer>();

    // Constructors are callable.
    let ctx = executor::InstanceContext::new_ephemeral(
        "inst-1",
        executor::instance::RuntimeKind::Vm,
        "agentic-dev",
        None,
        "127.0.0.1",
    );
    assert_eq!(ctx.instance_id, "inst-1");
    assert_eq!(ctx.runtime_kind, executor::instance::RuntimeKind::Vm);
    assert_eq!(ctx.loadout, "agentic-dev");

    let registry = executor::instance::InstanceRegistry::new();
    assert!(registry.is_empty());

    let _layer = executor::instance::InstanceLayer::new(registry);
    let _server = executor::ExecutorServer::new();
    let _card = executor::agent_card::AgentCard::stub("test-instance");

    // Storage re-exports resolve to the management crate.
    let _ = std::any::type_name::<executor::store::task_store::TaskStore>();
}
