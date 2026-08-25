import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  cleanupWorkerResources,
  driverErrorDocument as workerDriverErrorDocument,
  excludedCapabilityRejectionEvidence,
  excludedInventoryEffects,
  executeWorkerDriver,
  prepareWorkerConformanceProject,
} from "../../../scripts/celld-live-worker.mjs";

function profile(driver, hostHash = "2".repeat(64)) {
  return {
    schema_version: "agentic-sandbox.celld-live-profile/v1", profile_id: "test-profile", run_id: "test-run",
    expected_sandbox_git: "1".repeat(40),
    environment: { kind: "disposable-local", single_host: true, host_sha256: hostHash },
    authorization: { destructive_faults: false, inventory_path: "/tmp/test-inventory.json" },
    drivers: { "celld-live-worker": driver },
  };
}

function orchestrationConfig(repoRoot) {
  return {
    schema_version: "agentic-sandbox.celld-live-orchestration/v1", run_id: "test-run",
    working_root: "/dev/shm/agentic-celld-orchestration/test-run",
    inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
    management_binary_path: `${repoRoot}/management/target/release/agentic-mgmt`,
    agent_client_binary_path: `${repoRoot}/management/target/release/agent-client`,
    callback_relay_binary_path: `${repoRoot}/tools/celld-callback-relay/target/x86_64-unknown-linux-musl/release/agentic-celld-callback-relay`,
    qemu_cleanup_helper_path: "/usr/libexec/agentic-sandbox/agentic-celld-qemu-cleanup-helper",
    qemu_cleanup_helper_sha256: "e".repeat(64),
    docker_image_ref: `sha256:${"a".repeat(64)}`, base_images_dir: "/build/agentic-sandbox/base-images",
    vm_storage_dir: "/build/agentic-sandbox/vms", agentshare_root: "/var/tmp/agentic-celld-qualification-123/mount",
    libvirt_uri: "qemu:///system", management_grpc_port: 38120,
  };
}

test("disabled live Worker returns pre-mutation NOT_RUN evidence", async () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-live-worker-test-"));
  try {
    const profilePath = join(directory, "profile.json");
    writeFileSync(profilePath, `${JSON.stringify(profile({ enabled: false, config_path: "/tmp/orchestration.json" }))}\n`, { mode: 0o600 });
    chmodSync(profilePath, 0o600);
    const observation = await executeWorkerDriver({ scenarioId: "UAT-CELLD-007", runId: "test-run", liveProfilePath: profilePath, artifactDir: join(directory, "artifacts") });
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_LIVE_WORKER_DRIVER_DISABLED");
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("Worker driver error document keeps classification while redacting message text", () => {
  const error = new Error("secret token should never appear");
  error.name = "FleetControllerSubprocessError";
  error.operation = "docker_exec_diagnose";
  error.scenarioId = "UAT-CELLD-007";
  error.errorCode = "ETIMEDOUT";
  error.exitStatus = 1;
  error.stderrSha256 = "a".repeat(64);
  error.timedOut = true;
  const document = workerDriverErrorDocument(error);
  assert.equal(document.schema_version, "agentic-sandbox.celld-live-driver-error/v1");
  assert.equal(document.name, "FleetControllerSubprocessError");
  assert.equal(document.operation, "docker_exec_diagnose");
  assert.equal(document.scenario_id, "UAT-CELLD-007");
  assert.equal(document.error_code, "ETIMEDOUT");
  assert.equal(document.exit_status, 1);
  assert.equal(document.timed_out, true);
  assert.equal(document.stderr_sha256, "a".repeat(64));
  assert.match(document.message_sha256, /^[0-9a-f]{64}$/);
  assert.equal(JSON.stringify(document).includes("secret token"), false);
});

test("UAT-009 is typed NOT_RUN before mutation while per-isolate controls are unavailable", async () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-live-worker-limit-test-"));
  try {
    const repoRoot = new URL("../../../", import.meta.url).pathname.replace(/\/$/, "");
    const configPath = join(directory, "orchestration.json"), profilePath = join(directory, "profile.json");
    const host = "synthetic-titan";
    writeFileSync(configPath, `${JSON.stringify(orchestrationConfig(repoRoot))}\n`, { mode: 0o600 });
    writeFileSync(profilePath, `${JSON.stringify(profile({ enabled: true, config_path: configPath }, createHash("sha256").update(host).digest("hex")))}\n`, { mode: 0o600 });
    chmodSync(configPath, 0o600); chmodSync(profilePath, 0o600);
    const observation = await executeWorkerDriver(
      { scenarioId: "UAT-CELLD-009", runId: "test-run", liveProfilePath: profilePath, artifactDir: join(directory, "artifacts") },
      { gitCommit: () => "1".repeat(40), hostname: () => host },
    );
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_PER_ISOLATE_RESOURCE_ENFORCEMENT_UNAVAILABLE");
    assert.deepEqual(observation.assertions, []);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("Worker deployer cleanup removes only an exact-run labeled container", () => {
  const calls = [], labels = { "dev.agentic-sandbox.run": "test-run", "dev.agentic-sandbox.scope": "celld-qualification" };
  const runner = (program, args) => {
    calls.push([program, ...args]);
    if (args[0] === "info") return "27.0.0";
    if (args[0] === "container") return JSON.stringify([{ Config: { Labels: labels } }]);
    return "";
  };
  const result = cleanupWorkerResources("test-run", { runner });
  assert.equal(result.status, "PASS");
  assert.equal(result.removed.length, 1);
  assert.deepEqual(calls.at(-1).slice(0, 4), ["docker", "rm", "--force", "--volumes"]);
});

test("fixed Worker conformance project has valid JavaScript and a working Wasm module", () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-worker-conformance-test-"));
  try {
    const project = prepareWorkerConformanceProject(directory);
    assert.match(project.sha256, /^sha256:[0-9a-f]{64}$/);
    const checked = spawnSync(process.execPath, ["--check", join(project.path, "worker.mjs")], { encoding: "utf8", shell: false });
    assert.equal(checked.status, 0, checked.stderr);
    const module = new WebAssembly.Module(readFileSync(join(project.path, "add.wasm")));
    assert.equal(new WebAssembly.Instance(module).exports.add(2, 3), 5);
    assert.equal(readFileSync(join(project.path, "assets/capability-asset.txt"), "utf8"), "agentic-celld-asset-v1\n");
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("excluded capability attempts retain bounded typed raw evidence", () => {
  const record = excludedCapabilityRejectionEvidence(
    { status: 2, signal: null, stdout: "", stderr: "error: does not support these config keys: process" },
    "process",
    7,
    "2026-08-23T09:00:00.000Z",
    "2026-08-23T09:00:00.010Z",
  );
  assert.equal(record.rejection_code, "celld.unsupported_config_key");
  assert.equal(record.typed_rejection, true);
  assert.equal(record.attempt, 7);
  assert.match(record.output_sha256, /^sha256:[0-9a-f]{64}$/);
  assert.throws(
    () => excludedCapabilityRejectionEvidence({ status: 2, stderr: "x".repeat(4097) }, "process", 0, "before", "after"),
    /evidence bound/,
  );
});

test("excluded capability inventory reports each host effect independently", () => {
  const before = { containers: ["a"], processes_sha256: "p", sockets_sha256: "s", domains: ["v"], files_sha256: "f" };
  assert.deepEqual(excludedInventoryEffects(before, structuredClone(before)), {
    processes_created: 0, files_created: 0, sockets_created: 0, containers_created: 0, vms_created: 0,
  });
  assert.deepEqual(excludedInventoryEffects(before, { ...before, sockets_sha256: "changed", domains: ["v", "new"] }), {
    processes_created: 0, files_created: 0, sockets_created: 1, containers_created: 0, vms_created: 1,
  });
});

test("Worker driver fixes the evaluator-owned matrices and does not claim UAT-009", () => {
  const source = readFileSync(new URL("../../../scripts/celld-live-worker.mjs", import.meta.url), "utf8");
  assert.match(source, /const ADVERTISED = \["fetch", "rpc", "storage", "alarm", "websocket", "outbound_https", "wasm", "assets"\]/);
  assert.match(source, /attempt < 100/);
  assert.match(source, /attempts: 800/);
  assert.match(source, /attempt_records: attempts/);
  assert.match(source, /markerBefore\.status !== 409/);
  assert.match(source, /openFleetWorkerAccess/);
  assert.match(source, /withWorkerAccess\(operation\("worker-endpoint"\)/);
  assert.match(source, /operation\("deploy-candidate"\)/);
  assert.match(source, /operation\("capability-cases"\)/);
  assert.match(source, /operation\("reset-negative-projects"\)/);
  assert.match(source, /CELLD_LIVE_WORKER_RESET_NEGATIVE_PROJECTS_FAILED/);
  assert.match(source, /operation\("copy-negative-projects"\)/);
  assert.match(source, /\$\{negativeRoot\}\/\./);
  assert.match(source, /\$\{primary\}:\/tmp\/celld-negative\//);
  assert.match(source, /CELLD_LIVE_WORKER_COPY_NEGATIVE_PROJECTS_FAILED/);
  assert.match(source, /operation\(`dry-run-\$\{capability\}`\)/);
  assert.doesNotMatch(source, /proofHash/);
  assert.doesNotMatch(source, /AWS_SHARED_CREDENTIALS_FILE/);
  assert.match(source, /CELLD_PER_ISOLATE_RESOURCE_ENFORCEMENT_UNAVAILABLE/);
  assert.doesNotMatch(source, /CELLD\.009\.CONTAINMENT.*measurements/s);
});

test("unavailable per-isolate controls are absent from the advertised capability surface", () => {
  const root = new URL("../../../", import.meta.url);
  const bundle = JSON.parse(readFileSync(new URL("runtimes/celld/instance-cell/bundle.json", root), "utf8"));
  assert.deepEqual(bundle.capabilities, [
    "worker.fetch", "worker.rpc", "durable.storage", "durable.alarm", "websocket.inbound", "network.outbound.fetch", "wasm.module", "assets.static",
  ]);
  assert.ok(bundle.capabilities.every((capability) => !/cpu|memory|rate|storage_limit|resident|outbound_limit/.test(capability)));
});
