import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { executeObservabilityDriver, OBSERVABILITY_PREREQUISITES } from "../../../scripts/celld-live-observability.mjs";

function orchestrationConfig(repoRoot) {
  return { schema_version: "agentic-sandbox.celld-live-orchestration/v1", run_id: "test-run", working_root: "/dev/shm/agentic-celld-orchestration/test-run", inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json", management_binary_path: `${repoRoot}/management/target/release/agentic-mgmt`, agent_client_binary_path: `${repoRoot}/management/target/release/agent-client`, callback_relay_binary_path: `${repoRoot}/tools/celld-callback-relay/target/x86_64-unknown-linux-musl/release/agentic-celld-callback-relay`, docker_image_ref: `sha256:${"a".repeat(64)}`, base_images_dir: "/build/agentic-sandbox/base-images", vm_storage_dir: "/build/agentic-sandbox/vms", agentshare_root: "/var/tmp/agentic-celld-qualification-123/mount", libvirt_uri: "qemu:///system", management_grpc_port: 38120 };
}

function profile(configPath, hostHash, enabled = true) {
  return { schema_version: "agentic-sandbox.celld-live-profile/v1", profile_id: "test-profile", run_id: "test-run", expected_sandbox_git: "1".repeat(40), environment: { kind: "disposable-local", single_host: true, host_sha256: hostHash }, authorization: { destructive_faults: false, inventory_path: "/tmp/test-inventory.json" }, drivers: { "celld-live-observability": { enabled, config_path: configPath } } };
}

function fixture(enabled = true) {
  const directory = mkdtempSync(join(tmpdir(), "celld-live-observability-test-"));
  const repoRoot = new URL("../../../", import.meta.url).pathname.replace(/\/$/, ""), host = "synthetic-titan";
  const configPath = join(directory, "orchestration.json"), profilePath = join(directory, "profile.json");
  writeFileSync(configPath, `${JSON.stringify(orchestrationConfig(repoRoot))}\n`, { mode: 0o600 });
  writeFileSync(profilePath, `${JSON.stringify(profile(configPath, createHash("sha256").update(host).digest("hex"), enabled))}\n`, { mode: 0o600 });
  chmodSync(configPath, 0o600); chmodSync(profilePath, 0o600);
  return { directory, host, profilePath };
}

test("disabled observability driver returns pre-mutation NOT_RUN", () => {
  const value = fixture(false);
  try {
    const observation = executeObservabilityDriver({ scenarioId: "UAT-CELLD-014", runId: "test-run", liveProfilePath: value.profilePath });
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_LIVE_OBSERVABILITY_DRIVER_DISABLED");
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});

test("UAT-014 reports every unavailable dependency before fault injection", () => {
  const value = fixture();
  try {
    const observation = executeObservabilityDriver({ scenarioId: "UAT-CELLD-014", runId: "test-run", liveProfilePath: value.profilePath }, { gitCommit: () => "1".repeat(40), hostname: () => value.host });
    assert.equal(observation.mutation_started, false);
    assert.deepEqual(observation.prerequisites, OBSERVABILITY_PREREQUISITES);
    assert.deepEqual(observation.assertions, []);
    assert.equal(observation.cleanup.status, "not_required");
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});

test("observability prerequisites preserve separate authorization, rollout, and telemetry blockers", () => {
  assert.deepEqual(OBSERVABILITY_PREREQUISITES.map((item) => item.id), ["CELLD_CREDENTIAL_PROVENANCE_CAMPAIGN", "CELLD_ROLLOUT_QUALIFICATION", "CELLD_LIVE_TELEMETRY_STACK"]);
  assert.ok(OBSERVABILITY_PREREQUISITES.every((item) => item.status === "unavailable"));
});
