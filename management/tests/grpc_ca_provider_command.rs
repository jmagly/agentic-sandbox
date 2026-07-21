use std::process::Command;
use std::time::Duration;

use agentic_management::grpc_ca_backend::{CommandGrpcCaBackend, GrpcCaBackend};
use agentic_management::grpc_local_ca::LocalCaOptions;
use rcgen::{CertificateParams, DistinguishedName, KeyPair, SanType};

fn csr_for(spiffe_id: &str) -> String {
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.distinguished_name = DistinguishedName::new();
    params
        .subject_alt_names
        .push(SanType::URI(spiffe_id.try_into().unwrap()));
    params.serialize_request(&key).unwrap().pem().unwrap()
}

#[test]
fn command_provider_describes_fetches_bundle_and_signs() {
    let ca_dir = tempfile::tempdir().unwrap();
    std::env::set_var("AGENTIC_GRPC_REMOTE_CA_MOCK_DIR", ca_dir.path());
    std::env::set_var(
        "AGENTIC_GRPC_CA_AGENT_LEAF_TTL_SECS",
        LocalCaOptions::default()
            .agent_leaf_ttl
            .as_secs()
            .to_string(),
    );

    let executable = env!("CARGO_BIN_EXE_grpc-ca-provider-mock");
    let backend = CommandGrpcCaBackend::connect(
        executable,
        "fleet-test.agentic.local",
        Duration::from_secs(3600),
    )
    .unwrap();
    let spiffe_id = "spiffe://fleet-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";
    let issued = backend
        .issue_agent_certificate_from_csr(spiffe_id, &csr_for(spiffe_id))
        .unwrap();

    assert_eq!(backend.backend_name(), "remote");
    assert_eq!(backend.trust_domain(), "fleet-test.agentic.local");
    assert!(backend.ca_pem().contains("BEGIN CERTIFICATE"));
    assert!(issued.cert_pem.contains("BEGIN CERTIFICATE"));
}

#[test]
fn command_provider_help_is_a_discovery_surface() {
    let output = Command::new(env!("CARGO_BIN_EXE_grpc-ca-provider-mock"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for command in ["describe", "trust-bundle", "sign", "health"] {
        assert!(help.contains(command), "help did not list {command}");
    }
}

#[test]
fn command_provider_rejects_relative_executable_paths() {
    let error = CommandGrpcCaBackend::connect(
        "grpc-ca-provider-mock",
        "fleet-test.agentic.local",
        Duration::from_secs(3600),
    )
    .err()
    .expect("relative provider executable must be rejected");
    assert!(error.to_string().contains("must be absolute"));
}

#[cfg(unix)]
fn write_provider_script(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&path, permissions).unwrap();
    (dir, path)
}

#[cfg(unix)]
#[test]
fn command_provider_times_out_fail_closed() {
    let (_dir, executable) = write_provider_script("sleep 2");
    let error = CommandGrpcCaBackend::connect_with_timeout(
        executable,
        "fleet-test.agentic.local",
        Duration::from_secs(3600),
        Duration::from_millis(50),
    )
    .err()
    .expect("hung provider must fail closed");
    assert!(error.to_string().contains("timed out"));
}

#[cfg(unix)]
#[test]
fn command_provider_rejects_malformed_response() {
    let (_dir, executable) = write_provider_script("printf 'not-json'");
    let error = CommandGrpcCaBackend::connect(
        executable,
        "fleet-test.agentic.local",
        Duration::from_secs(3600),
    )
    .err()
    .expect("malformed response must fail closed");
    assert!(
        error
            .to_string()
            .contains("parsing CA provider response JSON"),
        "unexpected error: {error:#}"
    );
}

#[cfg(unix)]
#[test]
fn command_provider_rejects_degraded_health() {
    let script = r#"
case "$1" in
  describe)
    printf '{"protocol":{"major":1,"minor":0},"implementation":"test","implementation_version":"1","build_provenance":null,"capabilities":["CSR_SIGNING"]}'
    ;;
  health)
    printf '{"protocol":{"major":1,"minor":0},"state":"degraded","diagnostics_code":"test_degraded"}'
    ;;
esac
"#;
    let (_dir, executable) = write_provider_script(script);
    let error = CommandGrpcCaBackend::connect(
        executable,
        "fleet-test.agentic.local",
        Duration::from_secs(3600),
    )
    .err()
    .expect("degraded provider must fail closed");
    assert!(
        error.to_string().contains("is not ready"),
        "unexpected error: {error:#}"
    );
}

#[cfg(unix)]
#[test]
fn command_provider_rejects_oversized_response() {
    let (_dir, executable) = write_provider_script("head -c 5000000 /dev/zero");
    let error = CommandGrpcCaBackend::connect(
        executable,
        "fleet-test.agentic.local",
        Duration::from_secs(3600),
    )
    .err()
    .expect("oversized response must fail closed");
    let detail = error.to_string();
    assert!(
        detail.contains("response exceeds") || detail.contains("command failed"),
        "unexpected error: {error:#}"
    );
}

#[cfg(unix)]
#[test]
fn command_provider_does_not_surface_stderr_secrets() {
    let (_dir, executable) =
        write_provider_script("echo 'provider-token=SYNTHETIC_SECRET_SENTINEL' >&2; exit 1");
    let error = CommandGrpcCaBackend::connect(
        executable,
        "fleet-test.agentic.local",
        Duration::from_secs(3600),
    )
    .err()
    .expect("failed provider must fail closed");
    let detail = error.to_string();
    assert!(detail.contains("diagnostics suppressed"));
    assert!(!detail.contains("SYNTHETIC_SECRET_SENTINEL"));
}
