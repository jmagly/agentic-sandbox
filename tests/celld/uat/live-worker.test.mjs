import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  cleanupWorkerResources,
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
    assert.match(project.sha256, /^[0-9a-f]{64}$/);
    const checked = spawnSync(process.execPath, ["--check", join(project.path, "worker.mjs")], { encoding: "utf8", shell: false });
    assert.equal(checked.status, 0, checked.stderr);
    const module = new WebAssembly.Module(readFileSync(join(project.path, "add.wasm")));
    assert.equal(new WebAssembly.Instance(module).exports.add(2, 3), 5);
    assert.equal(readFileSync(join(project.path, "assets/capability-asset.txt"), "utf8"), "agentic-celld-asset-v1\n");
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("Worker driver fixes the evaluator-owned matrices and does not claim UAT-009", () => {
  const source = readFileSync(new URL("../../../scripts/celld-live-worker.mjs", import.meta.url), "utf8");
  assert.match(source, /const ADVERTISED = \["fetch", "rpc", "storage", "alarm", "websocket", "outbound_https", "wasm", "assets"\]/);
  assert.match(source, /attempt < 100/);
  assert.match(source, /attempts: 800/);
  assert.match(source, /CELLD_PER_ISOLATE_RESOURCE_ENFORCEMENT_UNAVAILABLE/);
  assert.doesNotMatch(source, /CELLD\.009\.CONTAINMENT.*measurements/s);
});
