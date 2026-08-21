import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { executeRecoveryDriver, RECOVERY_PREREQUISITES } from "../../../scripts/celld-live-recovery.mjs";

function orchestrationConfig(repoRoot) {
  return { schema_version: "agentic-sandbox.celld-live-orchestration/v1", run_id: "test-run", working_root: "/dev/shm/agentic-celld-orchestration/test-run", management_binary_path: `${repoRoot}/management/target/release/agentic-mgmt`, agent_client_binary_path: `${repoRoot}/management/target/release/agent-client`, callback_relay_binary_path: `${repoRoot}/tools/celld-callback-relay/target/x86_64-unknown-linux-musl/release/agentic-celld-callback-relay`, docker_image_ref: `sha256:${"a".repeat(64)}`, base_images_dir: "/build/agentic-sandbox/base-images", vm_storage_dir: "/build/agentic-sandbox/vms", agentshare_root: "/var/tmp/agentic-celld-qualification-123/mount", libvirt_uri: "qemu:///system", management_grpc_port: 38120 };
}

function fixture(enabled = true) {
  const directory = mkdtempSync(join(tmpdir(), "celld-live-recovery-test-"));
  const repoRoot = new URL("../../../", import.meta.url).pathname.replace(/\/$/, ""), host = "synthetic-titan";
  const configPath = join(directory, "orchestration.json"), profilePath = join(directory, "profile.json");
  const profile = { schema_version: "agentic-sandbox.celld-live-profile/v1", profile_id: "test-profile", run_id: "test-run", expected_sandbox_git: "1".repeat(40), environment: { kind: "disposable-local", single_host: true, host_sha256: createHash("sha256").update(host).digest("hex") }, authorization: { destructive_faults: false, inventory_path: "/tmp/test-inventory.json" }, drivers: { "celld-live-recovery": { enabled, config_path: configPath } } };
  writeFileSync(configPath, `${JSON.stringify(orchestrationConfig(repoRoot))}\n`, { mode: 0o600 });
  writeFileSync(profilePath, `${JSON.stringify(profile)}\n`, { mode: 0o600 });
  chmodSync(configPath, 0o600); chmodSync(profilePath, 0o600);
  return { directory, host, profilePath };
}

test("disabled recovery driver returns pre-mutation NOT_RUN", () => {
  const value = fixture(false);
  try {
    const observation = executeRecoveryDriver({ scenarioId: "UAT-CELLD-015", runId: "test-run", liveProfilePath: value.profilePath });
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_LIVE_RECOVERY_DRIVER_DISABLED");
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});

test("UAT-015 reports all unavailable recovery dependencies before mutation", () => {
  const value = fixture();
  try {
    const observation = executeRecoveryDriver({ scenarioId: "UAT-CELLD-015", runId: "test-run", liveProfilePath: value.profilePath }, { gitCommit: () => "1".repeat(40), hostname: () => value.host });
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
