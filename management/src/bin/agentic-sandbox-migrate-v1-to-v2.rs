//! CLI wrapper around [`agentic_management::aiwg_serve::migration::migrate`].
//!
//! Migrates a v1 `missions.json` (the persisted MissionStore) into a v2
//! `missions.db` (A2A TaskStore, #205 schema).
//!
//! # State mapping
//!
//! | v1 MissionState | v2 TaskState (wire) | Notes |
//! |---|---|---|
//! | Assigned     | submitted        | |
//! | Running      | working          | |
//! | HitlRequired | input-required   | |
//! | Suspended    | working          | metadata note records v1 origin |
//! | Completed    | completed        | terminal |
//! | Failed       | failed           | fail_kind=infrastructure (ADR-007 default) |
//! | Aborted      | canceled         | terminal |
//!
//! On success the v1 file is renamed to
//! `missions.json.v1-archived-<RFC3339>` next to the original.

use std::path::PathBuf;
use std::process::ExitCode;

use agentic_management::aiwg_serve::migration::migrate;
use clap::Parser;

/// Migrate a v1 missions.json store into a v2 missions.db (A2A TaskStore).
///
/// State mapping:
///   Assigned     → submitted
///   Running      → working
///   HitlRequired → input-required
///   Suspended    → working (metadata.note records v1 origin)
///   Completed    → completed
///   Failed       → failed (fail_kind=infrastructure per ADR-007 default)
///   Aborted      → canceled
#[derive(Parser, Debug)]
#[command(
    name = "agentic-sandbox-migrate-v1-to-v2",
    version,
    about = "Migrate v1 missions.json → v2 missions.db (A2A TaskStore)",
    long_about = None,
)]
struct Cli {
    /// Path to the v1 missions.json file.
    #[arg(long = "in", value_name = "PATH")]
    input: PathBuf,

    /// Path to the v2 missions.db SQLite file (created if missing).
    #[arg(long = "out", value_name = "PATH")]
    output: PathBuf,

    /// Permit merging into an already-populated v2 DB. Does not clear.
    #[arg(long)]
    force: bool,

    /// Validate and map without writing to the DB or archiving the v1 file.
    #[arg(long = "dry-run")]
    dry_run: bool,
}

fn main() -> ExitCode {
    // Quiet default tracing; users can override via RUST_LOG.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();

    match migrate(&cli.input, &cli.output, cli.force, cli.dry_run) {
        Ok(report) => {
            print!("{}", report.summary());
            if cli.dry_run {
                println!("(dry-run: no changes written)");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("migration failed: {e:#}");
            ExitCode::FAILURE
        }
    }
}
