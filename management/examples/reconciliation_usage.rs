//! Example: Using the reconciliation system
//!
//! Run with: cargo run --example reconciliation_usage

use agentic_management::orchestrator::{CheckpointStore, Reconciler, ReconciliationConfig};
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    println!("Reconciliation System Example\n");

    // 1. Create checkpoint store
    let checkpoint_store = Arc::new(CheckpointStore::new("/tmp/example-checkpoints"));
    checkpoint_store.initialize().await?;

    // 2. Create reconciliation config
    let config = ReconciliationConfig {
        interval: Duration::from_secs(300),
        checkpoint_retention_days: 7,
        managed_vm_prefix: "task-".to_string(),
        virsh_path: "virsh".to_string(),
        destroy_script_path: "/opt/agentic-sandbox/scripts/destroy-vm.sh".to_string(),
    };

    // 3. Create reconciler
    let reconciler = Arc::new(Reconciler::new(checkpoint_store, config));

    // 4. Run reconciliation in dry-run mode first
    println!("Running dry-run reconciliation...");
    let report = reconciler.reconcile(true).await?;

    println!("\nReconciliation Report (Dry Run):");
    println!("  Run at: {}", report.run_at);
    println!("  Total findings: {}", report.findings.len());
    println!("  - Orphaned VMs: {}", report.orphaned_vm_count());
    println!("  - Orphaned tasks: {}", report.orphaned_task_count());
    println!("  - Stale checkpoints: {}", report.stale_checkpoint_count());

    if !report.findings.is_empty() {
        println!("\nFindings:");
        for (i, finding) in report.findings.iter().enumerate() {
            println!("  {}. {:?}", i + 1, finding);
        }

        println!("\nActions that would be taken:");
        for (i, result) in report.actions_taken.iter().enumerate() {
            println!("  {}. {:?}", i + 1, result.action);
        }
    } else {
        println!("\nNo inconsistencies found - system is healthy!");
    }

    // 5. If dry-run looks good, run actual reconciliation
    if !report.findings.is_empty() {
        println!("\n\nWould you like to execute these actions? (This is just an example)");
        println!("In production, run: reconciler.reconcile(false).await?");

        // Uncomment to actually run:
        // let actual_report = reconciler.reconcile(false).await?;
        // println!("Reconciliation complete:");
        // println!("  Successful: {}", actual_report.successful_actions());
        // println!("  Failed: {}", actual_report.failed_actions());
    }

    // 6. Example: Start periodic reconciliation
    println!("\n\nTo run periodic reconciliation:");
    println!("let handle = reconciler.clone().start_periodic_reconciliation(false).await;");

    Ok(())
}
