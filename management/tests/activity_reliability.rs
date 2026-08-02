use agentic_management::activity::{
    ActivityCorrelation, ActivityEvent, ActivityIntegrity, ActivityOutcome, ActivityQuery,
    ActivityRuntime, ActivitySource, ActivityStore, EventPlane, IngestBatch, IngestScope,
    OutcomeStatus, RetentionClass, Sensitivity, SourceLayer, SourceTrust, ACTIVITY_SCHEMA_VERSION,
};
use agentic_management::activity_governance::BatchManifest;
use chrono::{Duration, Utc};
use serde_json::{json, Map, Value};
use uuid::Uuid;

fn scope(tenant: &str) -> IngestScope {
    IngestScope {
        tenant_id: tenant.into(),
        host_id: "host-a".into(),
        instance_id: "instance-a".into(),
        agent_id: "agent-a".into(),
        collector_id: "collector-a".into(),
    }
}

fn event(tenant: &str, sequence: u64, name: &str, retention: RetentionClass) -> ActivityEvent {
    let now = Utc::now();
    ActivityEvent {
        schema_version: ACTIVITY_SCHEMA_VERSION.into(),
        event_id: Uuid::now_v7().to_string(),
        event_name: name.into(),
        plane: if name.starts_with("telemetry.") {
            EventPlane::Integrity
        } else {
            EventPlane::Action
        },
        occurred_at: now,
        observed_at: now + Duration::milliseconds(1),
        source: ActivitySource {
            collector: "collector-a".into(),
            layer: SourceLayer::Guest,
            runtime: ActivityRuntime::QemuKvm,
            trust: SourceTrust::Observed,
            clock_id: Some("clock-a".into()),
            clock_error_ms: Some(1.0),
        },
        correlation: ActivityCorrelation {
            tenant_id: tenant.into(),
            host_id: "host-a".into(),
            instance_id: "instance-a".into(),
            agent_id: "agent-a".into(),
            session_id: Some("session-a".into()),
            mission_id: None,
            task_id: None,
            tool_call_id: Some("tool-a".into()),
            command_id: Some("command-a".into()),
            process_id: Some(format!("pid-generation-{sequence}")),
            parent_event_id: None,
            trace_id: None,
            span_id: None,
        },
        actor: None,
        target: None,
        outcome: Some(ActivityOutcome {
            status: OutcomeStatus::Success,
            exit_code: Some(0),
            reason: None,
        }),
        sensitivity: Sensitivity::Metadata,
        retention_class: retention,
        payload: Map::from_iter([("content_captured".into(), json!(false))]),
        integrity: ActivityIntegrity {
            collector_sequence: sequence,
            source_hash: None,
            source_previous_hash: None,
            timeline_hash: None,
            timeline_previous_hash: None,
            signature: None,
            key_id: None,
        },
    }
}

#[test]
fn steady_security_lifecycle_ingest_has_zero_loss_and_signed_export() {
    let store = ActivityStore::in_memory().unwrap();
    store
        .configure_export_signer("test-key", vec![0x42; 32])
        .unwrap();
    let scope = scope("tenant-a");
    for batch_index in 0..10_u64 {
        let events = (1..=100_u64)
            .map(|offset| {
                event(
                    "tenant-a",
                    batch_index * 100 + offset,
                    "process.exec",
                    RetentionClass::Security,
                )
            })
            .collect();
        let ack = store.ingest(&scope, IngestBatch { events }).unwrap();
        assert_eq!(ack.accepted, 100);
        assert!(ack.sequence_gaps_recorded.is_empty());
    }
    let result = store.query(&scope, &ActivityQuery::default()).unwrap();
    assert!(result.completeness.complete);
    assert_eq!(result.completeness.sequence_gap_count, 0);
    assert_eq!(result.completeness.durable_loss_count, 0);
    let export = store
        .export(&scope, &ActivityQuery::default(), "operator-a")
        .unwrap();
    assert_eq!(export.manifest.event_count, result.events.len());
    assert!(!export.manifest.signature.is_empty());
}

#[test]
fn duplicate_gap_and_lower_priority_loss_are_durable_and_bounded() {
    let store = ActivityStore::in_memory().unwrap();
    let scope = scope("tenant-a");
    let first = event("tenant-a", 1, "process.exec", RetentionClass::Security);
    store
        .ingest(
            &scope,
            IngestBatch {
                events: vec![first.clone()],
            },
        )
        .unwrap();
    let duplicate = store
        .ingest(
            &scope,
            IngestBatch {
                events: vec![first],
            },
        )
        .unwrap();
    assert_eq!((duplicate.accepted, duplicate.duplicates), (0, 1));

    let third = event(
        "tenant-a",
        3,
        "runtime.resource_sample",
        RetentionClass::Standard,
    );
    let gap_ack = store
        .ingest(
            &scope,
            IngestBatch {
                events: vec![third],
            },
        )
        .unwrap();
    assert_eq!(gap_ack.durable_through_sequence, 1);
    assert_eq!(gap_ack.sequence_gaps_recorded[0].first_missing_sequence, 2);
    let mut loss = event("tenant-a", 4, "telemetry.loss", RetentionClass::Security);
    loss.payload.insert("dropped_events".into(), json!(1));
    loss.payload
        .insert("first_missing_sequence".into(), json!(2));
    loss.payload
        .insert("last_missing_sequence".into(), json!(2));
    store
        .ingest(&scope, IngestBatch { events: vec![loss] })
        .unwrap();
    let coverage = store.query(&scope, &ActivityQuery::default()).unwrap();
    assert_eq!(coverage.completeness.sequence_gap_count, 1);
    assert_eq!(coverage.completeness.durable_loss_count, 1);
    assert_eq!(coverage.completeness.dropped_event_count, 1);
}

#[test]
fn scope_spoofing_and_secret_sentinel_fields_never_reach_query_or_export() {
    let store = ActivityStore::in_memory().unwrap();
    store
        .configure_export_signer("test-key", vec![0x42; 32])
        .unwrap();
    let scope_a = scope("tenant-a");
    let mut spoofed = event("tenant-b", 1, "process.exec", RetentionClass::Security);
    assert!(store
        .ingest(
            &scope_a,
            IngestBatch {
                events: vec![spoofed.clone()]
            }
        )
        .is_err());
    spoofed.correlation.tenant_id = "tenant-a".into();
    spoofed.payload.insert(
        "authorization_token".into(),
        json!("SENTINEL-MUST-NOT-PERSIST"),
    );
    assert!(store
        .ingest(
            &scope_a,
            IngestBatch {
                events: vec![spoofed]
            }
        )
        .is_err());
    assert!(store
        .query(&scope_a, &ActivityQuery::default())
        .unwrap()
        .events
        .is_empty());
    store
        .ingest(
            &scope_a,
            IngestBatch {
                events: vec![event(
                    "tenant-a",
                    1,
                    "process.exec",
                    RetentionClass::Security,
                )],
            },
        )
        .unwrap();
    let export = store
        .export(&scope_a, &ActivityQuery::default(), "operator-a")
        .unwrap();
    let encoded = serde_json::to_string(&export).unwrap();
    assert!(!encoded.contains("SENTINEL-MUST-NOT-PERSIST"));
}

#[test]
fn query_scope_isolates_tenants_even_with_reused_pid_and_nat_metadata() {
    let store = ActivityStore::in_memory().unwrap();
    for tenant in ["tenant-a", "tenant-b"] {
        let mut item = event(tenant, 1, "network.flow", RetentionClass::Security);
        item.correlation.process_id = Some("reused-pid-generation-1".into());
        item.payload.insert("nat_observed".into(), json!(true));
        item.payload.insert("destination_port".into(), json!(443));
        store
            .ingest(&scope(tenant), IngestBatch { events: vec![item] })
            .unwrap();
    }
    let a = store
        .query(&scope("tenant-a"), &ActivityQuery::default())
        .unwrap();
    let b = store
        .query(&scope("tenant-b"), &ActivityQuery::default())
        .unwrap();
    assert_eq!(a.events.len(), 1);
    assert_eq!(b.events.len(), 1);
    assert_eq!(a.events[0].correlation.tenant_id, "tenant-a");
    assert_eq!(b.events[0].correlation.tenant_id, "tenant-b");
}

#[test]
fn anchored_manifest_detects_mutation_removal_and_stale_signing_key() {
    let key = [0x42_u8; 32];
    let stale_key = [0x24_u8; 32];
    let events = vec![json!({"event_id":"one"}), json!({"event_id":"two"})];
    let manifest =
        BatchManifest::sign("tenant-a", "collector-a", &events, None, "key-v1", &key).unwrap();
    let anchor = manifest.checkpoint("anchor-a", Utc::now());
    assert!(manifest.verify(&events, &key, &anchor).is_ok());

    let mut mutated = events.clone();
    mutated[0]["event_id"] = json!("changed");
    assert!(manifest.verify(&mutated, &key, &anchor).is_err());
    assert!(manifest.verify(&events[..1], &key, &anchor).is_err());
    assert!(manifest.verify(&events, &stale_key, &anchor).is_err());

    let next = BatchManifest::sign(
        "tenant-a",
        "collector-a",
        &[Value::String("next".into())],
        Some(manifest.merkle_root.clone()),
        "key-v1",
        &key,
    )
    .unwrap();
    assert!(next.verify_chain(&manifest).is_ok());
}
