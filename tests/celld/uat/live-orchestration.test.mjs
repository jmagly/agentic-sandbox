import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmodSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  applyPlannedOrchestrationFault,
  executeOrchestrationDriver,
  healPlannedOrchestrationFault,
  issueCommand,
  loadAuthorizedOrchestrationInventory,
  observeOrchestrationProvider,
  requestHash,
  validateOrchestrationConfig,
  validateOrchestrationInventory,
} from "../../../scripts/celld-live-orchestration.mjs";

function config(overrides = {}) {
  return {
    schema_version: "agentic-sandbox.celld-live-orchestration/v1",
    run_id: "titan-123",
    working_root: "/dev/shm/agentic-celld-orchestration/titan-123",
    inventory_path: "/dev/shm/agentic-celld-orchestration/titan-123/orchestration-inventory.json",
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
  assert.match(validateOrchestrationConfig(config({ inventory_path: "/dev/shm/another-run/orchestration-inventory.json" }), { repoRoot: "/repo" }).join(";"), /fixed working-root file/);
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

test("every live orchestration scenario requires explicit exact-run destructive authorization before mutation", async () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-orchestration-auth-test-"));
  try {
    const configPath = join(directory, "orchestration.json");
    const profilePath = join(directory, "profile.json");
    const repoRoot = new URL("../../../", import.meta.url).pathname.replace(/\/$/, "");
    const liveConfig = config({
      run_id: "test-run",
      working_root: "/dev/shm/agentic-celld-orchestration/test-run",
      inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
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
    let prerequisiteCalls = 0;
    for (const scenarioId of ["UAT-CELLD-003", "UAT-CELLD-004", "UAT-CELLD-005", "UAT-CELLD-006"]) {
      const observation = await executeOrchestrationDriver(
        { scenarioId, runId: "test-run", liveProfilePath: profilePath, artifactDir: join(directory, "artifacts") },
        { gitCommit: () => "1".repeat(40), hostname: () => host, prerequisiteReason: () => { prerequisiteCalls += 1; return null; } },
      );
      assert.equal(observation.mutation_started, false);
      assert.equal(observation.prerequisites[0].reason_code, "CELLD_DESTRUCTIVE_AUTHORIZATION_REQUIRED");
    }
    assert.equal(prerequisiteCalls, 0, "no host or mutation prerequisite may run before authorization");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

function inventory(overrides = {}) {
  return {
    schema_version: "agentic-sandbox.celld-orchestration-inventory/v1",
    run_id: "test-run",
    working_root: "/dev/shm/agentic-celld-orchestration/test-run",
    owner: { repository: "roctinam/agentic-sandbox", workflow: "celld-qualification.yml", run_id: "test-run" },
    host_sha256: "2".repeat(64),
    created_at: "2026-08-23T00:00:00.000Z",
    updated_at: "2026-08-23T00:00:00.000Z",
    state: "prepared",
    resources: [],
    faults: [],
    ...overrides,
  };
}

test("orchestration inventory binds the exact run, repository, workflow, host, and fixed path", () => {
  const liveConfig = config({
    run_id: "test-run",
    working_root: "/dev/shm/agentic-celld-orchestration/test-run",
    inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
  });
  assert.deepEqual(validateOrchestrationInventory(inventory(), liveConfig, { expectedHostSha256: "2".repeat(64) }), []);
  for (const changed of [
    { run_id: "foreign-run" },
    { owner: { ...inventory().owner, repository: "someone/else" } },
    { owner: { ...inventory().owner, workflow: "foreign.yml" } },
    { owner: { ...inventory().owner, run_id: "foreign-run" } },
    { host_sha256: "3".repeat(64) },
    { working_root: "/dev/shm/agentic-celld-orchestration/foreign-run" },
  ]) {
    assert.notDeepEqual(validateOrchestrationInventory(inventory(changed), liveConfig, { expectedHostSha256: "2".repeat(64) }), []);
  }
});

test("authorized orchestration inventory rejects missing, symlinked, group-readable, and wrong-run files", () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-orchestration-inventory-test-"));
  try {
    const inventoryPath = join(directory, "orchestration-inventory.json");
    const liveConfig = config({ run_id: "test-run", working_root: directory, inventory_path: inventoryPath });
    const profile = { run_id: "test-run", authorization: { destructive_faults: true, exact_run_owner: "test-run", inventory_path: inventoryPath } };
    assert.throws(() => loadAuthorizedOrchestrationInventory(profile, liveConfig, "2".repeat(64)), /missing/);

    writeFileSync(inventoryPath, `${JSON.stringify(inventory({ working_root: directory }))}\n`, { mode: 0o640 });
    assert.throws(() => loadAuthorizedOrchestrationInventory(profile, liveConfig, "2".repeat(64)), /protected regular non-symlink/);
    chmodSync(inventoryPath, 0o600);
    assert.deepEqual(loadAuthorizedOrchestrationInventory(profile, liveConfig, "2".repeat(64)).resources, []);

    rmSync(inventoryPath);
    const target = join(directory, "target.json");
    writeFileSync(target, `${JSON.stringify(inventory({ working_root: directory }))}\n`, { mode: 0o600 });
    symlinkSync(target, inventoryPath);
    assert.throws(() => loadAuthorizedOrchestrationInventory(profile, liveConfig, "2".repeat(64)), /protected regular non-symlink/);

    rmSync(inventoryPath);
    writeFileSync(inventoryPath, `${JSON.stringify(inventory({ working_root: directory, run_id: "foreign-run" }))}\n`, { mode: 0o600 });
    assert.throws(() => loadAuthorizedOrchestrationInventory(profile, liveConfig, "2".repeat(64)), /inventory run\/owner/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("provider resources and fault targets are durably planned before mutation", async () => {
  const events = [];
  const liveConfig = config({
    run_id: "test-run",
    working_root: "/dev/shm/agentic-celld-orchestration/test-run",
    inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
  });
  const runtime = {
    scenarioId: "UAT-CELLD-003",
    config: liveConfig,
    orchestrationInventory: inventory(),
    providerResources: new Map(),
    workerEndpoint: "http://127.0.0.1:18080",
    fleet: { worker_vars_file_ref: "/protected/worker-vars" },
    persistInventory: (_path, document) => events.push(`persist:${document.resources.length}:${document.faults.length}:${document.faults.at(-1)?.status ?? "none"}`),
    sendWorkerCommand: async (request) => {
      events.push("provider-command");
      return { status: 202, body: { effects: [{ operation_id: request.operationId }] } };
    },
  };
  await issueCommand(runtime, {
    instanceId: "123e4567-e89b-42d3-a456-426614174000",
    generation: 1,
    operationId: "operation-1",
    action: "provision",
    payload: { runtime: "docker", name: "celld-test-provider" },
  });
  assert.deepEqual(events.slice(0, 2), ["persist:1:0:none", "provider-command"]);

  const fault = await applyPlannedOrchestrationFault(runtime, { kind: "callback_relay_pause", target: "celld-test-relay" }, async () => events.push("fault-apply"));
  const plannedIndex = events.indexOf("persist:1:1:planned");
  const applyIndex = events.indexOf("fault-apply");
  assert.ok(plannedIndex >= 0 && plannedIndex < applyIndex);
  await healPlannedOrchestrationFault(runtime, fault, async () => events.push("fault-heal"));
  assert.ok(events.indexOf("fault-heal") < events.indexOf("persist:1:1:healed"));
  assert.deepEqual(validateOrchestrationInventory(runtime.orchestrationInventory, liveConfig, { expectedHostSha256: "2".repeat(64) }), []);
});

test("provider observations use exact owned Docker and libvirt targets", () => {
  const instanceId = "123e4567-e89b-42d3-a456-426614174000";
  const dockerName = "celld-test-docker";
  const dockerCalls = [];
  const dockerRuntime = {
    providerResources: new Map([[instanceId, { instanceId, name: dockerName, substrate: "docker" }]]),
    config: { libvirt_uri: "qemu:///system" },
    runCommand: (program, args) => {
      dockerCalls.push([program, args]);
      if (args[0] === "ps") return "abcdef123456";
      if (args[0] === "inspect") return `${"a".repeat(64)}|running|sha256:${"b".repeat(64)}|${instanceId}|admin-v2`;
      throw new Error("unexpected Docker observation command");
    },
  };
  const docker = observeOrchestrationProvider(
    dockerRuntime,
    { instanceId, name: dockerName, substrate: "docker" },
    new Date("2026-08-23T00:00:00Z"),
  );
  assert.equal(docker.present, true);
  assert.equal(docker.state, "running");
  assert.match(docker.provider_identity_sha256, /^[0-9a-f]{64}$/);
  assert.deepEqual(dockerCalls.map(([program, args]) => [program, args[0]]), [["docker", "ps"], ["docker", "inspect"]]);

  const qemuName = "celld-test-qemu";
  const qemuCalls = [];
  const qemuRuntime = {
    providerResources: new Map([[instanceId, { instanceId, name: qemuName, substrate: "qemu" }]]),
    config: { libvirt_uri: "qemu:///system", vm_storage_dir: "/build/vms" },
    pathExists: () => true,
    runCommand: (program, args) => {
      qemuCalls.push([program, args]);
      if (args.includes("list")) return `${qemuName}\n`;
      if (args.includes("domuuid")) return "123e4567-e89b-42d3-a456-426614174001";
      if (args.includes("domstate")) return "shut off\n";
      if (args.includes("dumpxml")) return "<domain type='kvm'><name>celld-test-qemu</name></domain>";
      throw new Error("unexpected libvirt observation command");
    },
  };
  const qemu = observeOrchestrationProvider(
    qemuRuntime,
    { instanceId, name: qemuName, substrate: "qemu" },
    new Date("2026-08-23T00:00:01Z"),
  );
  assert.equal(qemu.present, true);
  assert.equal(qemu.state, "shut off");
  assert.equal(qemu.provider_storage_present, true);
  assert.equal(qemuCalls.every(([program]) => program === "virsh"), true);
  assert.equal(qemuCalls.some(([, args]) => args.includes("--inactive")), true);
});

test("provider observations fail closed for foreign, ambiguous, and absent targets", () => {
  const instanceId = "123e4567-e89b-42d3-a456-426614174000";
  let calls = 0;
  const runtime = {
    providerResources: new Map([[instanceId, { instanceId, name: "celld-owned", substrate: "docker" }]]),
    config: { libvirt_uri: "qemu:///system" },
    runCommand: () => { calls += 1; return ""; },
  };
  assert.throws(
    () => observeOrchestrationProvider(runtime, { instanceId, name: "celld-foreign", substrate: "docker" }),
    /not owned/,
  );
  assert.equal(calls, 0);

  const absent = observeOrchestrationProvider(runtime, { instanceId, name: "celld-owned", substrate: "docker" });
  assert.deepEqual(
    { present: absent.present, state: absent.state, storage: absent.provider_storage_present, identity: absent.provider_identity_sha256, configuration: absent.configuration_sha256 },
    { present: false, state: "absent", storage: false, identity: null, configuration: null },
  );

  runtime.runCommand = (_program, args) => args[0] === "ps" ? "abcdef123456\nfedcba654321" : "";
  assert.throws(
    () => observeOrchestrationProvider(runtime, { instanceId, name: "celld-owned", substrate: "docker" }),
    /ambiguous/,
  );
});
