use agentic_management::celld::{CelldFleetManifest, SUPPORTED_CELLD_VERSION};
use clap::Parser;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const REPORT_SCHEMA: &str = "agentic-sandbox.celld-node-preflight/v1";

#[derive(Debug, Parser)]
#[command(about = "Read-only local Celld service preflight")]
struct Args {
    #[arg(long)]
    manifest: PathBuf,
    #[arg(long, default_value = "/opt/agentic-sandbox/celld/v0.2.1/celld")]
    celld: PathBuf,
}

#[derive(Debug, Serialize)]
struct Check {
    id: &'static str,
    status: &'static str,
    reason_code: String,
}

#[derive(Debug, Serialize)]
struct Report {
    schema_version: &'static str,
    scope: &'static str,
    status: &'static str,
    reason_code: &'static str,
    ready: bool,
    mutating: bool,
    live_qualification: bool,
    fleet_id: Option<String>,
    checks: Vec<Check>,
    observations: BTreeMap<&'static str, serde_json::Value>,
}

fn main() {
    let args = Args::parse();
    let report = evaluate(&args);
    println!(
        "{}",
        serde_json::to_string(&report).expect("serialize report")
    );
    std::process::exit(if report.ready { 0 } else { 1 });
}

fn evaluate(args: &Args) -> Report {
    let mut checks = Vec::new();
    let manifest = read_manifest(&args.manifest, &mut checks);
    let artifact = inspect_artifact(&args.celld, &mut checks);
    inspect_credentials(&mut checks);
    let mut observations = BTreeMap::new();

    if let Some(manifest) = &manifest {
        inspect_environment(manifest, &mut checks, &mut observations);
    }
    observations.insert("artifact", artifact);
    observations.insert(
        "membership",
        serde_json::json!({"state":"not_observed_prestart"}),
    );

    let ready = checks.iter().all(|check| check.status == "PASS");
    Report {
        schema_version: REPORT_SCHEMA,
        scope: "local_prestart",
        status: if ready { "PASS" } else { "FAIL" },
        reason_code: if ready {
            "celld.preflight.local_ready"
        } else {
            "celld.preflight.local_check_failed"
        },
        ready,
        mutating: false,
        live_qualification: false,
        fleet_id: manifest.map(|value| value.fleet_id),
        checks,
        observations,
    }
}

fn read_manifest(path: &Path, checks: &mut Vec<Check>) -> Option<CelldFleetManifest> {
    let result = fs::read(path)
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            serde_json::from_slice::<CelldFleetManifest>(&bytes).map_err(|error| error.to_string())
        })
        .and_then(|manifest| {
            manifest
                .validate()
                .map(|()| manifest)
                .map_err(|error| error.to_string())
        });
    match result {
        Ok(manifest) => {
            passed(checks, "celld.preflight.manifest");
            Some(manifest)
        }
        Err(error) => {
            failed(checks, "celld.preflight.manifest", &error);
            None
        }
    }
}

fn inspect_artifact(path: &Path, checks: &mut Vec<Check>) -> serde_json::Value {
    let expected = std::env::var("AGENTIC_CELLD_EXPECTED_BINARY_SHA256").ok();
    let bytes = fs::read(path);
    let digest = bytes
        .as_ref()
        .ok()
        .map(|bytes| format!("sha256:{:x}", Sha256::digest(bytes)));
    let version = Command::new(path)
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok()
            } else {
                None
            }
        });
    let version_matches = version.as_deref().is_some_and(|value| {
        value.trim() == format!("celld {}", SUPPORTED_CELLD_VERSION.trim_start_matches('v'))
    });
    let digest_matches = expected
        .as_ref()
        .zip(digest.as_ref())
        .is_some_and(|(expected, observed)| expected == observed);
    if version_matches && digest_matches {
        passed(checks, "celld.preflight.artifact");
    } else {
        failed(
            checks,
            "celld.preflight.artifact",
            "pinned version or binary digest did not match",
        );
    }
    serde_json::json!({
        "celld_version": version.map(|value| value.trim().to_owned()),
        "binary_sha256": digest,
        "expected_digest_configured": expected.is_some()
    })
}

fn inspect_credentials(checks: &mut Vec<Check>) {
    let path = std::env::var_os("AWS_SHARED_CREDENTIALS_FILE")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("CREDENTIALS_DIRECTORY")
                .map(|dir| PathBuf::from(dir).join("object-store"))
        });
    let safe = path.as_deref().is_some_and(|path| {
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return false;
        };
        metadata.file_type().is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() > 0
            && metadata.mode() & 0o077 == 0
    });
    if safe {
        passed(checks, "celld.preflight.credential_file");
    } else {
        failed(
            checks,
            "celld.preflight.credential_file",
            "credential file must be a nonempty regular file with mode 0600 or stricter",
        );
    }
}

fn inspect_environment(
    manifest: &CelldFleetManifest,
    checks: &mut Vec<Check>,
    observations: &mut BTreeMap<&'static str, serde_json::Value>,
) {
    let bucket = std::env::var("CELLD_BUCKET").ok();
    let endpoint = std::env::var("S3_ENDPOINT").ok();
    let public = std::env::var("CELLD_ADDR").ok();
    let internal = std::env::var("CELLD_INTERNAL_ADDR").ok();
    let advertise = std::env::var("CELLD_ADVERTISE").ok();
    let node = std::env::var("CELLD_NODE").ok();
    let storage_probe = std::env::var("CELLD_STORAGE_PROBE").unwrap_or_else(|_| "1".into()) != "0";
    let expected_bucket = format!(
        "s3://{}/{}",
        manifest.storage.bucket,
        manifest.storage.prefix.trim_matches('/')
    );
    let endpoint_matches = endpoint.as_deref() == manifest.storage.endpoint.as_deref();
    let configured = bucket.as_deref() == Some(expected_bucket.as_str())
        && endpoint_matches
        && public.as_deref() == Some(manifest.network.public_listener.as_str())
        && internal.as_deref() == Some(manifest.network.internal_listener.as_str())
        && advertise
            .as_ref()
            .is_some_and(|value| manifest.network.advertised_addresses.contains(value))
        && node.as_ref().is_some_and(|value| !value.trim().is_empty())
        && storage_probe;
    if configured {
        passed(checks, "celld.preflight.environment");
    } else {
        failed(
            checks,
            "celld.preflight.environment",
            "node environment does not match the fleet manifest or storage probe is disabled",
        );
    }
    observations.insert(
        "backend",
        serde_json::json!({"substrate":manifest.nodes.substrate,"reachable":"not_observed_prestart"}),
    );
    observations.insert(
        "listeners",
        serde_json::json!({"public":public,"internal":internal,"advertise":advertise,"node_configured":node.is_some()}),
    );
    observations.insert(
        "store",
        serde_json::json!({
            "provider":manifest.storage.provider,
            "bucket":manifest.storage.bucket,
            "prefix":manifest.storage.prefix,
            "endpoint_configured":endpoint.is_some(),
            "startup_probe_enabled":storage_probe,
            "reachable":"not_observed_prestart"
        }),
    );
}

fn passed(checks: &mut Vec<Check>, id: &'static str) {
    checks.push(Check {
        id,
        status: "PASS",
        reason_code: format!("{id}.passed"),
    });
}

fn failed(checks: &mut Vec<Check>, id: &'static str, detail: &str) {
    checks.push(Check {
        id,
        status: "FAIL",
        reason_code: format!("{id}.failed:{}", sanitize(detail)),
    });
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
        .take(96)
        .collect()
}
