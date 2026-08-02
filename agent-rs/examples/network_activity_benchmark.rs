use agent_client::linux_activity::{
    CollectorConfig, CollectorScope, FlowObservation, LinuxActivityCollector, LinuxRuntime,
    SourceLayer,
};
use serde_json::json;
use std::time::Instant;

fn main() {
    let count = std::env::args()
        .nth(1)
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(100_000);
    let scope = CollectorScope {
        tenant_id: "benchmark-tenant".into(),
        host_id: "benchmark-host".into(),
        instance_id: "benchmark-instance".into(),
        agent_id: "benchmark-agent".into(),
        collector_id: "benchmark-network-host".into(),
        boot_id: "benchmark-boot".into(),
        layer: SourceLayer::Host,
        runtime: LinuxRuntime::Docker,
    };
    let config = CollectorConfig {
        queue_capacity: count as usize,
        ..CollectorConfig::default()
    };
    let mut collector = LinuxActivityCollector::new(scope, config, 1).unwrap();
    let started = Instant::now();
    for port in 0..count {
        collector.observe_flow(FlowObservation {
            source_address: "10.0.0.2",
            source_port: 49152 + (port % 1024) as u16,
            destination_address: "203.0.113.8",
            destination_port: 443,
            protocol: "tcp",
            first_seen: "2026-08-01T20:00:00Z",
            last_seen: "2026-08-01T20:00:01Z",
            bytes_sent: 1200,
            bytes_received: 4500,
            packets_sent: 8,
            packets_received: 10,
            result: "allowed",
            process_id: Some("boot:42:99"),
            cgroup: Some("/tenant/benchmark"),
            nat_observed: true,
        });
    }
    let elapsed = started.elapsed();
    let events = collector.drain(count as usize);
    let bytes: usize = events
        .iter()
        .map(|event| serde_json::to_vec(event).unwrap().len())
        .sum();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema_version": "activity.network-benchmark/v1", "events": events.len(),
            "normalization_elapsed_ms": elapsed.as_secs_f64() * 1000.0,
            "events_per_second": events.len() as f64 / elapsed.as_secs_f64(),
            "mean_event_bytes_precompression": bytes as f64 / events.len() as f64,
            "dropped": collector.health().dropped
        }))
        .unwrap()
    );
}
