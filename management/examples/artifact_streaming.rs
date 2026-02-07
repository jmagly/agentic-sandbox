//! Example: Streaming Artifact Collection via SCP
//!
//! Demonstrates how to use the StreamingArtifactCollector to transfer
//! artifacts from a VM to the host with checksum verification.
//!
//! Usage:
//!   cargo run --example artifact_streaming

use agentic_management::orchestrator::{
    ArtifactError, CollectorConfig, StreamingArtifactCollector,
};
use std::path::Path;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), ArtifactError> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Create collector with custom configuration
    let config = CollectorConfig {
        chunk_size: 512 * 1024, // 512KB chunks
        ssh_timeout: Duration::from_secs(30),
        scp_timeout: Duration::from_secs(300),
        max_retries: 3,
    };

    let collector = StreamingArtifactCollector::with_config(config);

    // Example 1: Stream a single artifact
    println!("Example 1: Streaming a single artifact");
    println!("---------------------------------------");

    // These would be real values in production:
    let vm_ip = "192.168.122.201";
    let ssh_key = "/var/lib/libvirt/images/agent-01-ssh-key";
    let remote_path = "/home/agent/outbox/task-123/results.json";
    let local_path = Path::new("/tmp/results.json");

    // Note: This example shows the API - it won't actually run without a VM
    println!("Would transfer from: {}:{}", vm_ip, remote_path);
    println!("To local path: {:?}", local_path);

    // Uncomment to actually run (requires running VM):
    // match collector.stream_artifact(vm_ip, ssh_key, remote_path, local_path).await {
    //     Ok(metadata) => {
    //         println!("✓ Successfully transferred artifact");
    //         println!("  Size: {} bytes", metadata.size);
    //         println!("  SHA256: {}", metadata.sha256);
    //         println!("  Collected at: {}", metadata.collected_at);
    //     }
    //     Err(e) => eprintln!("✗ Transfer failed: {}", e),
    // }

    // Example 2: Verify checksum
    println!("\nExample 2: Checksum verification");
    println!("----------------------------------");

    let expected_checksum = "abc123def456...";
    println!("Expected checksum: {}", expected_checksum);

    // Uncomment to actually run:
    // match collector.verify_checksum(local_path, expected_checksum).await {
    //     Ok(true) => println!("✓ Checksum verified"),
    //     Ok(false) => println!("✗ Checksum mismatch!"),
    //     Err(e) => eprintln!("✗ Verification failed: {}", e),
    // }

    // Example 3: Collect all artifacts from task outbox
    println!("\nExample 3: Collect all task artifacts");
    println!("--------------------------------------");

    let task_id = "task-123";
    let local_dir = Path::new("/tmp/artifacts");

    println!(
        "Would collect all artifacts from VM {} task {}",
        vm_ip, task_id
    );
    println!("To local directory: {:?}", local_dir);

    // Uncomment to actually run:
    // match collector.collect_all(vm_ip, ssh_key, task_id, local_dir).await {
    //     Ok(artifacts) => {
    //         println!("✓ Collected {} artifacts:", artifacts.len());
    //         for artifact in artifacts {
    //             println!("  - {:?}", artifact.path.file_name().unwrap());
    //             println!("    {} bytes, SHA256: {}",
    //                 artifact.size,
    //                 &artifact.sha256[..16]
    //             );
    //         }
    //     }
    //     Err(e) => eprintln!("✗ Collection failed: {}", e),
    // }

    println!("\n✓ Example complete (demonstration mode)");
    println!("  To run with real VMs, uncomment the execution blocks");

    Ok(())
}
