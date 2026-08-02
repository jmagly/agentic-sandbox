//! Resumable, real-time activity ingest/query soak campaign.
//!
//! The default duration is seven wall-clock days. Short durations are intended
//! only for contract checks; reports always state the requested and completed
//! wall time so accelerated fixtures cannot be mistaken for soak evidence.

use agentic_management::activity::{
    ActivityCorrelation, ActivityEvent, ActivityIntegrity, ActivityOutcome, ActivityQuery,
    ActivityRuntime, ActivitySource, ActivityStore, EventPlane, IngestBatch, IngestScope,
    OutcomeStatus, RetentionClass, Sensitivity, SourceLayer, SourceTrust, ACTIVITY_SCHEMA_VERSION,
};
use chrono::{Duration as ChronoDuration, Utc};
use serde_json::{json, Map};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

const SEVEN_DAYS_SECONDS: u64 = 7 * 24 * 60 * 60;

#[derive(Debug)]
struct Config {
    duration_seconds: u64,
    rate: u64,
    burst_rate: u64,
    burst_seconds: u64,
    output: PathBuf,
    state_dir: PathBuf,
}

fn parse_args() -> Result<Config, String> {
    let mut duration_seconds = SEVEN_DAYS_SECONDS;
    let mut rate = 100;
    let mut burst_rate = 1_000;
    let mut burst_seconds = 60;
    let mut output = None;
    let mut state_dir = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("missing value for {arg}"))
        };
        match arg.as_str() {
            "--duration-seconds" => {
                duration_seconds = value()?.parse().map_err(|_| "invalid duration")?
            }
            "--rate" => rate = value()?.parse().map_err(|_| "invalid rate")?,
            "--burst-rate" => burst_rate = value()?.parse().map_err(|_| "invalid burst rate")?,
            "--burst-seconds" => {
                burst_seconds = value()?.parse().map_err(|_| "invalid burst duration")?
            }
            "--output" => output = Some(PathBuf::from(value()?)),
            "--state-dir" => state_dir = Some(PathBuf::from(value()?)),
            "--help" | "-h" => {
                return Err("usage: activity_reliability_campaign --output <report.json> [--state-dir <dir>] [--duration-seconds 604800] [--rate 100] [--burst-rate 1000] [--burst-seconds 60]".into());
            }
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }
    let output = output.ok_or_else(|| "--output is required".to_string())?;
    let state_dir = state_dir.unwrap_or_else(|| output.with_extension("state"));
    if duration_seconds == 0 || rate == 0 || burst_rate == 0 || burst_rate > 1_000 {
        return Err("duration/rates must be positive and batches cannot exceed 1000".into());
    }
    Ok(Config {
        duration_seconds,
        rate,
        burst_rate,
        burst_seconds,
        output,
        state_dir,
    })
}

fn scope() -> IngestScope {
    IngestScope {
        tenant_id: "soak-tenant".into(),
        host_id: "soak-host".into(),
        instance_id: "soak-instance".into(),
        agent_id: "soak-agent".into(),
        collector_id: "soak-collector".into(),
    }
}

fn event(sequence: u64) -> ActivityEvent {
    let now = Utc::now();
    let lifecycle = sequence % 10 == 0;
    ActivityEvent {
        schema_version: ACTIVITY_SCHEMA_VERSION.into(),
        event_id: Uuid::now_v7().to_string(),
        event_name: if lifecycle {
            "process.exec"
        } else {
            "runtime.resource_sample"
        }
        .into(),
        plane: if lifecycle {
            EventPlane::Action
        } else {
            EventPlane::Runtime
        },
        occurred_at: now,
        observed_at: now + ChronoDuration::milliseconds(1),
        source: ActivitySource {
            collector: "soak-collector".into(),
            layer: SourceLayer::Guest,
            runtime: ActivityRuntime::CloudHypervisor,
            trust: SourceTrust::Observed,
            clock_id: Some("soak-clock".into()),
            clock_error_ms: Some(1.0),
        },
        correlation: ActivityCorrelation {
            tenant_id: "soak-tenant".into(),
            host_id: "soak-host".into(),
            instance_id: "soak-instance".into(),
            agent_id: "soak-agent".into(),
            session_id: Some("soak-session".into()),
            mission_id: None,
            task_id: None,
            tool_call_id: lifecycle.then(|| format!("tool-{sequence}")),
            command_id: lifecycle.then(|| format!("command-{sequence}")),
            process_id: lifecycle.then(|| format!("process-{sequence}")),
            parent_event_id: None,
            trace_id: None,
            span_id: None,
        },
        actor: None,
        target: None,
        outcome: Some(ActivityOutcome {
            status: OutcomeStatus::Success,
            exit_code: None,
            reason: None,
        }),
        sensitivity: Sensitivity::Metadata,
        retention_class: if lifecycle {
            RetentionClass::Security
        } else {
            RetentionClass::Standard
        },
        payload: Map::from_iter([
            ("cpu_millis".into(), json!(sequence % 100)),
            ("content_captured".into(), json!(false)),
        ]),
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

fn percentile(values: &mut [u128], percentile: usize) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_unstable();
    let index = ((values.len() - 1) * percentile) / 100;
    values[index] as f64 / 1_000_000.0
}

fn resource_usage() -> (f64, u64) {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return (0.0, 0);
    }
    let usage = unsafe { usage.assume_init() };
    let seconds = usage.ru_utime.tv_sec as f64
        + usage.ru_utime.tv_usec as f64 / 1_000_000.0
        + usage.ru_stime.tv_sec as f64
        + usage.ru_stime.tv_usec as f64 / 1_000_000.0;
    #[cfg(target_os = "macos")]
    let resident = usage.ru_maxrss as u64;
    #[cfg(not(target_os = "macos"))]
    let resident = (usage.ru_maxrss as u64).saturating_mul(1024);
    (seconds, resident)
}

fn storage_bytes(path: &Path) -> (u64, u64, u64) {
    let size = |item: &Path| fs::metadata(item).map(|meta| meta.len()).unwrap_or(0);
    (
        size(path),
        size(&path.with_extension("db-wal")),
        size(&path.with_extension("db-shm")),
    )
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.next");
    fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    fs::create_dir_all(&config.state_dir)?;
    let database = config.state_dir.join("activity.db");
    let checkpoint_path = config.state_dir.join("checkpoint.json");
    let checkpoint = fs::read(&checkpoint_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let mut sequence = checkpoint
        .as_ref()
        .and_then(|v| v["next_sequence"].as_u64())
        .unwrap_or(1);
    let previously_completed = checkpoint
        .as_ref()
        .and_then(|v| v["completed_wall_seconds"].as_u64())
        .unwrap_or(0);
    let mut sequence_gap_count = checkpoint
        .as_ref()
        .and_then(|v| v["sequence_gap_count"].as_u64())
        .unwrap_or(0);
    let mut durable_through_sequence = checkpoint
        .as_ref()
        .and_then(|v| v["durable_through_sequence"].as_u64())
        .unwrap_or_else(|| sequence.saturating_sub(1));
    let remaining = config.duration_seconds.saturating_sub(previously_completed);
    let store = ActivityStore::open(&database)?;
    store.configure_export_signer("campaign-key-v1", vec![0x42; 32])?;
    let scope = scope();
    let started = Instant::now();
    let (cpu_started, _) = resource_usage();
    let mut ingest_latency = Vec::new();
    let mut query_latency = Vec::new();
    let mut accepted = 0_u64;
    let mut security_events = 0_u64;

    for second in 0..remaining {
        let tick = Instant::now();
        let effective_rate = if previously_completed + second < config.burst_seconds {
            config.burst_rate
        } else {
            config.rate
        };
        let mut events = Vec::with_capacity(effective_rate as usize);
        for _ in 0..effective_rate {
            let item = event(sequence);
            if item.retention_class == RetentionClass::Security {
                security_events += 1;
            }
            events.push(item);
            sequence += 1;
        }
        let ingest_started = Instant::now();
        let ack = store.ingest(&scope, IngestBatch { events })?;
        ingest_latency.push(ingest_started.elapsed().as_nanos());
        accepted += ack.accepted as u64;
        sequence_gap_count += ack.sequence_gaps_recorded.len() as u64;
        durable_through_sequence = ack.durable_through_sequence;
        let query_started = Instant::now();
        let _ = store.query(&scope, &ActivityQuery::default())?;
        query_latency.push(query_started.elapsed().as_nanos());
        write_json(
            &checkpoint_path,
            &json!({
                "schema_version": "agentic.activity-soak-checkpoint.v1",
                "next_sequence": sequence,
                "completed_wall_seconds": previously_completed + second + 1,
                "requested_wall_seconds": config.duration_seconds,
                "sequence_gap_count": sequence_gap_count,
                "durable_through_sequence": durable_through_sequence
            }),
        )?;
        if let Some(delay) = Duration::from_secs(1).checked_sub(tick.elapsed()) {
            thread::sleep(delay);
        }
    }

    let completed_this_run = started.elapsed().as_secs_f64();
    let total_completed = previously_completed as f64 + completed_this_run;
    let export = store.export(&scope, &ActivityQuery::default(), "soak-operator")?;
    let (precheckpoint_main, precheckpoint_wal, precheckpoint_shm) = storage_bytes(&database);
    store.checkpoint_wal()?;
    let (database_main, database_wal, database_shm) = storage_bytes(&database);
    let (cpu_finished, resident) = resource_usage();
    let cpu_seconds = (cpu_finished - cpu_started).max(0.0);
    let cpu_percent_of_one_core = if completed_this_run > 0.0 {
        cpu_seconds * 100.0 / completed_this_run
    } else {
        0.0
    };
    let database_size = database_main + database_wal + database_shm;
    let precheckpoint_size = precheckpoint_main + precheckpoint_wal + precheckpoint_shm;
    let bytes_per_event = if durable_through_sequence > 0 {
        database_size as f64 / durable_through_sequence as f64
    } else {
        0.0
    };
    let ingest_p50 = percentile(&mut ingest_latency.clone(), 50);
    let ingest_p95 = percentile(&mut ingest_latency.clone(), 95);
    let ingest_p99 = percentile(&mut ingest_latency.clone(), 99);
    let query_p50 = percentile(&mut query_latency.clone(), 50);
    let query_p95 = percentile(&mut query_latency.clone(), 95);
    let query_p99 = percentile(&mut query_latency.clone(), 99);
    let report = json!({
        "schema_version": "agentic.activity-soak-report.v1",
        "campaign": {
            "requested_wall_seconds": config.duration_seconds,
            "completed_wall_seconds": total_completed,
            "complete": total_completed >= config.duration_seconds as f64,
            "real_time": true,
            "accelerated": false,
            "steady_events_per_second": config.rate,
            "burst_events_per_second": config.burst_rate,
            "burst_seconds": config.burst_seconds
        },
        "pipeline": {
            "accepted_this_run": accepted,
            "security_events_this_run": security_events,
            "durable_through_sequence": durable_through_sequence,
            "sequence_gap_count": sequence_gap_count,
            "durable_loss_count": sequence_gap_count,
            "signed_export_event_count": export.manifest.event_count
        },
        "performance": {
            "build_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "cpu_seconds_this_run": cpu_seconds,
            "cpu_percent_of_one_core": cpu_percent_of_one_core,
            "maximum_resident_bytes": resident,
            "database_bytes": database_size,
            "database_main_bytes": database_main,
            "database_wal_bytes": database_wal,
            "database_shm_bytes": database_shm,
            "database_bytes_before_checkpoint": precheckpoint_size,
            "database_wal_bytes_before_checkpoint": precheckpoint_wal,
            "database_bytes_per_event": bytes_per_event,
            "ingest_batch_latency_p50_ms": ingest_p50,
            "ingest_batch_latency_p95_ms": ingest_p95,
            "ingest_batch_latency_p99_ms": ingest_p99,
            "query_latency_p50_ms": query_p50,
            "query_latency_p95_ms": query_p95,
            "query_latency_p99_ms": query_p99,
            "sample_count": ingest_latency.len()
        },
        "budget_assessment": {
            "burst_cpu_limit_percent": 10.0,
            "burst_cpu_passed": cpu_percent_of_one_core <= 10.0,
            "host_memory_limit_bytes": 134217728,
            "host_memory_passed": resident <= 134217728,
            "storage_limit_bytes_per_event": 1024,
            "storage_passed": bytes_per_event <= 1024.0,
            "durable_ingest_p95_limit_ms": 2000.0,
            "durable_ingest_p95_passed": ingest_p95 <= 2000.0,
            "action_latency_measured": false
        },
        "evidence_limits": [
            "short contract runs are not seven-day evidence",
            "runtime-specific collectors require their own supported hosts",
            "source delivery latency is excluded when a platform source is externally gated"
        ]
    });
    write_json(&config.output, &report)?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}
