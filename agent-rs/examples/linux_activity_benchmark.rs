use agent_client::linux_activity::{
    CollectorConfig, CollectorScope, KernelProcessEvent, LinuxActivityCollector, LinuxRuntime,
    ProcessKind, SourceLayer,
};
use serde_json::json;
use std::time::Instant;

fn main() {
    let count = std::env::args()
        .nth(1)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(100_000);
    let scope = CollectorScope {
        tenant_id: "benchmark-tenant".into(),
        host_id: "benchmark-host".into(),
        instance_id: "benchmark-instance".into(),
        agent_id: "benchmark-agent".into(),
        collector_id: "benchmark-linux-host".into(),
        boot_id: "00000000-0000-0000-0000-000000000000".into(),
        layer: SourceLayer::Host,
        runtime: LinuxRuntime::Docker,
    };
    let config = CollectorConfig {
        queue_capacity: count as usize,
        ..CollectorConfig::default()
    };
    let mut collector = LinuxActivityCollector::new(scope, config, 1).unwrap();
    let argv = ["task-worker".to_string(), "--safe".to_string()];
    let started = Instant::now();
    for pid in 1..=count {
        collector.observe_process(KernelProcessEvent {
            kind: ProcessKind::Exec,
            pid,
            ppid: Some(1),
            uid: Some(1000),
            gid: Some(1000),
            start_time_ticks: pid as u64 * 10,
            executable: Some("/usr/bin/task-worker"),
            argv: Some(&argv),
            cgroup: Some("/tenant/benchmark"),
            exit_code: None,
            source: "linux-audit",
        });
    }
    let normalization_elapsed = started.elapsed();
    let events = collector.drain(count as usize);
    let total_bytes: usize = events
        .iter()
        .map(|event| serde_json::to_vec(event).unwrap().len())
        .sum();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema_version": "activity.collector-benchmark/v1",
            "events": events.len(),
            "normalization_elapsed_ms": normalization_elapsed.as_secs_f64() * 1_000.0,
            "events_per_second": events.len() as f64 / normalization_elapsed.as_secs_f64(),
            "mean_event_bytes_precompression": total_bytes as f64 / events.len() as f64,
            "max_queue_depth": collector.health().max_queue_depth,
            "dropped": collector.health().dropped
        }))
        .unwrap()
    );
}
