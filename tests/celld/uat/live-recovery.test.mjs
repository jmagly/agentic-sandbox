import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { executeRecoveryDriver, RECOVERY_PREREQUISITES, RECOVERY_READY_PREREQUISITES } from "../../../scripts/celld-live-recovery.mjs";
import { SAFE_LIVE_EVALUATORS } from "../../../scripts/celld-live-evaluators.mjs";

function orchestrationConfig(repoRoot) {
  return { schema_version: "agentic-sandbox.celld-live-orchestration/v1", run_id: "test-run", working_root: "/dev/shm/agentic-celld-orchestration/test-run", inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json", management_binary_path: `${repoRoot}/management/target/release/agentic-mgmt`, agent_client_binary_path: `${repoRoot}/management/target/release/agent-client`, callback_relay_binary_path: `${repoRoot}/tools/celld-callback-relay/target/x86_64-unknown-linux-musl/release/agentic-celld-callback-relay`, qemu_cleanup_helper_path: "/usr/libexec/agentic-sandbox/agentic-celld-qemu-cleanup-helper", qemu_cleanup_helper_sha256: "e".repeat(64), docker_image_ref: `sha256:${"a".repeat(64)}`, base_images_dir: "/build/agentic-sandbox/base-images", vm_storage_dir: "/build/agentic-sandbox/vms", agentshare_root: "/var/tmp/agentic-celld-qualification-123/mount", libvirt_uri: "qemu:///system", management_grpc_port: 38120 };
}

function fixture(enabled = true, authorized = false) {
  const directory = mkdtempSync(join(tmpdir(), "celld-live-recovery-test-"));
  const repoRoot = new URL("../../../", import.meta.url).pathname.replace(/\/$/, ""), host = "synthetic-titan";
  const configPath = join(directory, "orchestration.json"), profilePath = join(directory, "profile.json");
  const profile = { schema_version: "agentic-sandbox.celld-live-profile/v1", profile_id: "test-profile", run_id: "test-run", expected_sandbox_git: "1".repeat(40), environment: { kind: "disposable-local", single_host: true, host_sha256: createHash("sha256").update(host).digest("hex") }, authorization: { destructive_faults: authorized, inventory_path: "/tmp/test-inventory.json", ...(authorized ? { exact_run_owner: "test-run" } : {}) }, drivers: { "celld-live-recovery": { enabled, config_path: configPath } } };
  writeFileSync(configPath, `${JSON.stringify(orchestrationConfig(repoRoot))}\n`, { mode: 0o600 });
  writeFileSync(profilePath, `${JSON.stringify(profile)}\n`, { mode: 0o600 });
  chmodSync(configPath, 0o600); chmodSync(profilePath, 0o600);
  return { directory, host, profilePath };
}

function passingCampaign() {
  const digest = (value) => createHash("sha256").update(value).digest("hex");
  const restores = [1, 2].map((execution) => ({ execution, snapshot_version_id: `snapshot-version-${execution}`, source_prefix: "fleet/source", restore_prefix: `fleet/isolated-restore-${execution}`, isolated_restore: true, quarantined: true, source_writers_stopped: true, restore_authority_exclusive: true, latest_acknowledged_at_ms: 1_000, snapshot_captured_at_ms: 301_000, restore_started_at_ms: 302_000, restore_ready_at_ms: 1_802_000, generation_manifest_before_sha256: digest(`generation-${execution}`), generation_manifest_after_sha256: digest(`generation-${execution}`), tombstone_manifest_before_sha256: digest(`tombstone-${execution}`), tombstone_manifest_after_sha256: digest(`tombstone-${execution}`) }));
  const runbooks = ["node_loss", "full_restart", "authorization_loss", "snapshot_restore", "credential_rotation"].map((runbook) => ({ runbook, executions: [1, 2].map((ordinal) => ({ ordinal, operation_ids: [`operation-${runbook}`], lifecycle_effect_ids: [`effect-${runbook}`], state_sha256_after: digest(`state-${runbook}`) })), healed: true, cleanup_verified: true }));
  const evidence = {
    affected_fleet_store_id: "affected-store",
    external_evidence_store_id: "external-store",
    artifacts: ["snapshot_identity", "restore_timeline", "generation_comparison", "evidence_manifest"].map((kind) => ({ kind, storage_authority_id: "external-store", bytes: 1_024, sha256: digest(`evidence-${kind}`), downloaded_sha256: digest(`evidence-${kind}`), corruption_probe: { tampered_sha256: digest(`tampered-${kind}`), detected: true }, read_after_fleet_loss: true, retained: true })),
    affected_fleet_unavailable: true,
    external_evidence_store_reachable: true,
    manifest_verified: true,
    malicious_runner_tamper_proof_claimed: false,
  };
  return { mutation_started: true, restores, runbooks, evidence, baseline: { baseline_sha256: digest("baseline"), restored: true }, timeline: [{ sequence: 1, phase: "fixture" }], cleanup: { status: "passed", active_restore_authority: false, source_writers_stopped: false, affected_fleet_unavailable: false, restore_fixtures_removed: true, snapshot_resources_removed: true, external_evidence_retained: true } };
}

test("disabled recovery driver returns pre-mutation NOT_RUN", async () => {
  const value = fixture(false);
  try {
    const observation = await executeRecoveryDriver({ scenarioId: "UAT-CELLD-015", runId: "test-run", liveProfilePath: value.profilePath });
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_LIVE_RECOVERY_DRIVER_DISABLED");
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});

test("UAT-015 reports all unavailable recovery dependencies before mutation", async () => {
  const value = fixture();
  try {
    const observation = await executeRecoveryDriver({ scenarioId: "UAT-CELLD-015", runId: "test-run", liveProfilePath: value.profilePath }, { gitCommit: () => "1".repeat(40), hostname: () => value.host });
    assert.equal(observation.mutation_started, false);
    assert.deepEqual(observation.prerequisites, RECOVERY_PREREQUISITES);
    assert.deepEqual(observation.assertions, []);
    assert.equal(observation.cleanup.status, "not_required");
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});

test("recovery prerequisites distinguish auth, observability, snapshots, and external evidence", () => {
  assert.deepEqual(RECOVERY_PREREQUISITES.map((item) => item.id), ["CELLD_CREDENTIAL_PROVENANCE_CAMPAIGN", "CELLD_OBSERVABILITY_QUALIFICATION", "CELLD_VERSIONED_SNAPSHOT_FIXTURE", "CELLD_INDEPENDENT_EVIDENCE_STORE"]);
  assert.ok(RECOVERY_PREREQUISITES.every((item) => item.status === "unavailable"));
});

test("ready recovery prerequisites still require exact-run destructive authorization", async () => {
  const value = fixture();
  try {
    const observation = await executeRecoveryDriver({ scenarioId: "UAT-CELLD-015", runId: "test-run", liveProfilePath: value.profilePath }, { gitCommit: () => "1".repeat(40), hostname: () => value.host, prerequisites: () => RECOVERY_READY_PREREQUISITES });
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_DESTRUCTIVE_AUTHORIZATION_REQUIRED");
    assert.equal(observation.mutation_started, false);
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});

test("authorization cannot bypass the unavailable recovery adapter", async () => {
  const value = fixture(true, true);
  try {
    const observation = await executeRecoveryDriver({ scenarioId: "UAT-CELLD-015", runId: "test-run", liveProfilePath: value.profilePath }, { gitCommit: () => "1".repeat(40), hostname: () => value.host, prerequisites: () => RECOVERY_READY_PREREQUISITES });
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_RECOVERY_ADAPTER_UNAVAILABLE");
    assert.equal(observation.mutation_started, false);
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});

test("injected recovery controller output becomes protected artifact-backed evaluator input", async () => {
  const value = fixture(true, true);
  try {
    const observation = await executeRecoveryDriver(
      { scenarioId: "UAT-CELLD-015", runId: "test-run", liveProfilePath: value.profilePath, artifactDir: join(value.directory, "artifacts") },
      { gitCommit: () => "1".repeat(40), hostname: () => value.host, prerequisites: () => RECOVERY_READY_PREREQUISITES, recoveryAdapter: {}, executeCampaign: async () => passingCampaign() },
    );
    assert.equal(observation.mutation_started, true);
    assert.equal(observation.assertions.length, 3);
    assert.equal(observation.artifacts.length, 2);
    for (const assertion of observation.assertions) assert.equal(SAFE_LIVE_EVALUATORS[assertion.id](assertion.measurements).passed, true, assertion.id);
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});

test("recovery prerequisite overrides must match the exact status contract", async () => {
  const value = fixture();
  try {
    await assert.rejects(
      executeRecoveryDriver({ scenarioId: "UAT-CELLD-015", runId: "test-run", liveProfilePath: value.profilePath }, { gitCommit: () => "1".repeat(40), hostname: () => value.host, prerequisites: () => RECOVERY_READY_PREREQUISITES.slice(1) }),
      /exact prerequisite inventory/,
    );
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});
