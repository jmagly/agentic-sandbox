#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { chmodSync, existsSync, lstatSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { hostname } from "node:os";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  cleanupFleet,
  deployFleetQualificationWorker,
  deployFleetWorker,
  diagnoseFleet,
  openFleetWorkerAccess,
  prepareFleet,
  probeFleetWorker,
  startFleet,
  stopFleetForWorkerDeployment,
  workerDeploymentProjectDigest,
} from "./celld-fleet-fixture.mjs";
import { cleanupFixture, prepareFixture, startFixture } from "./celld-seaweedfs-fixture.mjs";
import { driverErrorDocument, driverOperationError, emitDriverError as emitLiveDriverError, withDriverOperation } from "./celld-live-driver-error.mjs";
import { getWorkerCell } from "./celld-worker-client.mjs";
import { validateOrchestrationConfig } from "./celld-live-orchestration.mjs";
import { validateLiveProfile } from "./celld-uat-live-protocol.mjs";
export { driverErrorDocument };

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
const DRIVER_ID = "celld-live-worker";
const DRIVER_VERSION = "celld-live-worker/v1";
const OBSERVATION_SCHEMA = "agentic-sandbox.celld-live-observation/v1";
const SCENARIOS = new Set(["UAT-CELLD-007", "UAT-CELLD-008", "UAT-CELLD-009"]);
const ADVERTISED = ["fetch", "rpc", "storage", "alarm", "websocket", "outbound_https", "wasm", "assets"];
const EXCLUDED = ["process", "pty", "workspace", "filesystem", "raw_network", "vm", "container", "host_api"];
const WASM_ADD_HEX = "0061736d0100000001070160027f7f017f030201000707010361646400000a09010700200020016a0b";
const CONFORMANCE_WORKER = `
import addModule from "./add.wasm";
import { DurableObject, WorkerEntrypoint } from "cloudflare:workers";

const add = new WebAssembly.Instance(addModule).exports.add;

export class CapabilityRpc extends WorkerEntrypoint {
  add(left, right) { return left + right; }
}

export class CapabilityCell extends DurableObject {
  constructor(state, env) { super(state, env); this.state = state; }
  async fetch(request) {
    const path = new URL(request.url).pathname;
    if (path === "/storage") {
      const value = (await this.state.storage.get("value") ?? 0) + 1;
      await this.state.storage.put("value", value);
      return Response.json({ value });
    }
    if (path === "/alarm") {
      await this.state.storage.put("alarm_fired", false);
      await this.state.storage.setAlarm(Date.now() + 100);
      return Response.json({ armed: true });
    }
    if (path === "/alarm-state") return Response.json({ fired: await this.state.storage.get("alarm_fired") === true });
    if (path === "/websocket") {
      if (request.headers.get("Upgrade")?.toLowerCase() !== "websocket") return new Response("upgrade required", { status: 426 });
      const pair = new WebSocketPair();
      this.state.acceptWebSocket(pair[0]);
      return new Response(null, { status: 101, webSocket: pair[1] });
    }
    return new Response("not found", { status: 404 });
  }
  async alarm() { await this.state.storage.put("alarm_fired", true); }
  webSocketMessage(socket, message) { socket.send(JSON.stringify({ echo: message })); }
}

export default {
  async fetch(request, env, ctx) {
    const path = new URL(request.url).pathname;
    if (path === "/fetch") return Response.json({ capability: "fetch" });
    if (path === "/rpc") return Response.json({ value: await ctx.exports.CapabilityRpc.add(20, 22) });
    if (["/storage", "/alarm", "/alarm-state", "/websocket"].includes(path)) return env.CAPABILITY.getByName("live-worker").fetch(request);
    if (path === "/outbound") {
      const response = await fetch("https://s3gateway1:8334/");
      return Response.json({ status: response.status });
    }
    if (path === "/wasm") return Response.json({ value: add(2, 3) });
    return new Response("not found", { status: 404 });
  },
};
`.trimStart();

function sha256(value) { return createHash("sha256").update(value).digest("hex"); }

function emitDriverError(error) {
  emitLiveDriverError("CELLD_LIVE_WORKER_ERROR", error);
}

function sleep(ms) { return new Promise((resolvePromise) => setTimeout(resolvePromise, ms)); }

function argument(args, name) {
  const index = args.indexOf(name);
  if (index < 0 || !args[index + 1]) throw new Error(`${name} is required`);
  return args[index + 1];
}

function run(program, args, options = {}) {
  const result = spawnSync(program, args, { encoding: "utf8", shell: false, ...options });
  if (result.error || result.status !== 0) throw new Error(`${basename(program)} failed: ${(result.error?.message ?? result.stderr ?? "").trim()}`);
  return result.stdout.trim();
}

function commandFailureFields(result, errorCode) {
  const fields = {
    errorCode,
    stdoutSha256: sha256(result?.stdout ?? ""),
    stderrSha256: sha256(result?.stderr ?? ""),
  };
  if (Number.isInteger(result?.status)) fields.exitStatus = result.status;
  if (typeof result?.signal === "string") fields.signal = result.signal;
  return fields;
}

function runWorkerCommand(operation, fields, program, args, options = {}, errorCode = "CELLD_LIVE_WORKER_COMMAND_FAILED") {
  const result = spawnSync(program, args, { encoding: "utf8", shell: false, ...options });
  if (result.error) {
    throw driverOperationError(operation, { ...fields, errorCode: result.error.code ?? errorCode }, result.error.message);
  }
  if (result.status !== 0) {
    throw driverOperationError(operation, { ...fields, ...commandFailureFields(result, errorCode) }, `${basename(program)} command failed`);
  }
  return result.stdout.trim();
}

async function withWorkerAccess(operation, errorFields, fleetPath, callback) {
  const access = await withDriverOperation(operation, errorFields, () => openFleetWorkerAccess(fleetPath));
  let callbackError = null;
  try {
    return await callback(access.endpoint, access);
  } catch (error) {
    callbackError = error;
    throw error;
  } finally {
    try {
      await withDriverOperation(`${operation}.close`, errorFields, () => access.close());
    } catch (closeError) {
      if (!callbackError) throw closeError;
    }
  }
}

function protectedJson(path, description) {
  if (!isAbsolute(path) || !existsSync(path)) throw new Error(`${description} is missing`);
  const metadata = lstatSync(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) throw new Error(`${description} must be a protected regular non-symlink file`);
  return JSON.parse(readFileSync(path, "utf8"));
}

function exactDeployer(runId) {
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(runId)) throw new Error("Worker cleanup run identity is invalid");
  return { name: `celld-worker-deploy-${sha256(runId).slice(0, 16)}`, labels: { "dev.agentic-sandbox.run": runId, "dev.agentic-sandbox.scope": "celld-qualification" } };
}

function inspectContainer(runner, name) {
  try { return JSON.parse(runner("docker", ["container", "inspect", name])); } catch { return null; }
}

export function cleanupWorkerResources(runId, { runner = run } = {}) {
  const deployer = exactDeployer(runId);
  runner("docker", ["info", "--format", "{{.ServerVersion}}"], { timeout: 30_000 });
  const document = inspectContainer(runner, deployer.name);
  if (!document) return { status: "PASS", run_id: runId, removed: [], residue: [] };
  const labels = document?.[0]?.Config?.Labels ?? {};
  for (const [key, value] of Object.entries(deployer.labels)) if (labels[key] !== value) throw new Error(`refusing unowned Worker deployer ${deployer.name}`);
  runner("docker", ["rm", "--force", "--volumes", deployer.name], { timeout: 120_000 });
  return { status: "PASS", run_id: runId, removed: [deployer.name], residue: [] };
}

export function prepareWorkerConformanceProject(root) {
  const project = join(root, "worker-conformance");
  const assets = join(project, "assets");
  mkdirSync(assets, { recursive: true, mode: 0o700 });
  const config = {
    name: "agentic-worker-conformance", main: "worker.mjs", compatibility_date: "2026-08-14",
    durable_objects: { bindings: [{ name: "CAPABILITY", class_name: "CapabilityCell" }] },
    migrations: [{ tag: "v1", new_sqlite_classes: ["CapabilityCell"] }],
    assets: { directory: "./assets", binding: "ASSETS" },
  };
  writeFileSync(join(project, "worker.mjs"), CONFORMANCE_WORKER, { mode: 0o600, flag: "wx" });
  writeFileSync(join(project, "wrangler.json"), `${JSON.stringify(config)}\n`, { mode: 0o600, flag: "wx" });
  writeFileSync(join(project, "add.wasm"), Buffer.from(WASM_ADD_HEX, "hex"), { mode: 0o600, flag: "wx" });
  writeFileSync(join(assets, "capability-asset.txt"), "agentic-celld-asset-v1\n", { mode: 0o600, flag: "wx" });
  return { path: project, sha256: workerDeploymentProjectDigest(project) };
}

function deployProject(runtime, projectPath, projectDigest, deploymentKind) {
  cleanupWorkerResources(runtime.runId);
  return deployFleetQualificationWorker(runtime.fleetPath, { projectPath, projectDigest, deploymentKind });
}

async function boundedFetch(endpoint, path) {
  const response = await fetch(new URL(path, endpoint), { redirect: "error", signal: AbortSignal.timeout(10_000) });
  const bytes = Buffer.from(await response.arrayBuffer());
  if (bytes.length > 4096) throw new Error("conformance response exceeds 4 KiB");
  return { status: response.status, bytes, text: bytes.toString("utf8"), json: () => JSON.parse(bytes.toString("utf8")) };
}

async function websocketCase(endpoint) {
  return new Promise((resolvePromise, reject) => {
    const socket = new WebSocket(new URL("/websocket", endpoint));
    const timer = setTimeout(() => { socket.close(); reject(new Error("WebSocket conformance deadline exceeded")); }, 10_000);
    socket.addEventListener("open", () => socket.send("celld-live-worker"));
    socket.addEventListener("message", (event) => {
      clearTimeout(timer);
      try { resolvePromise(JSON.parse(String(event.data)).echo === "celld-live-worker"); } catch (error) { reject(error); }
      socket.close();
    });
    socket.addEventListener("error", () => { clearTimeout(timer); reject(new Error("WebSocket conformance connection failed")); });
  });
}

async function capabilityCases(endpoint) {
  const results = {};
  const fetchCase = await boundedFetch(endpoint, "/fetch"); results.fetch = fetchCase.status === 200 && fetchCase.json().capability === "fetch";
  const rpc = await boundedFetch(endpoint, "/rpc"); results.rpc = rpc.status === 200 && rpc.json().value === 42;
  const storage1 = await boundedFetch(endpoint, "/storage"), storage2 = await boundedFetch(endpoint, "/storage");
  results.storage = storage1.status === 200 && storage1.json().value === 1 && storage2.json().value === 2;
  const armed = await boundedFetch(endpoint, "/alarm");
  let alarmFired = false;
  for (let attempt = 0; attempt < 50 && !alarmFired; attempt += 1) { await sleep(100); const state = await boundedFetch(endpoint, "/alarm-state"); alarmFired = state.status === 200 && state.json().fired === true; }
  results.alarm = armed.status === 200 && armed.json().armed === true && alarmFired;
  results.websocket = await websocketCase(endpoint);
  const outbound = await boundedFetch(endpoint, "/outbound"); results.outbound_https = outbound.status === 200 && Number.isInteger(outbound.json().status);
  const wasm = await boundedFetch(endpoint, "/wasm"); results.wasm = wasm.status === 200 && wasm.json().value === 5;
  const asset = await boundedFetch(endpoint, "/capability-asset.txt"); results.assets = asset.status === 200 && asset.text === "agentic-celld-asset-v1\n";
  return results;
}

async function runCapabilities(runtime, timeline) {
  const errorFields = runtime.errorFields ?? {};
  const operation = (name) => `worker.run-capabilities.${name}`;
  const instanceId = `rollback-${sha256(runtime.runId).slice(0, 20)}`, operationId = `rollback-${randomBytes(8).toString("hex")}`, nonce = randomBytes(16).toString("hex");
  const { seed, markerBefore } = await withWorkerAccess(operation("worker-endpoint"), errorFields, runtime.fleetPath, async (endpoint) => {
    const seedResult = await withDriverOperation(operation("seed-rollback-marker"), errorFields, () => getWorkerCell({ endpoint, varsFile: runtime.fleet.worker_vars_file_ref, instanceId, operationId, generation: 1, nonce }));
    const markerResult = await withDriverOperation(operation("verify-rollback-marker"), errorFields, () => getWorkerCell({ endpoint, varsFile: runtime.fleet.worker_vars_file_ref, instanceId, operationId, generation: 1, nonce }));
    return { seed: seedResult, markerBefore: markerResult };
  });
  if (seed.status !== 404 || seed.body?.error?.code !== "cell.missing") throw driverOperationError(operation("seed-rollback-marker"), errorFields, "reference Worker durable rollback marker was not accepted");
  if (markerBefore.status !== 409 || markerBefore.body?.error?.code !== "cell.signature_replayed") throw driverOperationError(operation("verify-rollback-marker"), errorFields, "reference Worker did not persist the rollback marker before deployment");
  const markerIdentity = `sha256:${sha256(`${instanceId}:${operationId}:${nonce}`)}`;
  const stateBefore = `sha256:${sha256(JSON.stringify({ marker_identity: markerIdentity, status: markerBefore.status, code: markerBefore.body.error.code }))}`;
  await withDriverOperation(operation("stop-fleet-for-candidate"), errorFields, () => stopFleetForWorkerDeployment(runtime.fleetPath));
  const conformance = await withDriverOperation(operation("prepare-conformance-project"), errorFields, () => prepareWorkerConformanceProject(runtime.root));
  const candidate = await withDriverOperation(operation("deploy-candidate"), errorFields, () => deployProject(runtime, conformance.path, conformance.sha256, "qualification-candidate"));
  if ((await withDriverOperation(operation("start-candidate-fleet"), errorFields, () => startFleet(runtime.fleetPath))).status !== "READY") throw driverOperationError(operation("start-candidate-fleet"), errorFields, "conformance deployment fleet did not become ready");
  const cases = await withWorkerAccess(operation("candidate-worker-endpoint"), errorFields, runtime.fleetPath, (endpoint) => withDriverOperation(operation("capability-cases"), errorFields, () => capabilityCases(endpoint)));
  await withDriverOperation(operation("stop-fleet-for-restore"), errorFields, () => stopFleetForWorkerDeployment(runtime.fleetPath));
  const approvedProject = join(REPO_ROOT, "runtimes/celld/instance-cell");
  const approvedProjectDigest = await withDriverOperation(operation("digest-approved-reference"), errorFields, () => workerDeploymentProjectDigest(approvedProject, { deploymentKind: "approved-reference" }));
  const restored = await withDriverOperation(operation("deploy-approved-reference"), errorFields, () => deployProject(runtime, approvedProject, approvedProjectDigest, "approved-reference"));
  if ((await withDriverOperation(operation("start-approved-fleet"), errorFields, () => startFleet(runtime.fleetPath))).status !== "READY") throw driverOperationError(operation("start-approved-fleet"), errorFields, "reference deployment rollback did not become ready");
  const { replay, probe } = await withWorkerAccess(operation("restored-worker-endpoint"), errorFields, runtime.fleetPath, async (endpoint) => {
    const replayResult = await withDriverOperation(operation("verify-restored-marker"), errorFields, () => getWorkerCell({ endpoint, varsFile: runtime.fleet.worker_vars_file_ref, instanceId, operationId, generation: 1, nonce }));
    const probeResult = await withDriverOperation(operation("probe-restored-worker"), errorFields, () => probeFleetWorker(runtime.fleetPath, { endpoint }));
    return { replay: replayResult, probe: probeResult };
  });
  const stateAfter = `sha256:${sha256(JSON.stringify({ marker_identity: markerIdentity, status: replay.status, code: replay.body?.error?.code }))}`;
  const rollbackPreserved = replay.status === 409 && replay.body?.error?.code === "cell.signature_replayed"
    && stateAfter === stateBefore && probe.status === "READY"
    && restored.deployment_kind === "approved-reference" && restored.worker_digest === runtime.fleet.pins.worker_digest;
  const passed = Object.values(cases).filter(Boolean).length;
  timeline.push({ scenario: "UAT-CELLD-007", cases, previous_version: runtime.initialDeployment.version_id, candidate_version: candidate.version_id, restored_version: restored.version_id, candidate_project_sha256: candidate.project_sha256, restored_project_sha256: restored.project_sha256, conformance_sha256: conformance.sha256, marker_identity: markerIdentity, marker_before: markerBefore, marker_after: replay, rollback_marker_preserved: rollbackPreserved });
  return { assertions: [
    { id: "CELLD.007.CLAIMS", measurements: { capabilities: ADVERTISED, advertised_cases: 8, passed_cases: passed, failed_cases: 8 - passed, not_run_cases: 0 } },
    { id: "CELLD.007.ROLLBACK", measurements: { previous_digest: runtime.initialDeployment.worker_digest, restored_digest: restored.worker_digest, previous_version_id: runtime.initialDeployment.version_id, candidate_version_id: candidate.version_id, restored_version_id: restored.version_id, state_sha256_before: stateBefore, state_sha256_after: stateAfter, approved_digest_active: rollbackPreserved } },
  ], metrics: [{ name: "worker_capability_cases_passed", value: passed, unit: "cases" }], faults: [{ kind: "worker_deployment_rollback", healed: rollbackPreserved }] };
}

function inventoryCommand(operation, errorFields, program, args, options = {}) {
  return runWorkerCommand(operation, errorFields, program, args, options, "CELLD_LIVE_WORKER_INVENTORY_FAILED");
}

export function reviewedFileInventoryArgs(runtime) {
  return [
    runtime.config.base_images_dir,
    runtime.config.vm_storage_dir,
    runtime.config.agentshare_root,
    ...runtime.fleet.nodes.map((node) => node.state_dir),
    "-xdev",
    "-ignore_readdir_race",
    "-mindepth", "1",
    "-maxdepth", "3",
    "-printf", "%p|%y|%s\n",
  ];
}

function inventorySnapshot(runtime, operation = (name) => `worker.inventory.${name}`, errorFields = {}) {
  const containers = inventoryCommand(operation("docker-ps"), errorFields, "docker", ["ps", "--all", "--filter", `label=dev.agentic-sandbox.run=${runtime.runId}`, "--format", "{{.Names}}"], { timeout: 30_000 }).split(/\r?\n/).filter(Boolean).sort();
  const pids = runtime.fleet.nodes.map((node) => inventoryCommand(operation("docker-inspect-pid"), errorFields, "docker", ["inspect", "--format", "{{.State.Pid}}", node.name], { timeout: 30_000 }));
  const processTrees = runtime.fleet.nodes.map((node) => inventoryCommand(operation("docker-top"), errorFields, "docker", ["top", node.name, "-eo", "pid,ppid,comm"], { timeout: 30_000 })).sort();
  const portMaps = runtime.fleet.nodes.map((node) => inventoryCommand(operation("docker-inspect-ports"), errorFields, "docker", ["inspect", "--format", "{{json .NetworkSettings.Ports}}", node.name], { timeout: 30_000 })).sort();
  const socketTables = pids.map((pid) => inventoryCommand(operation("nsenter-socket-table"), errorFields, "sudo", ["-n", "nsenter", "--target", pid, "--net", "cat", "/proc/net/tcp", "/proc/net/tcp6", "/proc/net/udp", "/proc/net/udp6", "/proc/net/unix"], { timeout: 30_000 })).sort();
  const domains = inventoryCommand(operation("virsh-list-domains"), errorFields, "virsh", ["--connect", runtime.config.libvirt_uri, "list", "--all", "--name"], { timeout: 30_000 }).split(/\r?\n/).filter(Boolean).sort();
  const reviewedFiles = inventoryCommand(operation("find-reviewed-files"), errorFields, "find", reviewedFileInventoryArgs(runtime), { timeout: 120_000 }).split(/\r?\n/).filter(Boolean).sort();
  const containerFiles = runtime.fleet.nodes.map((node) => inventoryCommand(operation("docker-diff"), errorFields, "docker", ["diff", node.name], { timeout: 30_000 })).sort();
  return {
    containers,
    processes_sha256: sha256(JSON.stringify({ pids, process_trees: processTrees })),
    sockets_sha256: sha256(JSON.stringify({ port_maps: portMaps, socket_tables: socketTables })),
    domains,
    files_sha256: sha256(JSON.stringify({ reviewed_files: reviewedFiles, container_files: containerFiles })),
  };
}

export function excludedInventoryEffects(before, after) {
  return {
    processes_created: before.processes_sha256 === after.processes_sha256 ? 0 : 1,
    files_created: before.files_sha256 === after.files_sha256 ? 0 : 1,
    sockets_created: before.sockets_sha256 === after.sockets_sha256 ? 0 : 1,
    containers_created: JSON.stringify(before.containers) === JSON.stringify(after.containers) ? 0 : 1,
    vms_created: JSON.stringify(before.domains) === JSON.stringify(after.domains) ? 0 : 1,
  };
}

export function excludedCapabilityRejectionEvidence(result, capability, attempt, startedAt, endedAt) {
  const output = `${result.stdout ?? ""}\n${result.stderr ?? ""}`.trim();
  if (Buffer.byteLength(output) > 4096) throw new Error("excluded capability rejection exceeds the evidence bound");
  const typed = result.status !== 0 && output.includes("does not support these config keys") && output.includes(capability);
  return {
    capability,
    attempt,
    started_at: startedAt,
    ended_at: endedAt,
    exit_status: result.status,
    signal: result.signal,
    timed_out: result.error?.code === "ETIMEDOUT",
    rejection_code: typed ? "celld.unsupported_config_key" : null,
    output,
    output_sha256: `sha256:${sha256(output)}`,
    typed_rejection: typed,
  };
}

function prepareNegativeProjects(root) {
  const negativeRoot = join(root, "worker-negative");
  mkdirSync(negativeRoot, { recursive: true, mode: 0o700 });
  for (const capability of EXCLUDED) {
    const directory = join(negativeRoot, capability);
    mkdirSync(directory, { mode: 0o700 });
    writeFileSync(join(directory, "worker.mjs"), "export default { fetch() { return new Response('unexpected'); } };\n", { mode: 0o600, flag: "wx" });
    writeFileSync(join(directory, "wrangler.json"), `${JSON.stringify({ name: `negative-${capability.replaceAll("_", "-")}`, main: "worker.mjs", compatibility_date: "2026-08-14", [capability]: { enabled: true } })}\n`, { mode: 0o600, flag: "wx" });
  }
  return negativeRoot;
}

function containerNegativeProjectPath(capability, filename) {
  if (!EXCLUDED.includes(capability) || !["worker.mjs", "wrangler.json"].includes(filename)) throw new Error("negative project path is outside the excluded capability fixture");
  return `/tmp/celld-negative/${capability}/${filename}`;
}

function copyNegativeProjectsToContainer(primary, negativeRoot, operation, errorFields) {
  for (const capability of EXCLUDED) {
    for (const filename of ["worker.mjs", "wrangler.json"]) {
      const sourcePath = join(negativeRoot, capability, filename);
      const targetPath = containerNegativeProjectPath(capability, filename);
      const targetDirectory = dirname(targetPath);
      const shortName = filename === "worker.mjs" ? "worker" : "wrangler";
      runWorkerCommand(
        operation(`copy-negative-${capability}-${shortName}`),
        errorFields,
        "docker",
        ["exec", "-i", primary, "sh", "-c", `mkdir -p ${targetDirectory} && cat > ${targetPath}`],
        { timeout: 120_000, input: readFileSync(sourcePath, "utf8") },
        "CELLD_LIVE_WORKER_COPY_NEGATIVE_PROJECTS_FAILED",
      );
    }
  }
}

async function runExcluded(runtime, timeline) {
  const errorFields = runtime.errorFields ?? {};
  const operation = (name) => `worker.run-excluded.${name}`;
  const primary = runtime.fleet.nodes[0].name;
  const negativeRoot = await withDriverOperation(operation("prepare-negative-projects"), errorFields, () => prepareNegativeProjects(runtime.root));
  await runWorkerCommand(operation("reset-negative-projects"), errorFields, "docker", ["exec", primary, "sh", "-c", "rm -rf /tmp/celld-negative && mkdir -p /tmp/celld-negative"], { timeout: 120_000 }, "CELLD_LIVE_WORKER_RESET_NEGATIVE_PROJECTS_FAILED");
  await withDriverOperation(operation("copy-negative-projects"), errorFields, () => copyNegativeProjectsToContainer(primary, negativeRoot, operation, errorFields));
  const before = await withDriverOperation(operation("inventory-before"), errorFields, () => inventorySnapshot(runtime, (name) => operation(`inventory-before.${name}`), errorFields));
  let typedRejections = 0, silentSuccesses = 0;
  const codes = {};
  const attempts = [];
  for (const capability of EXCLUDED) {
    let rejected = 0;
    for (let attempt = 0; attempt < 100; attempt += 1) {
      const startedAt = new Date().toISOString();
      const result = await withDriverOperation(operation(`dry-run-${capability}`), errorFields, () => spawnSync("docker", ["exec", primary, "/usr/local/bin/celld", "deploy", `/tmp/celld-negative/${capability}`, "--dry-run"], { encoding: "utf8", shell: false, timeout: 30_000 }));
      const record = await withDriverOperation(operation(`evidence-${capability}`), errorFields, () => excludedCapabilityRejectionEvidence(result, capability, attempt, startedAt, new Date().toISOString()));
      attempts.push(record);
      if (record.typed_rejection) { typedRejections += 1; rejected += 1; }
      else silentSuccesses += 1;
    }
    codes[capability] = rejected;
  }
  const after = await withDriverOperation(operation("inventory-after"), errorFields, () => inventorySnapshot(runtime, (name) => operation(`inventory-after.${name}`), errorFields));
  const effects = excludedInventoryEffects(before, after);
  const unchanged = Object.values(effects).every((value) => value === 0);
  const diagnosis = await withDriverOperation(operation("diagnose-fleet"), errorFields, () => diagnoseFleet(runtime.fleetPath));
  const probe = await withDriverOperation(operation("probe-worker"), errorFields, () => probeFleetWorker(runtime.fleetPath));
  timeline.push({ scenario: "UAT-CELLD-008", attempts: attempts.length, typed_rejections: typedRejections, codes, attempt_records: attempts, inventory_before: before, inventory_after: after, inventory_before_sha256: `sha256:${sha256(JSON.stringify(before))}`, inventory_after_sha256: `sha256:${sha256(JSON.stringify(after))}`, effects, fleet_status: diagnosis.status, worker_status: probe.status });
  return { assertions: [
    { id: "CELLD.008.LOUD_REJECTION", measurements: { capabilities: EXCLUDED, attempts_per_capability: 100, attempts: 800, typed_rejections: typedRejections, silent_successes: silentSuccesses } },
    { id: "CELLD.008.NO_SIDE_EFFECT", measurements: { attempts: 800, ...effects, host_inventory_restored: unchanged && diagnosis.status === "READY" && probe.status === "READY" } },
  ], metrics: [{ name: "excluded_capability_rejections", value: typedRejections, unit: "requests" }], faults: [{ kind: "excluded_worker_capability_matrix", classes: EXCLUDED.length }] };
}

function artifact(path, relativePath, mimeType) {
  const bytes = readFileSync(path);
  return { path: relativePath, mime_type: mimeType, sha256: sha256(bytes), bytes: bytes.length, contains_restricted_data: false };
}

function unavailable(profile, scenarioId, runId, startedAt, reasonCode) {
  return { schema_version: OBSERVATION_SCHEMA, driver_id: DRIVER_ID, run_id: runId, scenario_id: scenarioId, started_at: startedAt, ended_at: new Date().toISOString(), mutation_started: false, prerequisites: [{ id: "CELLD_LIVE_WORKER", status: "unavailable", reason_code: reasonCode }], assertions: [], identities: { profile_id: profile.profile_id, sandbox_git: profile.expected_sandbox_git, environment_host_sha256: profile.environment.host_sha256, driver_version: DRIVER_VERSION }, metrics: [], faults: [], artifacts: [], cleanup: { status: "not_required", assertions: [] } };
}

export async function executeWorkerDriver({ scenarioId, runId, liveProfilePath, artifactDir }, dependencies = {}) {
  const startedAt = new Date().toISOString();
  const errorFields = { scenarioId };
  const profile = await withDriverOperation("worker.load-profile", errorFields, () => protectedJson(liveProfilePath, "live profile"));
  const profileErrors = await withDriverOperation("worker.validate-profile", errorFields, () => validateLiveProfile(profile));
  if (profileErrors.length) throw driverOperationError("worker.validate-profile", errorFields, profileErrors.join("; "));
  const entry = profile.drivers?.[DRIVER_ID];
  if (!entry?.enabled) return unavailable(profile, scenarioId, runId, startedAt, "CELLD_LIVE_WORKER_DRIVER_DISABLED");
  const git = await withDriverOperation("worker.git-identity", errorFields, () => dependencies.gitCommit?.() ?? run("git", ["rev-parse", "HEAD"]));
  const host = await withDriverOperation("worker.host-identity", errorFields, () => dependencies.hostname?.() ?? hostname());
  if (!SCENARIOS.has(scenarioId) || profile.run_id !== runId || profile.expected_sandbox_git !== git || profile.environment.host_sha256 !== sha256(host)) throw driverOperationError("worker.validate-identity", errorFields, "Worker live identity does not match the requested run");
  const config = await withDriverOperation("worker.load-config", errorFields, () => protectedJson(entry.config_path, "orchestration config"));
  const configErrors = await withDriverOperation("worker.validate-config", errorFields, () => validateOrchestrationConfig(config));
  if (configErrors.length || config.run_id !== runId) throw driverOperationError("worker.validate-config", errorFields, [...configErrors, "config run identity mismatch"].join("; "));
  if (scenarioId === "UAT-CELLD-009") return unavailable(profile, scenarioId, runId, startedAt, "CELLD_PER_ISOLATE_RESOURCE_ENFORCEMENT_UNAVAILABLE");
  if (process.platform !== "linux") return unavailable(profile, scenarioId, runId, startedAt, "CELLD_LIVE_WORKER_LINUX_REQUIRED");

  mkdirSync(artifactDir, { recursive: true, mode: 0o700 }); chmodSync(artifactDir, 0o700);
  const timeline = [];
  let storage = null, fleet = null, fleetPath = null, campaign = null;
  const cleanupAssertions = [];
  let cleanupStatus = "failed";
  try {
    const root = join(config.working_root, `${scenarioId.toLowerCase()}-worker`, runId);
    storage = await withDriverOperation("worker.prepare-storage-fixture", errorFields, () => prepareFixture({ fixtureProfile: "titan-single-host-storage", runId, root }));
    await withDriverOperation("worker.start-storage-fixture", errorFields, () => startFixture(storage));
    fleet = await withDriverOperation("worker.prepare-fleet", errorFields, () => prepareFleet({ storageConfigPath: join(root, "fixture.json") }));
    fleetPath = join(root, "fleet.json");
    const initialDeployment = await withDriverOperation("worker.deploy-initial-worker", errorFields, () => deployFleetWorker(fleetPath));
    if ((await withDriverOperation("worker.start-fleet", errorFields, () => startFleet(fleetPath))).status !== "READY") throw driverOperationError("worker.start-fleet", errorFields, "Worker qualification fleet is not ready");
    const runtime = { config, storage, fleet, fleetPath, runId, root, initialDeployment, errorFields };
    campaign = await withDriverOperation(scenarioId === "UAT-CELLD-007" ? "worker.run-capabilities" : "worker.run-excluded", errorFields, () => (dependencies.runScenario ?? (scenarioId === "UAT-CELLD-007" ? runCapabilities : runExcluded))(runtime, timeline));
  } finally {
    try { cleanupWorkerResources(runId); cleanupAssertions.push("exact Worker deployer removed"); } catch (error) { cleanupAssertions.push(`Worker deployer cleanup digest ${sha256(error.message)}`); }
    try { if (fleetPath && existsSync(fleetPath)) cleanupFleet(fleetPath); cleanupAssertions.push("exact Worker fleet removed"); } catch (error) { cleanupAssertions.push(`fleet cleanup digest ${sha256(error.message)}`); }
    try { if (storage) cleanupFixture(storage); cleanupAssertions.push("exact Worker storage fixture removed"); } catch (error) { cleanupAssertions.push(`storage cleanup digest ${sha256(error.message)}`); }
    cleanupStatus = cleanupAssertions.some((value) => value.includes("digest")) ? "failed" : "passed";
  }
  if (!campaign) throw driverOperationError("worker.run-campaign", errorFields, "Worker campaign produced no measurements");
  const suffix = scenarioId.toLowerCase(), evidenceName = `worker-evidence-${suffix}.json`, timelineName = `worker-timeline-${suffix}.jsonl`;
  const evidencePath = join(artifactDir, evidenceName), timelinePath = join(artifactDir, timelineName);
  const timelineBytes = timeline.map((row) => JSON.stringify(row)).join("\n");
  const evidence = { schema_version: "agentic-sandbox.celld-worker-evidence/v1", run_id: runId, scenario_id: scenarioId, measurements: Object.fromEntries(campaign.assertions.map((item) => [item.id, item.measurements])), timeline_sha256: sha256(timelineBytes) };
  writeFileSync(evidencePath, `${JSON.stringify(evidence)}\n`, { mode: 0o600, flag: "wx" });
  writeFileSync(timelinePath, `${timelineBytes}\n`, { mode: 0o600, flag: "wx" });
  const artifacts = [artifact(evidencePath, `artifacts/${evidenceName}`, "application/json"), artifact(timelinePath, `artifacts/${timelineName}`, "application/x-ndjson")];
  return { schema_version: OBSERVATION_SCHEMA, driver_id: DRIVER_ID, run_id: runId, scenario_id: scenarioId, started_at: startedAt, ended_at: new Date().toISOString(), mutation_started: true, prerequisites: [{ id: "CELLD_LIVE_WORKER", status: "available", reason_code: "CELLD_LIVE_WORKER_READY" }, { id: "CELLD_PINNED_FLEET", status: "available", reason_code: "CELLD_PINNED_FLEET_READY" }], assertions: campaign.assertions.map((item) => ({ ...item, evidence_refs: artifacts.map((entryArtifact) => entryArtifact.path) })), identities: { profile_id: profile.profile_id, sandbox_git: profile.expected_sandbox_git, environment_host_sha256: profile.environment.host_sha256, driver_version: DRIVER_VERSION }, metrics: campaign.metrics, faults: campaign.faults, artifacts, cleanup: { status: cleanupStatus, assertions: cleanupAssertions } };
}

async function main(args) {
  if (args[0] === "cleanup") {
    const configPath = resolve(argument(args, "--config"));
    const config = protectedJson(configPath, "orchestration config");
    const errors = validateOrchestrationConfig(config);
    if (errors.length || configPath !== join(config.working_root, "orchestration.json")) throw new Error([...errors, "config is not the fixed run-root path"].join("; "));
    process.stdout.write(`${JSON.stringify(cleanupWorkerResources(config.run_id))}\n`);
    return;
  }
  const observation = await executeWorkerDriver({ scenarioId: argument(args, "--scenario-id"), runId: argument(args, "--run-id"), liveProfilePath: resolve(argument(args, "--profile")), artifactDir: resolve(argument(args, "--artifact-dir")) });
  process.stdout.write(`${JSON.stringify(observation)}\n`);
}

if (process.argv[1] && SCRIPT_PATH === resolve(process.argv[1])) main(process.argv.slice(2)).catch(emitDriverError);
