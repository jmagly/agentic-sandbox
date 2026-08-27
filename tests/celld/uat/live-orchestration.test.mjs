import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, linkSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, rmdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { hostname, tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import * as liveOrchestration from "../../../scripts/celld-live-orchestration.mjs";

import {
  applyPlannedOrchestrationFault,
  celldInstanceCellScope,
  clearDispatchGate,
  durableEffectHistoryObservation,
  executeOrchestrationDriver,
  healPlannedOrchestrationFault,
  issueCommand,
  launchManagement,
  loadAuthorizedOrchestrationInventory,
  loadProtectedOrchestrationRuntime,
  managementCelldVersion,
  managementEnvironment,
  observeCelldOwnership,
  observeOrchestrationProvider,
  prepareDispatchGate,
  requestHash,
  signalExactCallbackResponseLoss,
  validateOrchestrationConfig,
  validateOrchestrationInventory,
  waitDispatchGate,
} from "../../../scripts/celld-live-orchestration.mjs";

const orchestrationSource = readFileSync(new URL("../../../scripts/celld-live-orchestration.mjs", import.meta.url), "utf8");

test("management canonicalizes the reviewed image version to the qualified API pin", () => {
  assert.equal(managementCelldVersion("0.2.1"), "v0.2.1");
  assert.equal(managementCelldVersion("v0.2.1"), "v0.2.1");
  assert.throws(() => managementCelldVersion("latest"), /version is invalid/);
});

test("response-loss evidence comes from the exact durable unknown event", () => {
  const operationId = "uat005-qemu-1-start-0";
  const event = {
    document_type: "instance-cell-event", schema_version: "1", event_id: "123e4567-e89b-42d3-a456-426614174000",
    instance_id: "8be6be9f-eeb9-4a85-983b-3706a7e17400", operation_id: operationId, generation: 1,
    sequence: 3, kind: "effect_unknown", recorded_at: "2026-08-26T01:38:34.000Z", evidence: { attempts: 1 },
  };
  const observation = durableEffectHistoryObservation({ history: [event] }, operationId, "effect_unknown");
  assert.deepEqual({ ...observation, sha256: undefined }, {
    operation_id: operationId, kind: "effect_unknown", sequence: 3, count: 1, sha256: undefined,
  });
  assert.match(observation.sha256, /^[0-9a-f]{64}$/);
  assert.throws(
    () => durableEffectHistoryObservation({ history: [] }, operationId, "effect_unknown"),
    (error) => error.errorCode === "CELLD_DURABLE_EFFECT_EVENT_MISSING" && /^[0-9a-f]{64}$/.test(error.evidenceSha256),
  );
  assert.throws(
    () => durableEffectHistoryObservation({ history: [event, { ...event, sequence: 4 }] }, operationId, "effect_unknown"),
    (error) => error.errorCode === "CELLD_DURABLE_EFFECT_EVENT_DUPLICATE" && /^[0-9a-f]{64}$/.test(error.evidenceSha256),
  );
  assert.doesNotMatch(orchestrationSource, /waitCellEffect\([^\n]+\["unknown"\]\)/);
  assert.match(orchestrationSource, /getWorkerOperation\(/);
});

test("recovery and response-loss campaigns expose their exact bounded failure phases", () => {
  assert.match(orchestrationSource, /orchestration\.uat004\.\$\{recoveryPhase\}/);
  assert.match(orchestrationSource, /latestDiagnosis\?\.failure\?\.reason_code/);
  assert.match(orchestrationSource, /latestDiagnosis\?\.failure\?\.evidence_sha256/);
  assert.match(orchestrationSource, /campaignError = annotateDriverError\(error/);
  assert.match(orchestrationSource, /CELLD_RECOVERY_CLEANUP_/);
  assert.match(orchestrationSource, /campaignError,/);
  assert.match(orchestrationSource, /orchestration\.uat005\.\$\{substrate\}-\$\{action\}-\$\{responseLossPhase\}/);
  for (const phase of ["response-loss-arm", "issue-command", "worker-terminal", "durable-unknown-observation", "management-replay", "provider-after", "fault-heal"]) {
    assert.match(orchestrationSource, new RegExp(`responseLossPhase = "${phase}"`));
  }
});

test("response-loss injection waits until the exact relay acknowledges arming", async () => {
  const calls = [];
  let armed = 0;
  const providerId = "a".repeat(64);
  const binding = {
    owned: true,
    provider_id: providerId,
    target_identity_sha256: createHash("sha256").update(`docker:${providerId}`).digest("hex"),
    target_ownership_sha256: "b".repeat(64),
  };
  const runtime = {
    runCommand(program, args) {
      calls.push([program, ...args]);
      if (args[0] === "logs") return "Celld callback relay armed one response loss\n".repeat(armed);
      if (args[0] === "kill") armed += 1;
      return "";
    },
  };
  await signalExactCallbackResponseLoss(runtime, binding);
  await signalExactCallbackResponseLoss(runtime, binding);
  assert.equal(calls[0][1], "logs");
  assert.deepEqual(calls[0].slice(2, 4), ["--tail", "8192"]);
  assert.equal(calls[1][1], "kill");
  assert.deepEqual(calls[1].slice(2, 4), ["--signal", "SIGUSR1"]);
  assert.equal(calls[1][4], providerId);
  assert.equal(calls[2][1], "logs");
  assert.equal(calls[2].at(-1), providerId);
  assert.equal(armed, 2, "a stale first acknowledgement must not satisfy the second arm");
  assert.equal(runtime.callbackResponseLossBaselines.get(providerId).armed, 2);
});

test("response-loss observation requires an injected marker newer than the exact arm baseline", async () => {
  const providerId = "c".repeat(64);
  const target = "celld-test-node-1-callback-relay";
  const labels = {
    "dev.agentic-sandbox.repository": "roctinam/agentic-sandbox",
    "dev.agentic-sandbox.workflow": "celld-qualification",
    "dev.agentic-sandbox.run": "test-run",
    "dev.agentic-sandbox.scope": "celld-qualification",
  };
  let injected = 1;
  const runtime = {
    runId: "test-run",
    fleet: { nodes: [{ name: "celld-test-node-1" }] },
    callbackResponseLossBaselines: new Map([[providerId, { armed: 1, injected: 1 }]]),
    runCommand(_program, args) {
      if (args[0] === "inspect") return `${providerId}|${JSON.stringify(labels)}`;
      if (args[0] === "logs") return "Celld callback relay injected one response loss\n".repeat(injected);
      throw new Error(`unexpected Docker command ${args[0]}`);
    },
  };
  const subject = {
    fault_id: "a".repeat(32),
    kind: "callback_response_loss",
    target,
  };
  const plan = { mutation: "fault_apply", recorded_at: new Date().toISOString(), subject };
  const stale = await liveOrchestration.observeOrchestrationFaultTarget({ runtime, plan, fault: subject });
  assert.equal(stale.present, false);
  injected = 2;
  const current = await liveOrchestration.observeOrchestrationFaultTarget({ runtime, plan, fault: subject });
  assert.equal(current.present, true);
  assert.equal(current.provider_id, providerId);
});

function config(overrides = {}) {
  return {
    schema_version: "agentic-sandbox.celld-live-orchestration/v1",
    run_id: "titan-123",
    working_root: "/dev/shm/agentic-celld-orchestration/titan-123",
    inventory_path: "/dev/shm/agentic-celld-orchestration/titan-123/orchestration-inventory.json",
    management_binary_path: "/repo/.celld-target/release/agentic-mgmt",
    agent_client_binary_path: "/repo/.celld-target/release/agent-client",
    callback_relay_binary_path: "/repo/tools/celld-callback-relay/target/x86_64-unknown-linux-musl/release/agentic-celld-callback-relay",
    qemu_cleanup_helper_path: "/usr/libexec/agentic-sandbox/agentic-celld-qemu-cleanup-helper",
    qemu_cleanup_helper_sha256: "e".repeat(64),
    docker_image_ref: `sha256:${"a".repeat(64)}`,
    base_images_dir: "/build/agentic-sandbox/base-images",
    vm_storage_dir: "/build/agentic-sandbox/vms",
    agentshare_root: "/var/tmp/agentic-celld-qualification-123/mount",
    libvirt_uri: "qemu:///system",
    management_grpc_port: 38120,
    ...overrides,
  };
}

test("orchestration driver error document exposes safe phase fields only", () => {
  const error = new Error("secret callback bearer token should not be logged");
  error.name = "OrchestrationPhaseError";
  error.operation = "orchestration.run-uat-celld-003";
  error.scenarioId = "UAT-CELLD-003";
  error.exitCode = 4;
  const document = liveOrchestration.driverErrorDocument(error);
  assert.equal(document.schema_version, "agentic-sandbox.celld-live-driver-error/v1");
  assert.equal(document.name, "OrchestrationPhaseError");
  assert.equal(document.operation, "orchestration.run-uat-celld-003");
  assert.equal(document.scenario_id, "UAT-CELLD-003");
  assert.equal(document.exit_code, 4);
  assert.match(document.message_sha256, /^[0-9a-f]{64}$/);
  assert.equal(JSON.stringify(document).includes("bearer token"), false);
});

test("cleanup precedence retains a safe digest of the masked campaign failure", () => {
  const cleanup = new Error("storage cleanup failed");
  cleanup.exitCode = 4;
  cleanup.operation = "orchestration.cleanup-storage";
  cleanup.campaignError = Object.assign(new Error("private campaign detail"), {
    operation: "orchestration.run-uat-celld-004",
    errorCode: "CELLD_RESTART_FAILED",
  });
  const document = liveOrchestration.driverErrorDocument(cleanup);
  assert.equal(document.campaign_operation, "orchestration.run-uat-celld-004");
  assert.equal(document.campaign_error_code, "CELLD_RESTART_FAILED");
  assert.equal(document.campaign_message_sha256, createHash("sha256").update("private campaign detail").digest("hex"));
  assert.equal(JSON.stringify(document).includes("private campaign detail"), false);
});

test("orchestration config confines exact-run mutation targets and immutable inputs", () => {
  assert.deepEqual(validateOrchestrationConfig(config(), { repoRoot: "/repo" }), []);
  assert.match(validateOrchestrationConfig(config({ working_root: "/tmp/titan-123" }), { repoRoot: "/repo" }).join(";"), /below \/dev\/shm/);
  assert.match(validateOrchestrationConfig(config({ inventory_path: "/dev/shm/another-run/orchestration-inventory.json" }), { repoRoot: "/repo" }).join(";"), /fixed working-root file/);
  assert.match(validateOrchestrationConfig(config({ docker_image_ref: "latest" }), { repoRoot: "/repo" }).join(";"), /immutable local OCI image ID/);
  assert.match(validateOrchestrationConfig(config({ management_binary_path: "/tmp/agentic-mgmt" }), { repoRoot: "/repo" }).join(";"), /approved build target/);
  assert.match(validateOrchestrationConfig(config({ libvirt_uri: "qemu:///session" }), { repoRoot: "/repo" }).join(";"), /qemu:\/\/\/system/);
});

test("QEMU cleanup helper verification permits Cargo build hardlinks but keeps installed helper single-linked", () => {
  const verifier = orchestrationSource.match(/export function verifyQemuCleanupHelperInstallation\(config\) \{[\s\S]*?\n\}/)?.[0] ?? "";
  assert.notEqual(verifier, "", "missing QEMU cleanup helper verifier");
  assert.match(verifier, /built\.isFile\(\)/);
  assert.match(verifier, /built\.isSymbolicLink\(\)/);
  assert.match(verifier, /built\.mode & 0o111/);
  assert.doesNotMatch(verifier, /built\.nlink\s*!==\s*1/);
  assert.match(verifier, /installed\.nlink\s*!==\s*1/);
  assert.match(verifier, /installed\.uid !== 0/);
  assert.match(verifier, /installed\.gid !== 0/);
  assert.match(verifier, /\(installed\.mode & 0o777\) !== 0o755/);
});

test("callback request hash matches the Rust and Worker canonical contract", () => {
  assert.equal(requestHash({
    operationId: "op-1", instanceId: "instance-a", generation: 1, action: "provision",
    payload: { name: "instance-a", runtime: "docker" },
  }), "1115e4f5a1657ff842d76a9798214266a4954fcb7985e80ddd473ecfac24fd0b");
});

test("management transport can be pinned to the private Celld mTLS proxy", () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-management-transport-test-"));
  try {
    const workerVars = join(directory, "worker-vars");
    writeFileSync(workerVars, "CELL_AUTH_KEY_ID=run-key\nCELL_AUTH_KEY=this-is-a-long-enough-qualification-key\n", { mode: 0o600 });
    const fleet = {
      run_root: directory,
      worker_vars_file_ref: workerVars,
      nodes: [{ name: "celld-fleet-node-1" }],
      callback: {
        management_server_cert_file_ref: join(directory, "management.crt"), management_server_key_file_ref: join(directory, "management.key"),
        ca_file_ref: join(directory, "ca.crt"), management_auth_key_file_ref: join(directory, "management-auth-key"),
        effect_ledger_file_ref: join(directory, "effect-ledger.sqlite"), client_cn: "agentic-celld-worker-callback",
      },
      pins: { celld: { version: "v0.2.1", commit: "1".repeat(40) } },
    };
    const liveConfig = config({ management_grpc_port: 38120 });
    const tlsCa = join(directory, "network-ca.crt"), tlsIdentity = join(directory, "management-client.pem");
    const environment = managementEnvironment(liveConfig, fleet, "172.30.0.1", {
      celldEndpoint: "https://172.30.0.20:8443", tlsCaFile: tlsCa, tlsIdentityFile: tlsIdentity,
      operatorMtlsCn: "agentic-celld-management",
    });
    assert.equal(environment.AGENTIC_CELLD_ENDPOINT, "https://172.30.0.20:8443");
    assert.equal(environment.AGENTIC_CELLD_TLS_CA_FILE, tlsCa);
    assert.equal(environment.AGENTIC_CELLD_TLS_CLIENT_IDENTITY_FILE, tlsIdentity);
    assert.equal(environment.AIWG_MTLS_ADMIN_ALLOWLIST, "agentic-celld-management");
    assert.equal(environment.AGENTIC_GRPC_UDS.startsWith(`${liveConfig.working_root}/grpc-`), true);
    assert.equal(Buffer.byteLength(environment.AGENTIC_GRPC_UDS) < 108, true);
    assert.equal(environment.AGENTIC_GRPC_UDS.includes(directory), false);
    assert.equal(
      managementEnvironment(liveConfig, fleet, "172.30.0.1", {
        celldEndpoint: "https://172.30.0.20:8443", tlsCaFile: tlsCa, tlsIdentityFile: tlsIdentity,
        operatorMtlsCn: "agentic-celld-management",
      }).AGENTIC_GRPC_UDS,
      environment.AGENTIC_GRPC_UDS,
    );
    const qualificationKeyRoot = join(directory, "management-state", "secrets", "ssh-keys");
    const keyRootMetadata = lstatSync(qualificationKeyRoot);
    assert.equal(keyRootMetadata.isDirectory(), true);
    assert.equal(keyRootMetadata.isSymbolicLink(), false);
    assert.equal(keyRootMetadata.uid, process.getuid());
    assert.equal(keyRootMetadata.gid, process.getgid());
    assert.equal(keyRootMetadata.mode & 0o777, 0o700);
    assert.doesNotThrow(() => managementEnvironment(liveConfig, fleet, "172.30.0.1", {
      celldEndpoint: "https://172.30.0.20:8443", tlsCaFile: tlsCa, tlsIdentityFile: tlsIdentity,
      operatorMtlsCn: "agentic-celld-management",
    }));
    assert.throws(() => managementEnvironment(liveConfig, fleet, "172.30.0.1", { tlsCaFile: tlsCa }), /requires both/);
    assert.throws(
      () => managementEnvironment(liveConfig, fleet, "172.30.0.1", { operatorMtlsCn: "agentic-celld-management" }),
      /requires a bounded CN and private TLS identity/,
    );
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("management launch annotates protected environment failures", () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-management-launch-env-test-"));
  try {
    const fleet = {
      run_root: directory,
      worker_vars_file_ref: join(directory, "missing-worker-vars"),
      callback: {
        management_server_cert_file_ref: join(directory, "management.crt"), management_server_key_file_ref: join(directory, "management.key"),
        ca_file_ref: join(directory, "ca.crt"), management_auth_key_file_ref: join(directory, "management-auth-key"),
        effect_ledger_file_ref: join(directory, "effect-ledger.sqlite"), client_cn: "agentic-celld-worker-callback",
      },
      pins: { celld: { version: "v0.2.1", commit: "1".repeat(40) } },
    };
    let failure;
    assert.throws(
      () => launchManagement(config({ management_binary_path: join(directory, "agentic-mgmt") }), fleet, "127.0.0.1", { celldEndpoint: "http://127.0.0.1:18080" }),
      (error) => {
        failure = error;
        return true;
      },
    );
    assert.equal(failure.operation, "orchestration.launch-management.environment.worker-key");
    assert.equal(failure.errorCode, "CELLD_MANAGEMENT_WORKER_KEY_INVALID");
    assert.equal(failure.code, "ENOENT");
    const document = liveOrchestration.driverErrorDocument(failure);
    assert.equal(document.operation, "orchestration.launch-management.environment.worker-key");
    assert.equal(document.error_code, "CELLD_MANAGEMENT_WORKER_KEY_INVALID");
    assert.equal(document.node_code, "ENOENT");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("management environment reports worker endpoint failures before state mutation", () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-management-endpoint-test-"));
  try {
    const workerVars = join(directory, "worker-vars");
    writeFileSync(workerVars, "CELL_AUTH_KEY_ID=run-key\nCELL_AUTH_KEY=this-is-a-long-enough-qualification-key\n", { mode: 0o600 });
    const fleet = {
      run_root: directory,
      worker_vars_file_ref: workerVars,
      nodes: [{ name: "celld-fleet-node-1" }],
      callback: {
        management_server_cert_file_ref: join(directory, "management.crt"), management_server_key_file_ref: join(directory, "management.key"),
        ca_file_ref: join(directory, "ca.crt"), management_auth_key_file_ref: join(directory, "management-auth-key"),
        effect_ledger_file_ref: join(directory, "effect-ledger.sqlite"), client_cn: "agentic-celld-worker-callback",
      },
      pins: { celld: { version: "v0.2.1", commit: "1".repeat(40) } },
    };
    let failure;
    assert.throws(
      () => managementEnvironment(config({ management_binary_path: join(directory, "agentic-mgmt") }), fleet, "127.0.0.1"),
      (error) => {
        failure = error;
        return true;
      },
    );
    assert.equal(failure.operation, "orchestration.launch-management.environment.worker-endpoint");
    assert.equal(failure.errorCode, "CELLD_MANAGEMENT_WORKER_ENDPOINT_INVALID");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("orchestration driver uses fixture-managed Worker access for live management transport", () => {
  assert.match(orchestrationSource, /openFleetWorkerAccess\(fleetPath\)/);
  assert.match(orchestrationSource, /celldEndpoint: workerAccess\.endpoint/);
  assert.match(orchestrationSource, /workerEndpoint: workerAccess\.endpoint/);
  assert.match(orchestrationSource, /for \(const access of workerAccesses\)/);
});

test("owner recovery replaces and closes exact fixture-managed Worker access", async () => {
  assert.match(orchestrationSource, /await replaceFleetWorkerAccess\(runtime, fallbackIndex\)/);
  assert.doesNotMatch(orchestrationSource, /runtime\.workerEndpoint = workerEndpoint\(/);
  const opened = [];
  const closed = [];
  const initial = { endpoint: "http://127.0.0.1:18080", close: async () => closed.push("initial") };
  const runtime = {
    fleetPath: "/run/celld/fleet.json",
    fleet: { nodes: [{ name: "node-1" }, { name: "node-2" }] },
    workerAccess: initial,
    workerAccesses: new Set([initial]),
    workerEndpoint: initial.endpoint,
    openFleetWorkerAccess: async (path, { nodeIndex }) => {
      opened.push([path, nodeIndex]);
      return { endpoint: "http://127.0.0.1:28080", node: "node-2", close: async () => closed.push("replacement") };
    },
  };
  const replacement = await liveOrchestration.replaceFleetWorkerAccess(runtime, 1);
  assert.deepEqual(opened, [[runtime.fleetPath, 1]]);
  assert.deepEqual(closed, ["initial"]);
  assert.equal(runtime.workerAccess, replacement);
  assert.equal(runtime.workerEndpoint, replacement.endpoint);
  assert.deepEqual([...runtime.workerAccesses], [replacement]);
});

test("owner recovery rejects non-loopback replacement access and closes it", async () => {
  let closed = 0;
  const initial = { endpoint: "http://127.0.0.1:18080", close: async () => {} };
  const runtime = {
    fleetPath: "/run/celld/fleet.json",
    fleet: { nodes: [{ name: "node-1" }, { name: "node-2" }] },
    workerAccess: initial,
    workerEndpoint: initial.endpoint,
    openFleetWorkerAccess: async () => ({ endpoint: "http://192.0.2.10:8080", node: "node-2", close: async () => { closed += 1; } }),
  };
  await assert.rejects(liveOrchestration.replaceFleetWorkerAccess(runtime, 1), /host-loopback access/);
  assert.equal(closed, 1);
  assert.equal(runtime.workerAccess, initial);
  assert.equal(runtime.workerEndpoint, initial.endpoint);
});

test("owner recovery retains a replacement whose invalid-access cleanup fails", async () => {
  const initial = { endpoint: "http://127.0.0.1:18080", close: async () => {} };
  const invalid = {
    endpoint: "http://127.0.0.1:28080",
    node: "substituted-node",
    close: async () => { throw new Error("listener still active"); },
  };
  const runtime = {
    fleetPath: "/run/celld/fleet.json",
    fleet: { nodes: [{ name: "node-1" }, { name: "node-2" }] },
    workerAccess: initial,
    workerEndpoint: initial.endpoint,
    openFleetWorkerAccess: async () => invalid,
  };
  await assert.rejects(
    liveOrchestration.replaceFleetWorkerAccess(runtime, 1),
    (error) => error.exitCode === 4 && /cleanup failed/.test(error.message),
  );
  assert.equal(runtime.workerAccesses.has(invalid), true);
  assert.equal(runtime.workerAccess, initial);
  assert.equal(runtime.workerEndpoint, initial.endpoint);
});

test("management restart preserves the exact fixture-managed Worker transport", () => {
  assert.match(orchestrationSource, /celldTransport:\s*\{[\s\S]*?celldEndpoint:\s*environment\.AGENTIC_CELLD_ENDPOINT/);
  assert.match(orchestrationSource, /launchManagement\(config, fleet, managementHost, \{ \.\.\.management\.celldTransport, celldEndpoint \}\)/);
  assert.match(orchestrationSource, /restartManagement\(runtime\.management, runtime\.config, runtime\.fleet, runtime\.managementHost, runtime\.workerEndpoint\)/);
});

test("management launch annotates spawn failures before readiness polling", () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-management-launch-spawn-test-"));
  try {
    const workerVars = join(directory, "worker-vars");
    writeFileSync(workerVars, "CELL_AUTH_KEY_ID=run-key\nCELL_AUTH_KEY=this-is-a-long-enough-qualification-key\n", { mode: 0o600 });
    const fleet = {
      run_root: directory,
      worker_vars_file_ref: workerVars,
      callback: {
        management_server_cert_file_ref: join(directory, "management.crt"), management_server_key_file_ref: join(directory, "management.key"),
        ca_file_ref: join(directory, "ca.crt"), management_auth_key_file_ref: join(directory, "management-auth-key"),
        effect_ledger_file_ref: join(directory, "effect-ledger.sqlite"), client_cn: "agentic-celld-worker-callback",
      },
      pins: { celld: { version: "v0.2.1", commit: "1".repeat(40) } },
    };
    let failure;
    assert.throws(
      () => launchManagement(config({ management_binary_path: join(directory, "missing-agentic-mgmt") }), fleet, "127.0.0.1", { celldEndpoint: "http://127.0.0.1:18080" }),
      (error) => {
        failure = error;
        return true;
      },
    );
    assert.equal(failure.operation, "orchestration.launch-management.spawn");
    assert.equal(failure.errorCode, "CELLD_MANAGEMENT_SPAWN_NO_PID");
    const document = liveOrchestration.driverErrorDocument(failure);
    assert.equal(document.operation, "orchestration.launch-management.spawn");
    assert.equal(document.error_code, "CELLD_MANAGEMENT_SPAWN_NO_PID");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("qualification dispatch gates bind one exact operation, phase, and management pid", async () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-dispatch-gate-test-"));
  try {
    const root = join(directory, "management-state", "dispatch-gates");
    mkdirSync(root, { recursive: true, mode: 0o700 });
    chmodSync(root, 0o700);
    const runtime = { fleet: { run_root: directory } };
    const id = "uat004-docker-1-during_dispatch-0";
    const operationDigest = createHash("sha256").update(id).digest("hex");
    const requestPath = join(root, `${operationDigest}.request.json`);
    const reachedPath = join(root, `${operationDigest}.reached.json`);
    prepareDispatchGate(runtime, id, "during_dispatch");
    assert.equal(existsSync(requestPath), true);
    writeFileSync(reachedPath, `${JSON.stringify({
      schema_version: "agentic-sandbox.celld-dispatch-gate/v1",
      operation_id_sha256: operationDigest,
      phase: "during_dispatch",
      management_pid: 4242,
      reached_at: "2026-08-23T00:00:00.000Z",
    })}\n`, { mode: 0o600 });
    const reached = await waitDispatchGate(runtime, id, "during_dispatch", 4242);
    assert.equal(reached.operation_id_sha256, operationDigest);
    await assert.rejects(() => waitDispatchGate(runtime, id, "after_dispatch", 4242), /exact phase and management process/);
    clearDispatchGate(runtime, id);
    assert.equal(existsSync(requestPath), false);
    assert.equal(existsSync(reachedPath), false);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
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

function canonicalJson(value) {
  if (value === null || typeof value === "boolean" || typeof value === "string") return JSON.stringify(value);
  if (typeof value === "number") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
}

function appendV2JournalEntry(document, entry, recordedAt) {
  const base = {
    sequence: document.journal.length + 1,
    ...entry,
    recorded_at: recordedAt,
    previous_entry_sha256: document.journal.at(-1)?.entry_sha256 ?? null,
  };
  const record = {
    ...base,
    entry_sha256: createHash("sha256").update(canonicalJson(base)).digest("hex"),
  };
  document.journal.push(record);
  document.last_sequence = record.sequence;
  document.journal_head_sha256 = record.entry_sha256;
  document.updated_at = recordedAt;
  const terminal = new Set(document.journal.filter((candidate) => candidate.event !== "planned").map((candidate) => candidate.mutation_id));
  document.incomplete_mutation_ids = document.journal
    .filter((candidate) => candidate.event === "planned" && !terminal.has(candidate.mutation_id))
    .map((candidate) => candidate.mutation_id);
  return record;
}

function providerIntent(action = "provision") {
  return {
    instance_id: "123e4567-e89b-42d3-a456-426614174000",
    name: "celld-test-provider",
    substrate: "docker",
    operation_id: `operation-${action}`,
    generation: 1,
    action,
    request_sha256: "a".repeat(64),
  };
}

function exactDockerTerminalObservation({ present = true, state = "created" } = {}) {
  if (!present) return { owned: true, present: false, state: "absent", provider_storage_present: false, provider_identity_sha256: null, configuration_sha256: null };
  return {
    owned: true,
    present: true,
    state,
    provider_storage_present: false,
    provider_id: "1".repeat(64),
    provider_identity_sha256: "c".repeat(64),
    configuration_sha256: "d".repeat(64),
    ownership_binding_sha256: "2".repeat(64),
    managed_network_id: "3".repeat(64),
    managed_network_identity_sha256: "4".repeat(64),
    managed_network_configuration_sha256: "5".repeat(64),
  };
}

function exactFaultBinding() {
  return {
    owned: true,
    provider_id: "1".repeat(64),
    target_identity_sha256: "2".repeat(64),
    target_ownership_sha256: "3".repeat(64),
  };
}

function inventoryV2({ includeProvision = true, completedProvision = false } = {}) {
  const createdAt = "2026-08-23T00:00:00.000Z";
  const document = {
    schema_version: "agentic-sandbox.celld-orchestration-inventory/v2",
    run_id: "test-run",
    working_root: "/dev/shm/agentic-celld-orchestration/test-run",
    owner: {
      repository: "roctinam/agentic-sandbox",
      workflow: "celld-qualification.yml",
      run_id: "test-run",
      uid: typeof process.getuid === "function" ? process.getuid() : 0,
      gid: typeof process.getgid === "function" ? process.getgid() : 0,
    },
    host_sha256: "2".repeat(64),
    created_at: createdAt,
    updated_at: createdAt,
    state: "active",
    last_sequence: 0,
    journal_head_sha256: null,
    incomplete_mutation_ids: [],
    resources: [],
    faults: [],
    journal: [],
  };
  if (!includeProvision) {
    document.state = "prepared";
    return document;
  }
  const mutationId = "123e4567-e89b-42d3-a456-426614174111";
  const subject = providerIntent();
  const plan = appendV2JournalEntry(document, {
    mutation_id: mutationId,
    event: "planned",
    mutation: "provider_action",
    scenario_id: "UAT-CELLD-003",
    subject_type: "provider_resource",
    subject,
  }, createdAt);
  document.resources.push({
    scenario_id: "UAT-CELLD-003",
    instance_id: subject.instance_id,
    name: subject.name,
    substrate: subject.substrate,
    status: completedProvision ? "active" : "planned",
    last_sequence: plan.sequence,
    planned_at: createdAt,
    updated_at: createdAt,
  });
  if (completedProvision) {
    const completedAt = "2026-08-23T00:00:01.000Z";
    const observation = exactDockerTerminalObservation();
    const completion = appendV2JournalEntry(document, {
      mutation_id: mutationId,
      event: "completed",
      mutation: "provider_action",
      scenario_id: "UAT-CELLD-003",
      subject_type: "provider_resource",
      subject,
      plan_sequence: plan.sequence,
      outcome: "effect_observed",
      observed_provider_id: observation.provider_id,
      observed_identity_sha256: observation.provider_identity_sha256,
      observed_configuration_sha256: observation.configuration_sha256,
      observed_ownership_binding_sha256: observation.ownership_binding_sha256,
      observed_managed_network_id: observation.managed_network_id,
      observed_managed_network_identity_sha256: observation.managed_network_identity_sha256,
      observed_managed_network_configuration_sha256: observation.managed_network_configuration_sha256,
    }, completedAt);
    document.resources[0].last_sequence = completion.sequence;
    document.resources[0].updated_at = completedAt;
    document.resources[0].provider_id = observation.provider_id;
    document.resources[0].provider_identity_sha256 = observation.provider_identity_sha256;
    document.resources[0].configuration_sha256 = observation.configuration_sha256;
    document.resources[0].ownership_binding_sha256 = observation.ownership_binding_sha256;
    document.resources[0].managed_network_id = observation.managed_network_id;
    document.resources[0].managed_network_identity_sha256 = observation.managed_network_identity_sha256;
    document.resources[0].managed_network_configuration_sha256 = observation.managed_network_configuration_sha256;
  }
  return document;
}

function exactOrchestrationTestRoot(prefix) {
  const parent = "/dev/shm/agentic-celld-orchestration";
  const removeParent = !existsSync(parent);
  mkdirSync(parent, { recursive: true, mode: 0o700 });
  const outer = mkdtempSync(join(parent, `${prefix}-`));
  const root = join(outer, "test-run");
  mkdirSync(root, { mode: 0o700 });
  return {
    outer,
    root,
    cleanup() {
      rmSync(outer, { recursive: true, force: true });
      if (removeParent) rmdirSync(parent);
    },
  };
}

test("orchestration inventory v2 contract exposes exact typed mutation journal fields", () => {
  const schema = JSON.parse(readFileSync(new URL("./orchestration-inventory-v2.schema.json", import.meta.url), "utf8"));
  assert.equal(schema.$id, "https://agentic-sandbox.dev/schemas/celld-orchestration-inventory/v2");
  assert.equal(schema.properties.schema_version.const, "agentic-sandbox.celld-orchestration-inventory/v2");
  assert.deepEqual(
    schema.$defs.journalEntry.properties.mutation.enum,
    ["provider_action", "provider_cleanup", "fault_apply", "fault_heal"],
  );
  for (const field of ["last_sequence", "journal_head_sha256", "incomplete_mutation_ids", "journal"]) {
    assert.ok(schema.required.includes(field), field);
  }
  for (const field of ["target_identity_sha256", "target_ownership_sha256"]) {
    assert.ok(schema.$defs.faultIntent.required.includes(field), field);
    assert.ok(schema.$defs.fault.required.includes(field), field);
  }
  for (const field of ["provider_id", "ownership_binding_sha256", "managed_network_id", "managed_network_identity_sha256", "managed_network_configuration_sha256", "provider_storage_identity_sha256", "storage_path"]) {
    assert.ok(schema.$defs.resource.properties[field], field);
  }
  for (const field of [
    "observed_provider_id",
    "observed_ownership_binding_sha256",
    "observed_managed_network_id",
    "observed_managed_network_identity_sha256",
    "observed_managed_network_configuration_sha256",
    "observed_provider_storage_identity_sha256",
    "observed_storage_path",
  ]) assert.ok(schema.$defs.journalEntry.properties[field], field);
});

test("orchestration inventory v2 accepts a contiguous hash-chained replayable journal", () => {
  const liveConfig = config({
    run_id: "test-run",
    working_root: "/dev/shm/agentic-celld-orchestration/test-run",
    inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
  });
  assert.deepEqual(
    validateOrchestrationInventory(inventoryV2(), liveConfig, { expectedHostSha256: "2".repeat(64) }),
    [],
  );
});

test("orchestration inventory v2 rejects broken sequence, hash, completion, and materialized state", () => {
  const liveConfig = config({
    run_id: "test-run",
    working_root: "/dev/shm/agentic-celld-orchestration/test-run",
    inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
  });
  const invalid = [
    (document) => { document.journal[0].sequence = 2; },
    (document) => { document.journal[0].entry_sha256 = "f".repeat(64); },
    (document) => { document.journal[0].event = "completed"; },
    (document) => { document.resources[0].instance_id = "123e4567-e89b-42d3-a456-426614174999"; },
    (document) => { document.incomplete_mutation_ids = []; },
  ];
  for (const corrupt of invalid) {
    const document = inventoryV2();
    corrupt(document);
    assert.notDeepEqual(
      validateOrchestrationInventory(document, liveConfig, { expectedHostSha256: "2".repeat(64) }),
      [],
    );
  }
});

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

function protectedRuntimeFixture({ runId, destructive = false, active = false } = {}) {
  const resolvedRunId = runId ?? `test-${process.pid}-${Date.now()}`;
  const root = join("/dev/shm/agentic-celld-orchestration", resolvedRunId);
  const profileRoot = join("/dev/shm/agentic-celld-storage", resolvedRunId);
  mkdirSync(root, { recursive: true, mode: 0o700 });
  mkdirSync(profileRoot, { recursive: true, mode: 0o700 });
  chmodSync(root, 0o700);
  chmodSync(profileRoot, 0o700);
  const liveConfig = config({
    run_id: resolvedRunId,
    working_root: root,
    inventory_path: join(root, "orchestration-inventory.json"),
    management_binary_path: join(process.cwd(), ".celld-target/release/agentic-mgmt"),
    agent_client_binary_path: join(process.cwd(), ".celld-target/release/agent-client"),
    callback_relay_binary_path: join(process.cwd(), "tools/celld-callback-relay/target/x86_64-unknown-linux-musl/release/agentic-celld-callback-relay"),
  });
  const hostSha256 = createHash("sha256").update(hostname()).digest("hex");
  const document = inventoryV2({ includeProvision: active });
  document.run_id = resolvedRunId;
  document.working_root = root;
  document.host_sha256 = hostSha256;
  document.owner = { ...document.owner, run_id: resolvedRunId };
  for (const resource of document.resources) resource.scenario_id = "UAT-CELLD-003";
  const profile = {
    schema_version: "agentic-sandbox.celld-live-profile/v1",
    profile_id: `${resolvedRunId}-profile`,
    run_id: resolvedRunId,
    expected_sandbox_git: "1".repeat(40),
    environment: { kind: "disposable-local", single_host: true, host_sha256: hostSha256 },
    authorization: { destructive_faults: destructive, inventory_path: liveConfig.inventory_path, ...(destructive ? { exact_run_owner: resolvedRunId } : {}) },
    drivers: { "celld-live-orchestration": { enabled: true, config_path: join(root, "orchestration.json") } },
  };
  writeFileSync(join(root, "orchestration.json"), `${JSON.stringify(liveConfig, null, 2)}\n`, { mode: 0o600 });
  writeFileSync(join(root, "orchestration-inventory.json"), `${JSON.stringify(document, null, 2)}\n`, { mode: 0o600 });
  writeFileSync(join(profileRoot, "live-profile.json"), `${JSON.stringify(profile, null, 2)}\n`, { mode: 0o600 });
  chmodSync(join(root, "orchestration.json"), 0o600);
  chmodSync(join(root, "orchestration-inventory.json"), 0o600);
  chmodSync(join(profileRoot, "live-profile.json"), 0o600);
  return { root, profileRoot, configPath: join(root, "orchestration.json"), profilePath: join(profileRoot, "live-profile.json") };
}

test("protected clean no-op orchestration recovery and cleanup do not require destructive fault authorization", async () => {
  const fixture = protectedRuntimeFixture({ destructive: false, active: false });
  try {
    const runtime = loadProtectedOrchestrationRuntime(fixture.configPath, fixture.profilePath, { requireDestructiveAuthorization: false });
    const recovered = await liveOrchestration.recoverOrchestrationInventory(runtime);
    assert.equal(recovered.status, "PASS");
    assert.equal(recovered.recovered_mutation_ids.length, 0);
    const cleaned = liveOrchestration.cleanupOrchestrationRoot(fixture.configPath);
    assert.equal(cleaned.status, "PASS");
    assert.equal(existsSync(fixture.root), false);
  } finally {
    if (existsSync(fixture.root)) rmSync(fixture.root, { recursive: true, force: true });
    if (existsSync(fixture.profileRoot)) rmSync(fixture.profileRoot, { recursive: true, force: true });
  }
});

test("orchestration cleanup removes only an exact verified managed gRPC socket", () => {
  const fixture = protectedRuntimeFixture({ destructive: false, active: false });
  const socketPath = join(fixture.root, `grpc-${"a".repeat(32)}.sock`);
  try {
    const bound = spawnSync("python3", ["-c", "import socket,sys; s=socket.socket(socket.AF_UNIX); s.bind(sys.argv[1]); s.close()", socketPath], {
      encoding: "utf8",
      shell: false,
    });
    assert.equal(bound.status, 0, bound.stderr);
    assert.equal(lstatSync(socketPath).isSocket(), true);
    const cleaned = liveOrchestration.cleanupOrchestrationRoot(fixture.configPath);
    assert.equal(cleaned.status, "PASS");
    assert.equal(existsSync(fixture.root), false);
  } finally {
    if (existsSync(fixture.root)) rmSync(fixture.root, { recursive: true, force: true });
    if (existsSync(fixture.profileRoot)) rmSync(fixture.profileRoot, { recursive: true, force: true });
  }
});

test("orchestration cleanup rejects a regular file disguised as a managed gRPC socket", () => {
  const fixture = protectedRuntimeFixture({ destructive: false, active: false });
  const lookalikePath = join(fixture.root, `grpc-${"b".repeat(32)}.sock`);
  try {
    writeFileSync(lookalikePath, "not a socket\n", { mode: 0o600 });
    assert.throws(
      () => liveOrchestration.cleanupOrchestrationRoot(fixture.configPath),
      /ambiguous managed gRPC socket residue/,
    );
    assert.equal(existsSync(lookalikePath), true);
  } finally {
    if (existsSync(fixture.root)) rmSync(fixture.root, { recursive: true, force: true });
    if (existsSync(fixture.profileRoot)) rmSync(fixture.profileRoot, { recursive: true, force: true });
  }
});

test("protected retained orchestration effects still require exact destructive authorization", () => {
  const fixture = protectedRuntimeFixture({ destructive: false, active: true });
  try {
    assert.throws(
      () => loadProtectedOrchestrationRuntime(fixture.configPath, fixture.profilePath, { requireDestructiveAuthorization: false }),
      liveOrchestration.OrchestrationCleanupResidueError,
    );
  } finally {
    rmSync(fixture.root, { recursive: true, force: true });
    rmSync(fixture.profileRoot, { recursive: true, force: true });
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

test("Worker acknowledgement leaves every provider action incomplete until independent terminal observation", async (t) => {
  const completeObserved = liveOrchestration.completeObservedProviderMutation;
  for (const action of ["provision", "start", "stop", "destroy"]) {
    await t.test(action, async () => {
      const document = action === "provision"
        ? inventoryV2({ includeProvision: false })
        : inventoryV2({ completedProvision: true });
      const events = [];
      const instanceId = "123e4567-e89b-42d3-a456-426614174000";
      const runtime = {
        scenarioId: "UAT-CELLD-003",
        config: config({
          run_id: "test-run",
          working_root: "/dev/shm/agentic-celld-orchestration/test-run",
          inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
        }),
        orchestrationInventory: document,
        providerResources: new Map([[instanceId, { instanceId, name: "celld-test-provider", substrate: "docker" }]]),
        workerEndpoint: "http://127.0.0.1:18080",
        fleet: { worker_vars_file_ref: "/protected/worker-vars" },
        persistInventory: (_path, persisted) => {
          const entry = persisted.journal.at(-1);
          events.push(`commit:${entry.event}:${entry.mutation}:${entry.subject.action}`);
        },
        sendWorkerCommand: async (request) => {
          events.push(`provider:${request.action}`);
          return { status: 202, body: { effects: [{ operation_id: request.operationId }] } };
        },
      };
      const effect = await issueCommand(runtime, {
        instanceId,
        generation: 1,
        operationId: `operation-${action}`,
        action,
        payload: action === "provision"
          ? { runtime: "docker", name: "celld-test-provider" }
          : {},
      });
      assert.deepEqual(events, [
        `commit:planned:provider_action:${action}`,
        `provider:${action}`,
      ]);
      assert.equal(document.incomplete_mutation_ids.length, 1, "HTTP acknowledgement is not provider-terminal evidence");
      assert.equal(typeof completeObserved, "function", "provider journal completion needs an independent observation boundary");

      const observation = action === "destroy"
        ? exactDockerTerminalObservation({ present: false })
        : exactDockerTerminalObservation({ state: action === "start" ? "running" : action === "stop" ? "exited" : "created" });
      await completeObserved(runtime, { effect, instanceId, generation: 1, action, observation });

      assert.deepEqual(events, [
        `commit:planned:provider_action:${action}`,
        `provider:${action}`,
        `commit:completed:provider_action:${action}`,
      ]);
      assert.deepEqual(document.incomplete_mutation_ids, []);
      if (action === "provision") {
        assert.equal(document.resources[0].provider_identity_sha256, "c".repeat(64));
        assert.equal(document.resources[0].configuration_sha256, "d".repeat(64));
      }
    });
  }
});

test("provider effect convergence retries valid transient states but rejects unowned evidence immediately", async () => {
  const resource = {
    instanceId: "123e4567-e89b-42d3-a456-426614174000",
    name: "celld-test-provider",
    substrate: "qemu",
  };
  const states = ["running", "in shutdown", "shut off"];
  let calls = 0;
  const observation = await liveOrchestration.waitForObservedProviderEffect({}, { resource, action: "stop" }, {
    timeoutMs: 100,
    intervalMs: 0,
    observeProvider: async () => ({
      owned: true,
      present: true,
      state: states[calls++],
      provider_storage_present: true,
    }),
  });
  assert.equal(observation.state, "shut off");
  assert.equal(calls, 3);

  await assert.rejects(
    liveOrchestration.waitForObservedProviderEffect({}, { resource, action: "stop" }, {
      timeoutMs: 100,
      intervalMs: 0,
      observeProvider: async () => ({ owned: false, present: true, state: "shut off", provider_storage_present: true }),
    }),
    /exact-owned/,
  );
});

test("cleanup serializes behind and recovers one interrupted provider action", async () => {
  const document = inventoryV2({ completedProvision: true });
  const instanceId = document.resources[0].instance_id;
  const runtime = {
    scenarioId: "UAT-CELLD-003",
    config: config({
      run_id: "test-run",
      working_root: "/dev/shm/agentic-celld-orchestration/test-run",
      inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
    }),
    orchestrationInventory: document,
    providerResources: new Map([[instanceId, { instanceId, name: "celld-test-provider", substrate: "docker" }]]),
    workerEndpoint: "http://127.0.0.1:18080",
    fleet: { worker_vars_file_ref: "/protected/worker-vars" },
    persistInventory: () => {},
    sendWorkerCommand: async (request) => ({ status: 202, body: { effects: [{ operation_id: request.operationId }] } }),
  };
  await issueCommand(runtime, {
    instanceId,
    generation: 1,
    operationId: "interrupted-stop",
    action: "stop",
    payload: {},
  });
  assert.equal(document.incomplete_mutation_ids.length, 1);
  let observations = 0;
  const assertions = await liveOrchestration.cleanupOwnedProviderResources(runtime, {
    observeProviderResource: async () => (++observations === 1
      ? exactDockerTerminalObservation({ state: "running" })
      : exactDockerTerminalObservation({ present: false })),
    removeProviderResource: async () => {},
  });
  assert.equal(assertions.length, 1);
  assert.deepEqual(document.incomplete_mutation_ids, []);
  assert.equal(document.resources[0].status, "removed");
  assert.deepEqual(document.journal.slice(-2).map((entry) => [entry.event, entry.mutation, entry.outcome]), [
    ["completed", "provider_cleanup", "absent"],
    ["recovered", "provider_action", "absent"],
  ]);
});

test("admin v2 stop waits for libvirt terminal state and has a bounded force fallback", () => {
  const source = readFileSync(new URL("../../../management/src/http/admin_v2.rs", import.meta.url), "utf8");
  const handler = source.match(/async fn stop_instance[\s\S]*?async fn destroy_instance/)?.[0] ?? "";
  assert.match(handler, /graceful_deadline/);
  assert.match(handler, /get_domain_state\(&domain\).*VmState::Stopped/s);
  assert.match(handler, /domain\s*\.destroy\(\)/);
  assert.match(handler, /forced_deadline/);
  assert.ok(handler.indexOf("forced_deadline") < handler.lastIndexOf("synth_succeeded_op"));
});

test("provider terminal authorization is explicit and transactional before persistence", async (t) => {
  const vectors = [
    {
      name: "missing owned assertion",
      substrate: "docker",
      observation: {
        present: true,
        state: "created",
        provider_storage_present: false,
        provider_identity_sha256: "c".repeat(64),
        configuration_sha256: "d".repeat(64),
      },
      error: /owned|authorization/,
    },
    {
      name: "QEMU provision without owned storage",
      substrate: "qemu",
      observation: {
        owned: true,
        present: true,
        state: "shut off",
        provider_storage_present: false,
        provider_identity_sha256: "c".repeat(64),
        configuration_sha256: "d".repeat(64),
      },
      error: /storage|owned/,
    },
  ];
  for (const vector of vectors) {
    await t.test(vector.name, async () => {
      const document = inventoryV2({ includeProvision: false });
      const instanceId = "123e4567-e89b-42d3-a456-426614174000";
      let persistCalls = 0;
      const runtime = {
        scenarioId: "UAT-CELLD-003",
        config: config({
          run_id: "test-run",
          working_root: "/dev/shm/agentic-celld-orchestration/test-run",
          inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
        }),
        orchestrationInventory: document,
        providerResources: new Map(),
        workerEndpoint: "http://127.0.0.1:18080",
        fleet: { worker_vars_file_ref: "/protected/worker-vars" },
        persistInventory: () => { persistCalls += 1; },
        sendWorkerCommand: async (request) => ({ status: 202, body: { effects: [{ operation_id: request.operationId }] } }),
      };
      const effect = await issueCommand(runtime, {
        instanceId,
        generation: 1,
        operationId: `unauthorized-${vector.substrate}`,
        action: "provision",
        payload: { runtime: vector.substrate, name: "celld-test-provider" },
      });
      const beforeTerminal = structuredClone(document);
      await assert.rejects(
        liveOrchestration.completeObservedProviderMutation(runtime, {
          effect,
          instanceId,
          generation: 1,
          action: "provision",
          observation: vector.observation,
        }),
        vector.error,
      );
      assert.deepEqual(document, beforeTerminal, "invalid terminal evidence must not mutate the in-memory journal or materialized view");
      assert.equal(persistCalls, 1, "only the durable plan may be persisted");
    });
  }
});

test("provider provision persists immutable run-owned destructive bindings for Docker and QEMU", async (t) => {
  const vectors = [
    {
      substrate: "docker",
      observation: {
        owned: true,
        present: true,
        state: "created",
        provider_storage_present: false,
        provider_id: "1".repeat(64),
        provider_identity_sha256: "2".repeat(64),
        configuration_sha256: "3".repeat(64),
        ownership_binding_sha256: "4".repeat(64),
        managed_network_id: "5".repeat(64),
        managed_network_identity_sha256: "6".repeat(64),
        managed_network_configuration_sha256: "7".repeat(64),
      },
      fields: ["ownership_binding_sha256", "managed_network_identity_sha256", "managed_network_configuration_sha256"],
    },
    {
      substrate: "qemu",
      observation: {
        owned: true,
        present: true,
        state: "shut off",
        provider_storage_present: true,
        provider_id: "11111111-2222-4333-8444-555555555555",
        provider_identity_sha256: "8".repeat(64),
        configuration_sha256: "9".repeat(64),
        ownership_binding_sha256: "a".repeat(64),
        provider_storage_identity_sha256: "b".repeat(64),
        storage_path: "/build/agentic-sandbox/vms/celld-test-provider",
        storage_device: "8",
        storage_inode: "9",
        storage_uid: "1000",
        storage_gid: "1000",
      },
      fields: ["ownership_binding_sha256", "provider_storage_identity_sha256"],
    },
  ];
  for (const vector of vectors) {
    await t.test(vector.substrate, async () => {
      const document = inventoryV2({ includeProvision: false });
      const instanceId = "123e4567-e89b-42d3-a456-426614174000";
      const runtime = {
        scenarioId: "UAT-CELLD-003",
        config: config({
          run_id: "test-run",
          working_root: "/dev/shm/agentic-celld-orchestration/test-run",
          inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
        }),
        orchestrationInventory: document,
        providerResources: new Map(),
        workerEndpoint: "http://127.0.0.1:18080",
        fleet: { worker_vars_file_ref: "/protected/worker-vars" },
        persistInventory: () => {},
        sendWorkerCommand: async (request) => ({ status: 202, body: { effects: [{ operation_id: request.operationId }] } }),
      };
      const effect = await issueCommand(runtime, {
        instanceId,
        generation: 1,
        operationId: `binding-${vector.substrate}`,
        action: "provision",
        payload: { runtime: vector.substrate, name: "celld-test-provider" },
      });
      const resource = await liveOrchestration.completeObservedProviderMutation(runtime, {
        effect,
        instanceId,
        generation: 1,
        action: "provision",
        observation: vector.observation,
      });
      assert.equal(resource.provider_identity_sha256, vector.observation.provider_identity_sha256);
      assert.equal(resource.configuration_sha256, vector.observation.configuration_sha256);
      for (const field of vector.fields) assert.equal(resource[field], vector.observation[field], field);
    });
  }
});

test("fault apply and heal require independent observers before journal completion", async () => {
  const document = inventoryV2({ includeProvision: false });
  const events = [];
  let faultPresent = false;
  const binding = exactFaultBinding();
  const runtime = {
    scenarioId: "UAT-CELLD-006",
    runId: "test-run",
    config: config({
      run_id: "test-run",
      working_root: "/dev/shm/agentic-celld-orchestration/test-run",
      inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
    }),
    orchestrationInventory: document,
    resolveFaultTarget: async () => binding,
    revalidateFaultTarget: async () => binding,
    persistInventory: (_path, persisted) => {
      const entry = persisted.journal.at(-1);
      events.push(`commit:${entry.event}:${entry.mutation}`);
    },
    observeFaultTarget: async ({ plan }) => {
      events.push(`observe:${plan.mutation}:${faultPresent ? "present" : "absent"}`);
      return { ...binding, present: faultPresent };
    },
  };

  const record = await applyPlannedOrchestrationFault(
    runtime,
    { kind: "callback_relay_pause", target: "celld-test-relay" },
    async () => {
      events.push("fault-apply");
      faultPresent = true;
    },
  );
  await healPlannedOrchestrationFault(runtime, record, async () => {
    events.push("fault-heal");
    faultPresent = false;
  });

  assert.deepEqual(events, [
    "commit:planned:fault_apply",
    "fault-apply",
    "observe:fault_apply:present",
    "commit:completed:fault_apply",
    "commit:planned:fault_heal",
    "fault-heal",
    "observe:fault_heal:absent",
    "commit:completed:fault_heal",
  ]);
});

test("fault mutation persists an immutable run-owned target binding before the destructive callback", async () => {
  const document = inventoryV2({ includeProvision: false });
  const events = [];
  const binding = {
    owned: true,
    provider_id: "1".repeat(64),
    target_identity_sha256: "2".repeat(64),
    target_ownership_sha256: "3".repeat(64),
    labels: { "agentic-run-id": "test-run", "agentic-source": "celld-live-orchestration" },
  };
  const runtime = {
    scenarioId: "UAT-CELLD-006",
    runId: "test-run",
    config: config({
      run_id: "test-run",
      working_root: "/dev/shm/agentic-celld-orchestration/test-run",
      inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
    }),
    orchestrationInventory: document,
    resolveFaultTarget: async () => {
      events.push("resolve-exact-target");
      return binding;
    },
    observeFaultTarget: async ({ plan }) => {
      events.push("observe-effect");
      assert.equal(plan.subject.target_identity_sha256, binding.target_identity_sha256);
      assert.equal(plan.subject.target_ownership_sha256, binding.target_ownership_sha256);
      return { ...binding, present: true };
    },
    persistInventory: (_path, inventory) => events.push(`commit:${inventory.journal.at(-1).event}`),
  };
  let destructiveCalls = 0;
  await applyPlannedOrchestrationFault(
    runtime,
    { kind: "callback_relay_pause", target: "celld-test-relay" },
    async (exactTarget) => {
      events.push("destructive-effect");
      destructiveCalls += 1;
      assert.deepEqual(exactTarget, binding);
    },
  );
  assert.deepEqual(events, ["resolve-exact-target", "commit:planned", "destructive-effect", "observe-effect", "commit:completed"]);
  assert.equal(destructiveCalls, 1);
  assert.equal(document.faults[0].target_identity_sha256, binding.target_identity_sha256);
  assert.equal(document.faults[0].target_ownership_sha256, binding.target_ownership_sha256);
});

test("fault target substitution immediately before the effect makes zero destructive calls", async () => {
  const document = inventoryV2({ includeProvision: false });
  const authorized = {
    owned: true,
    provider_id: "1".repeat(64),
    target_identity_sha256: "2".repeat(64),
    target_ownership_sha256: "3".repeat(64),
  };
  const substituted = { ...authorized, provider_id: "4".repeat(64), target_identity_sha256: "5".repeat(64) };
  const runtime = {
    scenarioId: "UAT-CELLD-006",
    runId: "test-run",
    config: config({
      run_id: "test-run",
      working_root: "/dev/shm/agentic-celld-orchestration/test-run",
      inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
    }),
    orchestrationInventory: document,
    resolveFaultTarget: async () => authorized,
    revalidateFaultTarget: async () => substituted,
    observeFaultTarget: async () => ({ ...substituted, present: true }),
    persistInventory: () => {},
  };
  let destructiveCalls = 0;
  await assert.rejects(
    applyPlannedOrchestrationFault(
      runtime,
      { kind: "callback_relay_pause", target: "celld-test-relay" },
      async () => { destructiveCalls += 1; },
    ),
    /substitut|binding|identity|ownership/,
  );
  assert.equal(destructiveCalls, 0);
});

test("fault mutation is refused before apply when no exact observer is installed", async () => {
  let applyCalls = 0;
  const document = inventoryV2({ includeProvision: false });
  const runtime = {
    scenarioId: "UAT-CELLD-006",
    config: config({
      run_id: "test-run",
      working_root: "/dev/shm/agentic-celld-orchestration/test-run",
      inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
    }),
    orchestrationInventory: document,
    persistInventory: () => {},
  };
  await assert.rejects(
    applyPlannedOrchestrationFault(runtime, { kind: "callback_relay_pause", target: "celld-test-relay" }, async () => { applyCalls += 1; }),
    /observer/,
  );
  assert.equal(applyCalls, 0);
  assert.deepEqual(document.incomplete_mutation_ids, []);
});

test("fault journal completion refuses an independently observed mismatched terminal state", async (t) => {
  await t.test("apply remains incomplete when the target is still absent", async () => {
    let applyCalls = 0;
    let observeCalls = 0;
    const document = inventoryV2({ includeProvision: false });
    const binding = exactFaultBinding();
    const runtime = {
      scenarioId: "UAT-CELLD-006",
      runId: "test-run",
      config: config({
        run_id: "test-run",
        working_root: "/dev/shm/agentic-celld-orchestration/test-run",
        inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
      }),
      orchestrationInventory: document,
      resolveFaultTarget: async () => binding,
      revalidateFaultTarget: async () => binding,
      persistInventory: () => {},
      observeFaultTarget: async () => {
        observeCalls += 1;
        return { ...binding, present: false };
      },
    };
    await assert.rejects(
      applyPlannedOrchestrationFault(runtime, { kind: "callback_relay_pause", target: "celld-test-relay" }, async () => { applyCalls += 1; }),
      /observ|effect|present/,
    );
    assert.equal(applyCalls, 1);
    assert.equal(observeCalls, 1);
    assert.equal(document.incomplete_mutation_ids.length, 1);
  });

  await t.test("heal remains incomplete when the target is still present", async () => {
    let faultPresent = false;
    let healCalls = 0;
    const document = inventoryV2({ includeProvision: false });
    const binding = exactFaultBinding();
    const runtime = {
      scenarioId: "UAT-CELLD-006",
      runId: "test-run",
      config: config({
        run_id: "test-run",
        working_root: "/dev/shm/agentic-celld-orchestration/test-run",
        inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
      }),
      orchestrationInventory: document,
      resolveFaultTarget: async () => binding,
      revalidateFaultTarget: async () => binding,
      persistInventory: () => {},
      observeFaultTarget: async () => ({ ...binding, present: faultPresent }),
    };
    const record = await applyPlannedOrchestrationFault(
      runtime,
      { kind: "callback_relay_pause", target: "celld-test-relay" },
      async () => { faultPresent = true; },
    );
    await assert.rejects(
      healPlannedOrchestrationFault(runtime, record, async () => { healCalls += 1; }),
      (error) => {
        assert.match(error.message, /observ|effect|absent/);
        assert.equal(error.errorCode, "CELLD_FAULT_HEAL_OBSERVATION_PRESENT");
        assert.match(error.evidenceSha256, /^[0-9a-f]{64}$/);
        return true;
      },
    );
    assert.equal(healCalls, 1);
    assert.equal(document.incomplete_mutation_ids.length, 1);
  });

  await t.test("heal classifies an unavailable terminal observer", async () => {
    let observeCalls = 0;
    const document = inventoryV2({ includeProvision: false });
    const binding = exactFaultBinding();
    const runtime = {
      scenarioId: "UAT-CELLD-006",
      runId: "test-run",
      config: config({
        run_id: "test-run",
        working_root: "/dev/shm/agentic-celld-orchestration/test-run",
        inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
      }),
      orchestrationInventory: document,
      resolveFaultTarget: async () => binding,
      revalidateFaultTarget: async () => binding,
      persistInventory: () => {},
      observeFaultTarget: async () => {
        observeCalls += 1;
        if (observeCalls > 1) throw new Error("observer unavailable");
        return { ...binding, present: true };
      },
    };
    const record = await applyPlannedOrchestrationFault(
      runtime,
      { kind: "callback_relay_pause", target: "celld-test-relay" },
      async () => {},
    );
    await assert.rejects(
      healPlannedOrchestrationFault(runtime, record, async () => {}),
      (error) => {
        assert.equal(error.errorCode, "CELLD_FAULT_HEAL_OBSERVATION_FAILED");
        return true;
      },
    );
    assert.equal(document.incomplete_mutation_ids.length, 1);
  });
});

test("authorized orchestration inventory rejects unsafe run-root and hard-link ownership before mutation", async (t) => {
  await t.test("group-accessible run root", () => {
    const fixture = exactOrchestrationTestRoot("red-owner");
    const { root } = fixture;
    try {
      const inventoryPath = join(root, "orchestration-inventory.json");
      const liveConfig = config({ run_id: "test-run", working_root: root, inventory_path: inventoryPath });
      const profile = { run_id: "test-run", authorization: { destructive_faults: true, exact_run_owner: "test-run", inventory_path: inventoryPath } };
      writeFileSync(inventoryPath, `${JSON.stringify(inventory({ working_root: root }))}\n`, { mode: 0o600 });
      chmodSync(root, 0o750);
      assert.throws(() => loadAuthorizedOrchestrationInventory(profile, liveConfig, "2".repeat(64)));
    } finally {
      chmodSync(root, 0o700);
      fixture.cleanup();
    }
  });

  await t.test("hard-linked inventory", () => {
    const fixture = exactOrchestrationTestRoot("red-hardlink");
    const { root } = fixture;
    try {
      const inventoryPath = join(root, "orchestration-inventory.json");
      const backingPath = join(root, "backing.json");
      const liveConfig = config({ run_id: "test-run", working_root: root, inventory_path: inventoryPath });
      const profile = { run_id: "test-run", authorization: { destructive_faults: true, exact_run_owner: "test-run", inventory_path: inventoryPath } };
      writeFileSync(backingPath, `${JSON.stringify(inventory({ working_root: root }))}\n`, { mode: 0o600 });
      linkSync(backingPath, inventoryPath);
      assert.throws(() => loadAuthorizedOrchestrationInventory(profile, liveConfig, "2".repeat(64)));
    } finally {
      fixture.cleanup();
    }
  });

  await t.test("symlinked run-root component", () => {
    const parent = "/dev/shm/agentic-celld-orchestration";
    const removeParent = !existsSync(parent);
    mkdirSync(parent, { recursive: true, mode: 0o700 });
    const outer = mkdtempSync(join(parent, "red-parent-link-"));
    try {
      const realRoot = join(outer, "real-root");
      const linkedRoot = join(outer, "test-run");
      mkdirSync(realRoot, { mode: 0o700 });
      symlinkSync(realRoot, linkedRoot);
      const inventoryPath = join(linkedRoot, "orchestration-inventory.json");
      const liveConfig = config({ run_id: "test-run", working_root: linkedRoot, inventory_path: inventoryPath });
      const profile = { run_id: "test-run", authorization: { destructive_faults: true, exact_run_owner: "test-run", inventory_path: inventoryPath } };
      writeFileSync(join(realRoot, "orchestration-inventory.json"), `${JSON.stringify(inventory({ working_root: linkedRoot }))}\n`, { mode: 0o600 });
      assert.throws(() => loadAuthorizedOrchestrationInventory(profile, liveConfig, "2".repeat(64)));
    } finally {
      rmSync(outer, { recursive: true, force: true });
      if (removeParent) rmdirSync(parent);
    }
  });
});

test("restart recovery reconciles an incomplete provider plan without replay and is idempotent", async () => {
  const recover = liveOrchestration.recoverOrchestrationInventory;
  assert.equal(typeof recover, "function", "the orchestration inventory needs an explicit restart-recovery boundary");

  const document = inventoryV2();
  const instanceId = document.resources[0].instance_id;
  let providerPresent = true;
  let providerActionCalls = 0;
  let cleanupCalls = 0;
  let observationCalls = 0;
  const runtime = {
    scenarioId: "UAT-CELLD-003",
    runId: "test-run",
    config: config({
      run_id: "test-run",
      working_root: "/dev/shm/agentic-celld-orchestration/test-run",
      inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
    }),
    orchestrationInventory: document,
    providerResources: new Map([[instanceId, { instanceId, name: "celld-test-provider", substrate: "docker" }]]),
    sendWorkerCommand: async () => { providerActionCalls += 1; },
    persistInventory: () => {},
  };
  const dependencies = {
    observeProviderResource: async () => {
      observationCalls += 1;
      return providerPresent
        ? exactDockerTerminalObservation()
        : exactDockerTerminalObservation({ present: false });
    },
    removeProviderResource: async () => {
      cleanupCalls += 1;
      providerPresent = false;
    },
  };

  await recover(runtime, dependencies);
  await recover(runtime, dependencies);

  assert.equal(providerActionCalls, 0, "recovery must never replay the incomplete original action");
  assert.equal(cleanupCalls, 1, "the exact observed target is removed once");
  assert.ok(observationCalls >= 2, "absence is independently observed");
  assert.deepEqual(document.incomplete_mutation_ids, []);
  assert.equal(document.resources[0].status, "removed");
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

function ownerRuntime({ stopped = [], foreign = [], epochs = [7, 7] } = {}) {
  const nodes = [1, 2, 3].map((index) => ({
    name: `celld-test-node-${index}`,
    node_id: `test-run-node-${index}`,
    advertise: `celld-test-node-${index}:8081`,
  }));
  const scope = celldInstanceCellScope("123e4567-e89b-42d3-a456-426614174000");
  let remoteIndex = 0;
  return {
    runId: "test-run",
    fleet: { run_id: "test-run", network: { name: "celld-test-private" }, nodes },
    runCommand: (_program, args) => {
      const node = nodes.find((candidate) => candidate.name === args.at(-1));
      const repository = foreign.includes(node.name) ? "someone/else" : "roctinam/agentic-sandbox";
      return `${stopped.includes(node.name) ? "false" : "true"}|${repository}|celld-qualification|test-run|celld-qualification|172.29.0.${nodes.indexOf(node) + 2}`;
    },
    fetchCelldInternal: async (url) => {
      assert.equal(url.pathname, `/cell/${scope}`);
      if (url.hostname === "172.29.0.3") return { status: 200, body: { route: "local", cell: scope } };
      return {
        status: 307,
        body: {
          route: "remote",
          node: "test-run-node-2",
          addr: "celld-test-node-2:8081",
          epoch: epochs[remoteIndex++],
          peer_protocol: 2,
        },
      };
    },
  };
}

test("Celld ownership observation targets the exact local owner and records remote epoch agreement", async () => {
  const instanceId = "123e4567-e89b-42d3-a456-426614174000";
  assert.equal(celldInstanceCellScope(instanceId), "InstanceCell:28a01b598434d0091a8aab9b95f429d56a68e52ec797754bf538c8cb3395f936");
  const observed = await observeCelldOwnership(ownerRuntime(), { instanceId }, new Date("2026-08-23T00:00:00Z"));
  assert.equal(observed.owner_target, "celld-test-node-2");
  assert.equal(observed.owner_node_id, "test-run-node-2");
  assert.equal(observed.owner_epoch, 7);
  assert.equal(observed.live_nodes, 3);
  assert.equal(observed.route_agreement, true);
  assert.match(observed.cell_scope_sha256, /^[0-9a-f]{64}$/);

  const afterLoss = await observeCelldOwnership(ownerRuntime({ stopped: ["celld-test-node-1"] }), { instanceId });
  assert.equal(afterLoss.owner_target, "celld-test-node-2");
  assert.equal(afterLoss.live_nodes, 2);
});

test("Celld ownership observation rejects foreign nodes and disagreeing epochs", async () => {
  const instanceId = "123e4567-e89b-42d3-a456-426614174000";
  await assert.rejects(
    observeCelldOwnership(ownerRuntime({ foreign: ["celld-test-node-1"] }), { instanceId }),
    /unowned fleet node/,
  );
  await assert.rejects(
    observeCelldOwnership(ownerRuntime({ epochs: [7, 8] }), { instanceId }),
    /disagree on owner identity or epoch/,
  );
});

test("provider observation rejects production-realistic Docker metadata without exact external run ownership", () => {
  const instanceId = "123e4567-e89b-42d3-a456-426614174000";
  const providerId = "1".repeat(64);
  const networkId = "2".repeat(64);
  const runtime = {
    runId: "test-run",
    config: config(),
    providerResources: new Map([[instanceId, { instanceId, name: "celld-test-provider", substrate: "docker" }]]),
    runCommand: (program, args) => {
      assert.equal(program, "docker");
      if (args[0] === "ps") return providerId;
      if (args[0] === "network") {
        return `${networkId}|${JSON.stringify({ "agentic-sandbox": "celld", "agentic-egress": "deny" })}`;
      }
      if (args[2].includes(".State.Status")) {
        return `${providerId}|created|sha256:${"a".repeat(64)}|${instanceId}|admin-v2|celld-provider-network`;
      }
      return JSON.stringify({
        "agentic-instance-id": instanceId,
        "agentic-source": "admin-v2",
        "agentic-managed-network": "celld-provider-network",
      });
    },
  };
  assert.throws(
    () => observeOrchestrationProvider(runtime, { instanceId, name: "celld-test-provider", substrate: "docker" }),
    /run|ownership|label|external|verif/,
  );
});

test("provider observation rejects QEMU identity synthesized from requested name UUID and local path", () => {
  const fixture = exactOrchestrationTestRoot("red-qemu-metadata");
  const instanceId = "123e4567-e89b-42d3-a456-426614174000";
  const name = "celld-test-provider";
  const storageRoot = join(fixture.root, "vms");
  try {
    mkdirSync(join(storageRoot, name), { recursive: true, mode: 0o700 });
    const runtime = {
      runId: "test-run",
      config: config({ vm_storage_dir: storageRoot }),
      providerResources: new Map([[instanceId, { instanceId, name, substrate: "qemu" }]]),
      runCommand: (program, args) => {
        assert.equal(program, "virsh");
        if (args.includes("list")) return `${name}\n`;
        if (args.includes("domuuid")) return "11111111-2222-4333-8444-555555555555\n";
        if (args.includes("domstate")) return "shut off\n";
        if (args.includes("dumpxml")) return `<domain><name>${name}</name><devices><disk><source file="${join(storageRoot, name, "disk.qcow2")}"/></disk></devices></domain>`;
        throw new Error(`unexpected virsh arguments: ${args.join(" ")}`);
      },
    };
    assert.throws(
      () => observeOrchestrationProvider(runtime, { instanceId, name, substrate: "qemu" }),
      /run|ownership|metadata|external|verif/,
    );
  } finally {
    fixture.cleanup();
  }
});

test("UAT-004 healing accepts only a typed same-run management PID replacement", async (t) => {
  function processHandle(pid, runId) {
    return {
      pid,
      exitCode: null,
      signalCode: null,
      killed: false,
      spawn_identity: {
        run_id: runId,
        executable_sha256: "6".repeat(64),
        process_start_time_ticks: String(10_000 + pid),
      },
    };
  }

  async function appliedManagementFault() {
    const document = inventoryV2({ includeProvision: false });
    const oldHandle = processHandle(4101, "test-run");
    const runtime = {
      scenarioId: "UAT-CELLD-004",
      runId: "test-run",
      config: config({
        run_id: "test-run",
        working_root: "/dev/shm/agentic-celld-orchestration/test-run",
        inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
      }),
      orchestrationInventory: document,
      management: { processHandle: oldHandle },
      persistInventory: () => {},
      resolveFaultTarget: liveOrchestration.resolveOrchestrationFaultTarget,
      revalidateFaultTarget: liveOrchestration.resolveOrchestrationFaultTarget,
      observeFaultTarget: liveOrchestration.observeOrchestrationFaultTarget,
    };
    const record = await applyPlannedOrchestrationFault(runtime, { kind: "management_process_kill", target: "management" }, async () => {
      oldHandle.killed = true;
      oldHandle.signalCode = "SIGKILL";
    });
    return { document, runtime, record };
  }

  await t.test("same-run replacement is an expected heal transition", async () => {
    const { document, runtime, record } = await appliedManagementFault();
    runtime.management = { processHandle: processHandle(4102, "test-run") };
    let healCalls = 0;
    await healPlannedOrchestrationFault(runtime, record, async () => { healCalls += 1; });
    assert.equal(healCalls, 1);
    assert.equal(record.status, "healed");
    const terminal = document.journal.at(-1);
    assert.equal(terminal.target_transition?.kind, "management_process_replacement");
    assert.equal(terminal.target_transition?.replacement_provider_id, "4102");
  });

  await t.test("foreign replacement remains denied before heal effect", async () => {
    const { runtime, record } = await appliedManagementFault();
    runtime.management = { processHandle: processHandle(5102, "foreign-run") };
    let healCalls = 0;
    await assert.rejects(
      healPlannedOrchestrationFault(runtime, record, async () => { healCalls += 1; }),
      /foreign|ownership|substitut|run/,
    );
    assert.equal(healCalls, 0);
  });
});

test("UAT-005 response-loss signal addresses the persisted revalidated container ID, never its mutable name", async () => {
  const signal = liveOrchestration.signalExactCallbackResponseLoss;
  assert.equal(typeof signal, "function", "the production UAT-005 signal needs a behavior-testable exact-ID boundary");
  const providerId = "3".repeat(64);
  const calls = [];
  let armed = false;
  await signal({
    runId: "test-run",
    runCommand: (program, args) => {
      calls.push([program, args]);
      if (args[0] === "logs") return armed ? "Celld callback relay armed one response loss\n" : "";
      if (args[0] === "kill") armed = true;
      return "";
    },
  }, {
    owned: true,
    target: "celld-test-node-1-callback-relay",
    provider_id: providerId,
    target_identity_sha256: createHash("sha256").update(`docker:${providerId}`).digest("hex"),
    target_ownership_sha256: "4".repeat(64),
  });
  assert.deepEqual(calls[0], ["docker", ["logs", "--tail", "8192", providerId]]);
  assert.deepEqual(calls[1], ["docker", ["kill", "--signal", "SIGUSR1", providerId]]);
  assert.equal(calls[2][0], "docker");
  assert.deepEqual(calls[2][1].slice(0, 3), ["logs", "--tail", "8192"]);
  assert.equal(calls[2][1].at(-1), providerId);
  assert.equal(calls.flat(2).includes("celld-test-node-1-callback-relay"), false);
});

test("fault target resolution rejects exact-name containers with foreign or missing authoritative labels", async (t) => {
  const providerId = "5".repeat(64);
  const exactLabels = {
    "dev.agentic-sandbox.repository": "roctinam/agentic-sandbox",
    "dev.agentic-sandbox.workflow": "celld-qualification",
    "dev.agentic-sandbox.run": "test-run",
    "dev.agentic-sandbox.scope": "celld-qualification",
  };
  const vectors = [
    { name: "fleet node foreign repository", kind: "fleet_node_stop", target: "celld-test-node-1", alter: (labels) => { labels["dev.agentic-sandbox.repository"] = "someone/else"; } },
    { name: "fleet node missing workflow", kind: "fleet_node_stop", target: "celld-test-node-1", alter: (labels) => { delete labels["dev.agentic-sandbox.workflow"]; } },
    { name: "callback relay foreign run", kind: "callback_response_loss", target: "celld-test-node-1-callback-relay", alter: (labels) => { labels["dev.agentic-sandbox.run"] = "foreign-run"; } },
    { name: "callback relay missing scope", kind: "callback_relay_pause", target: "celld-test-node-1-callback-relay", alter: (labels) => { delete labels["dev.agentic-sandbox.scope"]; } },
  ];
  for (const vector of vectors) {
    await t.test(vector.name, async () => {
      const labels = structuredClone(exactLabels);
      vector.alter(labels);
      const runtime = {
        runId: "test-run",
        fleet: { nodes: [{ name: "celld-test-node-1" }] },
        runCommand: (program, args) => {
          assert.equal(program, "docker");
          assert.equal(args.at(-1), vector.target, "the exact allowed target must still be inspected");
          return `${providerId}|${JSON.stringify(labels)}`;
        },
      };
      await assert.rejects(
        liveOrchestration.resolveOrchestrationFaultTarget({ runtime, fault: { kind: vector.kind, target: vector.target } }),
        /foreign|missing|repository|workflow|run|scope|ownership|label/,
      );
    });
  }
});
