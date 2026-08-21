import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  executeOrchestrationDriver,
  requestHash,
  validateOrchestrationConfig,
} from "../../../scripts/celld-live-orchestration.mjs";

function config(overrides = {}) {
  return {
    schema_version: "agentic-sandbox.celld-live-orchestration/v1",
    run_id: "titan-123",
    working_root: "/dev/shm/agentic-celld-orchestration/titan-123",
    management_binary_path: "/repo/.celld-target/release/agentic-mgmt",
    agent_client_binary_path: "/repo/.celld-target/release/agent-client",
    callback_relay_binary_path: "/repo/tools/celld-callback-relay/target/x86_64-unknown-linux-musl/release/agentic-celld-callback-relay",
    docker_image_ref: `sha256:${"a".repeat(64)}`,
    base_images_dir: "/build/agentic-sandbox/base-images",
    vm_storage_dir: "/build/agentic-sandbox/vms",
    agentshare_root: "/var/tmp/agentic-celld-qualification-123/mount",
    libvirt_uri: "qemu:///system",
    management_grpc_port: 38120,
    ...overrides,
  };
}

test("orchestration config confines exact-run mutation targets and immutable inputs", () => {
  assert.deepEqual(validateOrchestrationConfig(config(), { repoRoot: "/repo" }), []);
  assert.match(validateOrchestrationConfig(config({ working_root: "/tmp/titan-123" }), { repoRoot: "/repo" }).join(";"), /below \/dev\/shm/);
  assert.match(validateOrchestrationConfig(config({ docker_image_ref: "latest" }), { repoRoot: "/repo" }).join(";"), /immutable local OCI image ID/);
  assert.match(validateOrchestrationConfig(config({ management_binary_path: "/tmp/agentic-mgmt" }), { repoRoot: "/repo" }).join(";"), /approved build target/);
  assert.match(validateOrchestrationConfig(config({ libvirt_uri: "qemu:///session" }), { repoRoot: "/repo" }).join(";"), /qemu:\/\/\/system/);
});

test("callback request hash matches the Rust and Worker canonical contract", () => {
  assert.equal(requestHash({
    operationId: "op-1", instanceId: "instance-a", generation: 1, action: "provision",
    payload: { name: "instance-a", runtime: "docker" },
  }), "1115e4f5a1657ff842d76a9798214266a4954fcb7985e80ddd473ecfac24fd0b");
});

test("disabled live orchestration returns pre-mutation NOT_RUN evidence", async () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-orchestration-test-"));
  try {
    const profilePath = join(directory, "profile.json");
    const profile = {
      schema_version: "agentic-sandbox.celld-live-profile/v1",
      profile_id: "test-profile",
      run_id: "test-run",
      expected_sandbox_git: "1".repeat(40),
      environment: { kind: "disposable-local", single_host: true, host_sha256: "2".repeat(64) },
      authorization: { destructive_faults: false, inventory_path: "/tmp/test-inventory.json" },
      drivers: { "celld-live-orchestration": { enabled: false, config_path: "/tmp/orchestration.json" } },
    };
    writeFileSync(profilePath, `${JSON.stringify(profile)}\n`, { mode: 0o600 });
    chmodSync(profilePath, 0o600);
    const observation = await executeOrchestrationDriver({ scenarioId: "UAT-CELLD-003", runId: "test-run", liveProfilePath: profilePath, artifactDir: join(directory, "artifacts") });
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_ORCHESTRATION_DRIVER_DISABLED");
    assert.equal(observation.cleanup.status, "not_required");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("fault scenarios require explicit exact-run destructive authorization before mutation", async () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-orchestration-auth-test-"));
  try {
    const configPath = join(directory, "orchestration.json");
    const profilePath = join(directory, "profile.json");
    const repoRoot = new URL("../../../", import.meta.url).pathname.replace(/\/$/, "");
    const liveConfig = config({
      run_id: "test-run",
      working_root: "/dev/shm/agentic-celld-orchestration/test-run",
      management_binary_path: `${repoRoot}/management/target/release/agentic-mgmt`,
      agent_client_binary_path: `${repoRoot}/management/target/release/agent-client`,
      callback_relay_binary_path: `${repoRoot}/tools/celld-callback-relay/target/x86_64-unknown-linux-musl/release/agentic-celld-callback-relay`,
    });
    writeFileSync(configPath, `${JSON.stringify(liveConfig)}\n`, { mode: 0o600 });
    const host = "synthetic-titan";
    const profile = {
      schema_version: "agentic-sandbox.celld-live-profile/v1", profile_id: "test-profile", run_id: "test-run",
      expected_sandbox_git: "1".repeat(40),
      environment: { kind: "titan-single-host", single_host: true, host_sha256: createHash("sha256").update(host).digest("hex") },
      authorization: { destructive_faults: false, inventory_path: "/tmp/test-inventory.json" },
      drivers: { "celld-live-orchestration": { enabled: true, config_path: configPath } },
    };
    writeFileSync(profilePath, `${JSON.stringify(profile)}\n`, { mode: 0o600 });
    chmodSync(configPath, 0o600); chmodSync(profilePath, 0o600);
    const observation = await executeOrchestrationDriver(
      { scenarioId: "UAT-CELLD-005", runId: "test-run", liveProfilePath: profilePath, artifactDir: join(directory, "artifacts") },
      { gitCommit: () => "1".repeat(40), hostname: () => host },
    );
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_DESTRUCTIVE_AUTHORIZATION_REQUIRED");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
