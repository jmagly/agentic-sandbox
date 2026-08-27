#!/usr/bin/env node

import { spawn, spawnSync } from "node:child_process";
import { createHash, createHmac, randomBytes, randomUUID } from "node:crypto";
import {
  chmodSync,
  closeSync,
  constants,
  existsSync,
  fstatSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  rmdirSync,
  writeFileSync,
} from "node:fs";
import { Agent as HttpsAgent, request as httpsRequest } from "node:https";
import { request as httpRequest } from "node:http";
import { hostname } from "node:os";
import { basename, dirname, isAbsolute, join, resolve, sep } from "node:path";
import { connect as tlsConnect } from "node:tls";
import { fileURLToPath } from "node:url";

import {
  cleanupFleet,
  deployFleetWorker,
  diagnoseFleet,
  openFleetWorkerAccess,
  prepareFleet,
  startCallbackRelays,
  startFleet,
  validateFleetConfig,
} from "./celld-fleet-fixture.mjs";
import {
  cleanupFixture,
  prepareFixture,
  startFixture,
} from "./celld-seaweedfs-fixture.mjs";
import { annotateDriverError, driverErrorDocument, driverOperationError, emitDriverError as emitLiveDriverError, withDriverOperation } from "./celld-live-driver-error.mjs";
import {
  acquireOrchestrationInventoryLifecycle,
  commitOrchestrationInventory,
  createOrchestrationInventoryV2,
  finishOrchestrationMutation,
  isOrchestrationInventoryV2,
  loadProtectedJson,
  loadProtectedOrchestrationInventory,
  newFaultId,
  planOrchestrationMutation,
  releaseOrchestrationInventoryLifecycle,
  validateOrchestrationInventoryDocument,
  QEMU_CLEANUP_CAPTURE_ROOT,
} from "./celld-orchestration-inventory.mjs";
import { getWorkerOperation, sendWorkerCommand } from "./celld-worker-client.mjs";
import { validateLiveProfile } from "./celld-uat-live-protocol.mjs";
export { driverErrorDocument };

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
const OBSERVATION_SCHEMA = "agentic-sandbox.celld-live-observation/v1";
const CONFIG_SCHEMA = "agentic-sandbox.celld-live-orchestration/v1";
const DRIVER_ID = "celld-live-orchestration";
const DRIVER_VERSION = "celld-live-orchestration/v1";
const DISPATCH_GATE_SCHEMA = "agentic-sandbox.celld-dispatch-gate/v1";
const CRASH_PHASE_EVIDENCE_SCHEMA = "agentic-sandbox.celld-crash-phase-evidence/v1";
const CALLBACK_PATH = "/api/v2/celld/effects";
const RUN_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const IMAGE_ID = /^sha256:[0-9a-f]{64}$/;
const SCENARIOS = new Set(["UAT-CELLD-003", "UAT-CELLD-004", "UAT-CELLD-005", "UAT-CELLD-006"]);
const ACTIONS = ["provision", "start", "stop", "destroy"];
const SUBSTRATES = ["qemu", "docker"];
const CRASH_POINTS = ["before_dispatch", "during_dispatch", "after_dispatch"];
const FAULT_KINDS = new Set(["management_process_kill", "fleet_node_stop", "callback_response_loss", "callback_relay_pause"]);
const INSTANCE_CELL_SCRIPT = "agentic-instance-cell";
const INSTANCE_CELL_CLASS = "InstanceCell";
export const QEMU_CLEANUP_HELPER_PATH = "/usr/libexec/agentic-sandbox/agentic-celld-qemu-cleanup-helper";
const QEMU_CLEANUP_HELPER_NAME = basename(QEMU_CLEANUP_HELPER_PATH);
const QEMU_CLEANUP_HELPER_MAX_INPUT = 8192;
const QEMU_CLEANUP_HELPER_MAX_OUTPUT = 4096;

export class OrchestrationCleanupResidueError extends Error {
  constructor(message, options = {}) {
    super(message, options);
    this.name = "OrchestrationCleanupResidueError";
    this.exitCode = 4;
  }
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

export function celldInstanceCellScope(instanceId) {
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(instanceId ?? "")) {
    throw new Error("InstanceCell owner observation requires an exact instance UUID");
  }
  const namespace = `cells:v1:${INSTANCE_CELL_SCRIPT.length}:${INSTANCE_CELL_SCRIPT}:${INSTANCE_CELL_CLASS}`;
  const key = createHash("sha256").update(namespace).digest();
  const first = createHmac("sha256", key).update(instanceId).digest().subarray(0, 16);
  const second = createHmac("sha256", key).update(first).digest().subarray(0, 16);
  return `${INSTANCE_CELL_CLASS}:${Buffer.concat([first, second]).toString("hex")}`;
}

function canonicalJson(value) {
  if (value === null || typeof value === "boolean" || typeof value === "string") return JSON.stringify(value);
  if (typeof value === "number") {
    if (!Number.isFinite(value)) throw new Error("non-finite numbers are not valid JSON");
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value && typeof value === "object") return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
  throw new Error("unsupported JSON value");
}

export function durableEffectHistoryObservation(cell, operationIdValue, kind) {
  if (!cell || !Array.isArray(cell.history)) throw new Error("Worker cell is missing durable effect history");
  const events = cell.history.filter((event) => event?.operation_id === operationIdValue && event?.kind === kind);
  if (events.length !== 1) {
    throw annotateDriverError(new Error(`Worker cell requires exactly one durable ${kind} event for ${operationIdValue}`), {
      errorCode: events.length === 0 ? "CELLD_DURABLE_EFFECT_EVENT_MISSING" : "CELLD_DURABLE_EFFECT_EVENT_DUPLICATE",
      evidenceSha256: sha256(canonicalJson({ event_count: events.length, kind })),
    });
  }
  const event = events[0];
  if (event.document_type !== "instance-cell-event" || event.schema_version !== "1"
    || !Number.isInteger(event.sequence) || event.sequence < 1 || Number.isNaN(Date.parse(event.recorded_at ?? ""))) {
    throw new Error(`Worker cell durable ${kind} event is invalid`);
  }
  return {
    operation_id: event.operation_id,
    kind: event.kind,
    sequence: event.sequence,
    sha256: sha256(canonicalJson(event)),
    count: events.length,
  };
}

function argument(args, name) {
  const index = args.indexOf(name);
  if (index < 0 || !args[index + 1]) throw new Error(`${name} is required`);
  return args[index + 1];
}

function optionalArgument(args, name) {
  const index = args.indexOf(name);
  return index < 0 ? null : args[index + 1] ?? null;
}

function run(program, args, options = {}) {
  const result = spawnSync(program, args, { encoding: "utf8", shell: false, ...options });
  if (result.error || result.status !== 0) throw new Error(`${basename(program)} failed: ${(result.error?.message ?? result.stderr ?? "").trim()}`);
  return result.stdout.trim();
}

function available(program, args = ["--version"]) {
  return spawnSync(program, args, { encoding: "utf8", shell: false, timeout: 15_000 }).status === 0;
}

function protectedJson(path, description) {
  return loadProtectedJson(path, description);
}

function atomicJson(path, value) {
  const temporary = `${path}.tmp-${process.pid}-${randomBytes(8).toString("hex")}`;
  try {
    writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600, flag: "wx" });
    chmodSync(temporary, 0o600);
    renameSync(temporary, path);
  } finally {
    if (existsSync(temporary)) rmSync(temporary, { force: true });
  }
}

function validTimestamp(value) {
  return typeof value === "string" && Number.isFinite(Date.parse(value));
}

export function validateOrchestrationInventory(inventory, config, options = {}) {
  return validateOrchestrationInventoryDocument(inventory, config, options);
}

export function loadAuthorizedOrchestrationInventory(profile, config, expectedHostSha256) {
  if (!profileHasExactDestructiveAuthorization(profile)) throw new Error("exact-run destructive authorization is required");
  return loadProfileBoundOrchestrationInventory(profile, config, expectedHostSha256);
}

function profileHasExactDestructiveAuthorization(profile) {
  return profile.authorization?.destructive_faults === true && profile.authorization?.exact_run_owner === profile.run_id;
}

function loadProfileBoundOrchestrationInventory(profile, config, expectedHostSha256) {
  if (resolve(profile.authorization?.inventory_path ?? "") !== resolve(config.inventory_path ?? "")) throw new Error("authorization inventory path is not the fixed orchestration inventory");
  return loadProtectedOrchestrationInventory(config.inventory_path, config, { expectedHostSha256 });
}

function orchestrationInventoryNeedsDestructiveRecovery(inventory) {
  const activeFaults = inventory.faults.filter((fault) => ["applied", "heal_pending"].includes(fault.status));
  const activeResources = inventory.resources.filter((resource) => resource.status !== "removed");
  const pendingIds = [...(inventory.incomplete_mutation_ids ?? [])];
  const retainedQemuCleanup = Array.isArray(inventory.journal) && inventory.resources.some((resource) => resource.status === "removed"
    && resource.substrate === "qemu"
    && latestProviderCleanupPlan(inventory, resource.instance_id));
  return pendingIds.length > 0 || activeFaults.length > 0 || activeResources.length > 0 || retainedQemuCleanup;
}

function executable(path, description) {
  if (!isAbsolute(path) || !existsSync(path)) return `${description} is missing`;
  const metadata = lstatSync(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o111) === 0) return `${description} must be an executable regular non-symlink file`;
  return null;
}

function fileSha256(path) {
  return sha256(readFileSync(path));
}

function builtQemuCleanupHelperPath(callbackRelayBinaryPath) {
  return join(dirname(resolve(callbackRelayBinaryPath)), QEMU_CLEANUP_HELPER_NAME);
}

export function verifyQemuCleanupHelperInstallation(config) {
  try {
    if (config.qemu_cleanup_helper_path !== QEMU_CLEANUP_HELPER_PATH
        || !SHA256.test(config.qemu_cleanup_helper_sha256 ?? "")) return false;
    const builtPath = builtQemuCleanupHelperPath(config.callback_relay_binary_path);
    const built = lstatSync(builtPath);
    const installed = lstatSync(QEMU_CLEANUP_HELPER_PATH);
    if (!built.isFile() || built.isSymbolicLink() || (built.mode & 0o111) === 0
        || !installed.isFile() || installed.isSymbolicLink() || installed.nlink !== 1
        || installed.uid !== 0 || installed.gid !== 0 || (installed.mode & 0o777) !== 0o755) return false;
    const builtDigest = fileSha256(builtPath);
    return builtDigest === config.qemu_cleanup_helper_sha256 && fileSha256(QEMU_CLEANUP_HELPER_PATH) === builtDigest;
  } catch {
    return false;
  }
}

export function validateOrchestrationConfig(config, { repoRoot = REPO_ROOT } = {}) {
  const errors = [];
  if (!config || typeof config !== "object" || Array.isArray(config)) return ["config must be an object"];
  const allowed = new Set([
    "schema_version", "run_id", "working_root", "inventory_path", "management_binary_path", "agent_client_binary_path",
    "callback_relay_binary_path", "docker_image_ref", "base_images_dir",
    "qemu_cleanup_helper_path", "qemu_cleanup_helper_sha256",
    "vm_storage_dir", "agentshare_root", "libvirt_uri", "management_grpc_port",
  ]);
  for (const key of Object.keys(config)) if (!allowed.has(key)) errors.push(`config.${key} is not allowed`);
  if (config.schema_version !== CONFIG_SCHEMA) errors.push(`config.schema_version must be ${CONFIG_SCHEMA}`);
  if (!RUN_ID.test(config.run_id ?? "")) errors.push("config.run_id is invalid");
  const root = resolve(config.working_root ?? "");
  if (!isAbsolute(config.working_root ?? "") || !root.startsWith("/dev/shm/") || !root.split(sep).includes(config.run_id)) errors.push("config.working_root must be an exact-run directory below /dev/shm");
  if (resolve(config.inventory_path ?? "") !== join(root, "orchestration-inventory.json")) errors.push("config.inventory_path must be the fixed working-root file");
  const expectedManagementRoots = [resolve(repoRoot, ".celld-target"), resolve(repoRoot, "management/target")];
  const management = resolve(config.management_binary_path ?? "");
  if (!isAbsolute(config.management_binary_path ?? "") || basename(management) !== "agentic-mgmt" || !expectedManagementRoots.some((candidate) => management.startsWith(`${candidate}${sep}`))) errors.push("config.management_binary_path is outside an approved build target");
  const agentClient = resolve(config.agent_client_binary_path ?? "");
  if (!isAbsolute(config.agent_client_binary_path ?? "") || basename(agentClient) !== "agent-client" || !expectedManagementRoots.some((candidate) => agentClient.startsWith(`${candidate}${sep}`))) errors.push("config.agent_client_binary_path is outside an approved build target");
  const relay = resolve(config.callback_relay_binary_path ?? "");
  const relayRoot = resolve(repoRoot, "tools/celld-callback-relay/target");
  if (!isAbsolute(config.callback_relay_binary_path ?? "") || basename(relay) !== "agentic-celld-callback-relay" || !relay.startsWith(`${relayRoot}${sep}`)) errors.push("config.callback_relay_binary_path is outside the fixed relay target");
  if (config.qemu_cleanup_helper_path !== QEMU_CLEANUP_HELPER_PATH) errors.push("config.qemu_cleanup_helper_path must be the fixed installed root helper");
  if (!SHA256.test(config.qemu_cleanup_helper_sha256 ?? "")) errors.push("config.qemu_cleanup_helper_sha256 must bind the reviewed built helper bytes");
  if (!IMAGE_ID.test(config.docker_image_ref ?? "")) errors.push("config.docker_image_ref must be an immutable local OCI image ID");
  if (config.base_images_dir !== "/build/agentic-sandbox/base-images") errors.push("config.base_images_dir must be the reviewed Titan base-image directory");
  if (config.vm_storage_dir !== "/build/agentic-sandbox/vms") errors.push("config.vm_storage_dir must be the reviewed Titan VM directory");
  if (!isAbsolute(config.agentshare_root ?? "") || !/^\/var\/tmp\/agentic-celld-qualification-[0-9]+\/mount$/.test(resolve(config.agentshare_root))) errors.push("config.agentshare_root must be the job-scoped qualification mount");
  if (config.libvirt_uri !== "qemu:///system") errors.push("config.libvirt_uri must be qemu:///system");
  if (!Number.isSafeInteger(config.management_grpc_port) || config.management_grpc_port < 20000 || config.management_grpc_port > 59997) errors.push("config.management_grpc_port must reserve three high loopback ports");
  return errors;
}

export function prepareOrchestrationConfig({ runId, workingRoot, managementBinaryPath, agentClientBinaryPath, callbackRelayBinaryPath, dockerImageRef, agentshareRoot, managementGrpcPort = 38120, now = new Date(), host = hostname() }) {
  const resolvedWorkingRoot = resolve(workingRoot);
  const cleanupHelperBuiltPath = builtQemuCleanupHelperPath(callbackRelayBinaryPath);
  const config = {
    schema_version: CONFIG_SCHEMA,
    run_id: runId,
    working_root: resolvedWorkingRoot,
    inventory_path: join(resolvedWorkingRoot, "orchestration-inventory.json"),
    management_binary_path: resolve(managementBinaryPath),
    agent_client_binary_path: resolve(agentClientBinaryPath),
    callback_relay_binary_path: resolve(callbackRelayBinaryPath),
    qemu_cleanup_helper_path: QEMU_CLEANUP_HELPER_PATH,
    qemu_cleanup_helper_sha256: fileSha256(cleanupHelperBuiltPath),
    docker_image_ref: dockerImageRef,
    base_images_dir: "/build/agentic-sandbox/base-images",
    vm_storage_dir: "/build/agentic-sandbox/vms",
    agentshare_root: resolve(agentshareRoot),
    libvirt_uri: "qemu:///system",
    management_grpc_port: managementGrpcPort,
  };
  const errors = validateOrchestrationConfig(config);
  if (errors.length) throw new Error(errors.join("; "));
  if (!verifyQemuCleanupHelperInstallation(config)) throw new Error("installed QEMU cleanup helper is missing or does not match the exact reviewed build");
  if (existsSync(config.working_root)) throw new Error("orchestration working root already exists");
  const path = join(config.working_root, "orchestration.json");
  const inventory = createOrchestrationInventoryV2({ runId: config.run_id, workingRoot: config.working_root, hostSha256: sha256(host), now });
  const inventoryErrors = validateOrchestrationInventory(inventory, config, { expectedHostSha256: inventory.host_sha256 });
  if (inventoryErrors.length) throw new Error(inventoryErrors.join("; "));
  try {
    mkdirSync(config.working_root, { recursive: true, mode: 0o700 });
    chmodSync(config.working_root, 0o700);
    writeFileSync(path, `${JSON.stringify(config, null, 2)}\n`, { mode: 0o600, flag: "wx" });
    chmodSync(path, 0o600);
    commitOrchestrationInventory(config.inventory_path, inventory, { config });
  } catch (error) {
    rmSync(config.working_root, { recursive: true, force: true });
    throw error;
  }
  return { config, path, inventory, inventoryPath: config.inventory_path };
}

function readWorkerKey(path) {
  const values = new Map(readFileSync(path, "utf8").split(/\r?\n/).filter(Boolean).map((line) => line.split(/=(.*)/s).slice(0, 2)));
  const keyId = values.get("CELL_AUTH_KEY_ID");
  const key = values.get("CELL_AUTH_KEY");
  if (!keyId || !key || Buffer.byteLength(key) < 32) throw new Error("protected Worker keyring is invalid");
  return { keyId, key };
}

function signedCallback({ keyId, key, operationId, generation, body }) {
  const timestamp = new Date().toISOString();
  const nonce = randomBytes(16).toString("hex");
  const bodyDigest = sha256(body);
  const canonical = ["POST", CALLBACK_PATH, operationId, String(generation), timestamp, nonce, bodyDigest].join("\n");
  return {
    "x-agentic-key-id": keyId,
    "x-agentic-timestamp": timestamp,
    "x-agentic-nonce": nonce,
    "x-agentic-generation": String(generation),
    "x-agentic-operation-id": operationId,
    "x-agentic-body-sha256": bodyDigest,
    "x-agentic-signature": createHmac("sha256", key).update(canonical).digest("hex"),
  };
}

export function requestHash({ operationId, instanceId, generation, action, payload }) {
  return sha256(canonicalJson({ operation_id: operationId, instance_id: instanceId, generation, action, payload }));
}

function callbackBody(instanceId, generation, effect) {
  return JSON.stringify({ instance_id: instanceId, generation, effect });
}

function callbackRequest(context, effect, { bodyOverride } = {}) {
  const body = bodyOverride ?? callbackBody(context.instanceId, context.generation, effect);
  const headers = signedCallback({ ...context.keyring, operationId: effect.operation_id, generation: context.generation, body });
  return new Promise((resolvePromise, rejectPromise) => {
    const request = httpsRequest({
      host: context.managementHost,
      port: 8122,
      path: CALLBACK_PATH,
      method: "POST",
      servername: "management.internal",
      ca: context.ca,
      cert: context.clientCert,
      key: context.clientKey,
      rejectUnauthorized: true,
      agent: context.agent ?? false,
      timeout: 30_000,
      headers: { "content-type": "application/json", "content-length": Buffer.byteLength(body), "idempotency-key": effect.operation_id, ...headers },
    }, (response) => {
      const chunks = [];
      let bytes = 0;
      response.on("data", (chunk) => {
        bytes += chunk.length;
        if (bytes > 1024 * 1024) request.destroy(new Error("management callback response exceeds 1 MiB"));
        else chunks.push(chunk);
      });
      response.on("end", () => {
        try { resolvePromise({ status: response.statusCode, body: JSON.parse(Buffer.concat(chunks).toString("utf8")) }); }
        catch { rejectPromise(new Error("management callback response is not bounded JSON")); }
      });
    });
    request.on("timeout", () => request.destroy(new Error("management callback timed out")));
    request.on("error", rejectPromise);
    request.end(body);
  });
}

async function parallelRepeat(count, concurrency, operation) {
  let next = 0;
  const results = [];
  const workers = Array.from({ length: Math.min(count, concurrency) }, async () => {
    for (;;) {
      const index = next++;
      if (index >= count) return;
      results[index] = await operation(index);
    }
  });
  await Promise.all(workers);
  return results;
}

function sleep(ms) {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, ms));
}

async function waitFor(predicate, { timeoutMs = 300_000, intervalMs = 250, description = "condition" } = {}) {
  const deadline = Date.now() + timeoutMs;
  let last;
  while (Date.now() < deadline) {
    try {
      last = await predicate();
      if (last) return last;
    } catch (error) { last = error; }
    await sleep(intervalMs);
  }
  throw new Error(`timed out waiting for ${description}: ${last instanceof Error ? last.message : "not ready"}`);
}

export function storageGateway(config) {
  const document = JSON.parse(run("docker", ["network", "inspect", config.network.name]));
  const gateway = document?.[0]?.IPAM?.Config?.[0]?.Gateway;
  if (!/^(?:10\.|192\.168\.|172\.(?:1[6-9]|2\d|3[01])\.)/.test(gateway ?? "")) throw new Error("fleet network gateway is not private IPv4");
  return gateway;
}

export function workerEndpoint(config, nodeIndex = 0) {
  const port = run("docker", ["port", config.nodes[nodeIndex].name, "8080/tcp"]);
  const match = /^127\.0\.0\.1:(\d+)$/.exec(port.trim());
  if (!match) throw new Error("Celld Worker endpoint is not host-loopback only");
  return `http://127.0.0.1:${match[1]}`;
}

export async function replaceFleetWorkerAccess(runtime, nodeIndex) {
  if (!runtime?.fleetPath || !runtime?.fleet?.nodes?.[nodeIndex]) {
    throw new Error("Celld Worker access replacement requires an exact fleet node");
  }
  const accessFactory = runtime.openFleetWorkerAccess ?? openFleetWorkerAccess;
  const next = await accessFactory(runtime.fleetPath, { nodeIndex });
  const previous = runtime.workerAccess;
  runtime.workerAccesses ??= new Set(previous ? [previous] : []);
  if (typeof next?.close === "function") runtime.workerAccesses.add(next);
  let endpoint;
  try {
    endpoint = new URL(next?.endpoint);
  } catch {
    endpoint = null;
  }
  const port = Number(endpoint?.port);
  if (!endpoint || endpoint.protocol !== "http:" || endpoint.hostname !== "127.0.0.1"
      || endpoint.username || endpoint.password || endpoint.pathname !== "/"
      || endpoint.search || endpoint.hash || !Number.isSafeInteger(port) || port < 1 || port > 65_535
      || typeof next.close !== "function" || next.node !== runtime.fleet.nodes[nodeIndex].name) {
    if (typeof next?.close === "function") {
      try {
        await next.close();
        runtime.workerAccesses.delete(next);
      } catch (error) {
        throw new OrchestrationCleanupResidueError("invalid Celld Worker access cleanup failed", { cause: error });
      }
    }
    throw new Error("Celld Worker access replacement is not exact host-loopback access");
  }
  runtime.workerAccess = next;
  runtime.workerEndpoint = next.endpoint;
  if (previous && previous !== next) {
    await previous.close();
    runtime.workerAccesses.delete(previous);
  }
  return next;
}

function boundedInternalJson(url) {
  return new Promise((resolvePromise, rejectPromise) => {
    const request = httpRequest(url, { method: "GET", timeout: 10_000 }, (response) => {
      const chunks = [];
      let bytes = 0;
      response.on("data", (chunk) => {
        bytes += chunk.length;
        if (bytes > 4096) request.destroy(new Error("Celld owner response exceeds 4096 bytes"));
        else chunks.push(chunk);
      });
      response.on("end", () => {
        try {
          resolvePromise({ status: response.statusCode, body: JSON.parse(Buffer.concat(chunks).toString("utf8")) });
        } catch {
          rejectPromise(new Error("Celld owner response is not bounded JSON"));
        }
      });
    });
    request.on("timeout", () => request.destroy(new Error("Celld owner observation timed out")));
    request.on("error", rejectPromise);
    request.end();
  });
}

function privateIpv4(value) {
  const octets = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(value ?? "")?.slice(1).map(Number);
  if (!octets || octets.some((octet) => octet > 255)) return false;
  return octets[0] === 10
    || (octets[0] === 192 && octets[1] === 168)
    || (octets[0] === 172 && octets[1] >= 16 && octets[1] <= 31);
}

export async function observeCelldOwnership(runtime, { instanceId }, now = new Date()) {
  if (!runtime?.fleet || runtime.fleet.nodes?.length !== 3 || runtime.fleet.run_id !== runtime.runId) {
    throw new Error("Celld owner observation requires the exact three-node run fleet");
  }
  const execute = runtime.runCommand ?? run;
  const fetchInternal = runtime.fetchCelldInternal ?? boundedInternalJson;
  const scope = celldInstanceCellScope(instanceId);
  const routes = [];
  for (const node of runtime.fleet.nodes) {
    const template = `{{.State.Running}}|{{index .Config.Labels "dev.agentic-sandbox.repository"}}|{{index .Config.Labels "dev.agentic-sandbox.workflow"}}|{{index .Config.Labels "dev.agentic-sandbox.run"}}|{{index .Config.Labels "dev.agentic-sandbox.scope"}}|{{with index .NetworkSettings.Networks "${runtime.fleet.network.name}"}}{{.IPAddress}}{{end}}`;
    const fields = execute("docker", ["inspect", "--format", template, node.name], { timeout: 30_000 }).trim().split("|");
    if (fields.length !== 6
        || fields[1] !== "roctinam/agentic-sandbox"
        || fields[2] !== "celld-qualification"
        || fields[3] !== runtime.runId
        || fields[4] !== "celld-qualification"
        || !privateIpv4(fields[5])) {
      throw new Error(`refusing Celld owner observation through unowned fleet node ${node.name}`);
    }
    if (fields[0] === "false") {
      routes.push({ node, running: false, route: "stopped" });
      continue;
    }
    if (fields[0] !== "true") throw new Error("Celld fleet node has an invalid running state");
    const response = await fetchInternal(new URL(`http://${fields[5]}:8081/cell/${scope}`));
    if (!response?.body || typeof response.body !== "object" || Array.isArray(response.body)) throw new Error("Celld owner route response is invalid");
    if (response.status === 200
        && response.body.route === "local"
        && response.body.cell === scope
        && JSON.stringify(Object.keys(response.body).sort()) === JSON.stringify(["cell", "route"])) {
      routes.push({ node, running: true, route: "local" });
      continue;
    }
    if (response.status === 307
        && response.body.route === "remote"
        && typeof response.body.node === "string"
        && typeof response.body.addr === "string"
        && Number.isSafeInteger(response.body.epoch)
        && response.body.epoch > 0
        && response.body.peer_protocol === 2
        && JSON.stringify(Object.keys(response.body).sort()) === JSON.stringify(["addr", "epoch", "node", "peer_protocol", "route"])) {
      routes.push({ node, running: true, route: "remote", ownerNodeId: response.body.node, ownerAddress: response.body.addr, epoch: response.body.epoch });
      continue;
    }
    throw new Error("Celld owner route response does not match the pinned operator contract");
  }
  const local = routes.filter((route) => route.route === "local");
  const remote = routes.filter((route) => route.route === "remote");
  if (local.length !== 1 || remote.length < 1 || routes.filter((route) => route.running).length < 2) {
    throw new Error("Celld owner route has no two-node ownership quorum");
  }
  const owner = local[0].node;
  if (!remote.every((route) => route.ownerNodeId === owner.node_id && route.ownerAddress === owner.advertise && route.epoch === remote[0].epoch)) {
    throw new Error("Celld owner routes disagree on owner identity or epoch");
  }
  return {
    observed_at: now.toISOString(),
    cell_scope_sha256: sha256(scope),
    owner_target: owner.name,
    owner_target_sha256: sha256(owner.name),
    owner_node_id: owner.node_id,
    owner_node_id_sha256: sha256(owner.node_id),
    owner_epoch: remote[0].epoch,
    live_nodes: routes.filter((route) => route.running).length,
    route_agreement: true,
  };
}

function celldOwnershipEvidence(observation) {
  return {
    observed_at: observation.observed_at,
    cell_scope_sha256: observation.cell_scope_sha256,
    owner_target_sha256: observation.owner_target_sha256,
    owner_node_id_sha256: observation.owner_node_id_sha256,
    owner_epoch: observation.owner_epoch,
    live_nodes: observation.live_nodes,
    route_agreement: observation.route_agreement,
  };
}

export function managementCelldVersion(version) {
  if (!/^v?\d+\.\d+\.\d+$/.test(version ?? "")) throw new Error("Celld management version is invalid");
  return version.startsWith("v") ? version : `v${version}`;
}

export function managementEnvironment(config, fleet, managementHost, { celldEndpoint, tlsCaFile, tlsIdentityFile, operatorMtlsCn } = {}) {
  if ((tlsCaFile === undefined) !== (tlsIdentityFile === undefined)) {
    throw annotateDriverError(new Error("Celld management TLS requires both CA and client identity files"), {
      operation: "orchestration.launch-management.environment.transport",
      errorCode: "CELLD_MANAGEMENT_TRANSPORT_INVALID",
    });
  }
  if (operatorMtlsCn !== undefined && (!tlsCaFile || !/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(operatorMtlsCn))) {
    throw annotateDriverError(new Error("Celld management operator mTLS requires a bounded CN and private TLS identity"), {
      operation: "orchestration.launch-management.environment.operator-mtls",
      errorCode: "CELLD_MANAGEMENT_OPERATOR_MTLS_INVALID",
    });
  }
  if (celldEndpoint === undefined) {
    try {
      celldEndpoint = workerEndpoint(fleet);
    } catch (error) {
      throw annotateDriverError(error, {
        operation: "orchestration.launch-management.environment.worker-endpoint",
        errorCode: "CELLD_MANAGEMENT_WORKER_ENDPOINT_INVALID",
      });
    }
  }
  const stateRoot = join(fleet.run_root, "management-state");
  const secrets = join(stateRoot, "secrets");
  const sshKeys = join(secrets, "ssh-keys");
  const dispatchGates = join(stateRoot, "dispatch-gates");
  try {
    mkdirSync(secrets, { recursive: true, mode: 0o700 });
    chmodSync(secrets, 0o700);
    // The QEMU provisioner creates root-owned ephemeral key leaves below this
    // directory and removes those leaves during destroy. Keep the exact
    // qualification directory owned by the runner so fixture cleanup can reap
    // the now-empty directory without a generic privileged delete.
    mkdirSync(sshKeys, { recursive: true, mode: 0o700 });
    chmodSync(sshKeys, 0o700);
    mkdirSync(dispatchGates, { recursive: true, mode: 0o700 });
    chmodSync(dispatchGates, 0o700);
  } catch (error) {
    throw annotateDriverError(error, {
      operation: "orchestration.launch-management.environment.state-root",
      errorCode: "CELLD_MANAGEMENT_STATE_ROOT_INVALID",
    });
  }
  let workerVars;
  try {
    workerVars = readWorkerKey(fleet.worker_vars_file_ref);
  } catch (error) {
    throw annotateDriverError(error, {
      operation: "orchestration.launch-management.environment.worker-key",
      errorCode: "CELLD_MANAGEMENT_WORKER_KEY_INVALID",
    });
  }
  // AF_UNIX socket paths are capped by sockaddr_un.sun_path (108 bytes on
  // Linux, including the terminator). The Actions checkout plus the
  // scenario/fleet state root is longer than that, so keep the managed gRPC
  // socket in the already protected, exact-run orchestration root. Hashing the
  // fleet root keeps simultaneous scenario fixtures distinct while preserving
  // the same endpoint across management restarts.
  const grpcUdsPath = join(config.working_root, `grpc-${sha256(fleet.run_root).slice(0, 32)}.sock`);
  if (Buffer.byteLength(grpcUdsPath) >= 108) {
    throw driverOperationError("orchestration.launch-management.environment.grpc-uds", {
      errorCode: "CELLD_MANAGEMENT_GRPC_UDS_PATH_TOO_LONG",
    }, "qualification managed gRPC UDS path exceeds the platform boundary");
  }
  return {
    ...process.env,
    LISTEN_ADDR: `127.0.0.1:${config.management_grpc_port}`,
    SECRETS_DIR: secrets,
    AIWG_TLS_LISTEN: `${managementHost}:8122`,
    AIWG_TLS_CERT: fleet.callback.management_server_cert_file_ref,
    AIWG_TLS_KEY: fleet.callback.management_server_key_file_ref,
    AIWG_TLS_CLIENT_CA: fleet.callback.ca_file_ref,
    AIWG_TLS_CLIENT_AUTH: "required",
    AIWG_MTLS_ADMIN_ALLOWLIST: operatorMtlsCn ?? "",
    AGENTIC_CELLD_ENABLED: "1",
    AGENTIC_CELLD_ENDPOINT: celldEndpoint,
    AGENTIC_CELLD_AUTH_KEY_ID: workerVars.keyId,
    AGENTIC_CELLD_AUTH_KEY_FILE: fleet.callback.management_auth_key_file_ref,
    AGENTIC_CELLD_EFFECT_LEDGER_PATH: fleet.callback.effect_ledger_file_ref,
    AGENTIC_CELLD_QUALIFICATION_DISPATCH_GATE_DIR: dispatchGates,
    AGENTIC_CELLD_CALLBACK_MTLS_CN: fleet.callback.client_cn,
    AGENTIC_CELLD_VERSION: managementCelldVersion(fleet.pins.celld.version),
    AGENTIC_CELLD_COMMIT: fleet.pins.celld.commit,
    AGENTIC_GRPC_UDS: grpcUdsPath,
    AGENTIC_GRPC_VSOCK_PORT: "0",
    BASE_IMAGES_DIR: config.base_images_dir,
    VM_STORAGE_DIR: config.vm_storage_dir,
    AGENTSHARE_ROOT: config.agentshare_root,
    LIBVIRT_DEFAULT_URI: config.libvirt_uri,
    AGENTIC_BACKEND: "libvirt",
    AGENT_CLIENT_SOURCE_BIN: config.agent_client_binary_path,
    RUST_LOG: "info",
    ...(tlsCaFile ? { AGENTIC_CELLD_TLS_CA_FILE: tlsCaFile, AGENTIC_CELLD_TLS_CLIENT_IDENTITY_FILE: tlsIdentityFile } : {}),
  };
}

export function launchManagement(config, fleet, managementHost, celldTransport = {}) {
  const logPath = join(fleet.run_root, "management-state", "management.log");
  let environment;
  try {
    environment = managementEnvironment(config, fleet, managementHost, celldTransport);
  } catch (error) {
    throw annotateDriverError(error, {
      operation: "orchestration.launch-management.environment",
      errorCode: "CELLD_MANAGEMENT_ENVIRONMENT_INVALID",
    });
  }
  const append = (chunk) => {
    const previous = existsSync(logPath) ? readFileSync(logPath) : Buffer.alloc(0);
    const combined = Buffer.concat([previous, chunk]).subarray(-1024 * 1024);
    writeFileSync(logPath, combined, { mode: 0o600 });
  };
  const processHandle = spawn(config.management_binary_path, [], {
    cwd: REPO_ROOT,
    env: environment,
    shell: false,
    stdio: ["ignore", "pipe", "pipe"],
  });
  processHandle.once("error", () => {});
  processHandle.stdout.on("data", append);
  processHandle.stderr.on("data", append);
  if (!Number.isSafeInteger(processHandle.pid) || processHandle.pid < 1) {
    throw driverOperationError("orchestration.launch-management.spawn", {
      errorCode: "CELLD_MANAGEMENT_SPAWN_NO_PID",
    }, "management process did not report a pid");
  }
  let processStartTimeTicks;
  try {
    const stat = readFileSync(`/proc/${processHandle.pid}/stat`, "utf8");
    const fieldsAfterCommand = stat.slice(stat.lastIndexOf(")") + 2).trim().split(/\s+/);
    processStartTimeTicks = fieldsAfterCommand[19];
    if (!/^[0-9]+$/.test(processStartTimeTicks ?? "")) {
      throw driverOperationError("orchestration.launch-management.spawn-identity", {
        errorCode: "CELLD_MANAGEMENT_START_IDENTITY_UNAVAILABLE",
      }, "management process start identity is unavailable");
    }
  } catch (error) {
    if (processHandle.exitCode === null && processHandle.signalCode === null) processHandle.kill("SIGTERM");
    throw annotateDriverError(error, {
      operation: "orchestration.launch-management.spawn-identity",
      errorCode: "CELLD_MANAGEMENT_PROC_STAT_UNREADABLE",
    });
  }
  let executableSha256;
  try {
    executableSha256 = sha256(readFileSync(config.management_binary_path));
  } catch (error) {
    if (processHandle.exitCode === null && processHandle.signalCode === null) processHandle.kill("SIGTERM");
    throw annotateDriverError(error, {
      operation: "orchestration.launch-management.executable-hash",
      errorCode: "CELLD_MANAGEMENT_BINARY_UNREADABLE",
    });
  }
  processHandle.spawn_identity = {
    run_id: config.run_id,
    executable_sha256: executableSha256,
    process_start_time_ticks: processStartTimeTicks,
  };
  return {
    processHandle,
    logPath,
    managementHost,
    celldTransport: {
      celldEndpoint: environment.AGENTIC_CELLD_ENDPOINT,
      ...(celldTransport.tlsCaFile ? {
        tlsCaFile: celldTransport.tlsCaFile,
        tlsIdentityFile: celldTransport.tlsIdentityFile,
      } : {}),
      ...(celldTransport.operatorMtlsCn ? { operatorMtlsCn: celldTransport.operatorMtlsCn } : {}),
    },
    operatorMtls: celldTransport.operatorMtlsCn ? {
      cn: celldTransport.operatorMtlsCn,
      ca_file_ref: celldTransport.tlsCaFile,
      identity_file_ref: celldTransport.tlsIdentityFile,
    } : null,
  };
}

function stopManagement(management, signal = "SIGTERM") {
  if (!management?.processHandle || management.processHandle.exitCode !== null || management.processHandle.signalCode !== null) return;
  management.processHandle.kill(signal);
}

export async function stopManagementAndWait(management, signal = "SIGTERM") {
  if (!management?.processHandle || management.processHandle.exitCode !== null || management.processHandle.signalCode !== null) return;
  stopManagement(management, signal);
  try {
    await waitFor(() => management.processHandle.exitCode !== null || management.processHandle.signalCode !== null, { timeoutMs: 15_000, intervalMs: 100, description: "management process exit" });
  } catch {
    stopManagement(management, "SIGKILL");
    await waitFor(() => management.processHandle.exitCode !== null || management.processHandle.signalCode !== null, { timeoutMs: 15_000, intervalMs: 100, description: "forced management process exit" });
  }
}

export async function waitManagement(management, fleet) {
  await waitFor(() => new Promise((resolvePromise) => {
    const socket = tlsConnect({
      host: management.managementHost,
      port: 8122,
      servername: "management.internal",
      ca: readFileSync(fleet.callback.ca_file_ref),
      cert: readFileSync(fleet.callback.relay_client_cert_file_ref),
      key: readFileSync(fleet.callback.relay_client_key_file_ref),
      rejectUnauthorized: true,
    });
    socket.once("secureConnect", () => { socket.destroy(); resolvePromise(true); });
    socket.once("error", () => resolvePromise(false));
  }), { timeoutMs: 120_000, intervalMs: 250, description: "management private mTLS listener" });
}

async function restartManagement(management, config, fleet, managementHost, celldEndpoint) {
  await stopManagementAndWait(management, "SIGKILL");
  const restarted = launchManagement(config, fleet, managementHost, { ...management.celldTransport, celldEndpoint });
  await waitManagement(restarted, fleet);
  return restarted;
}

function callbackContext(fleet, managementHost, instanceId, generation) {
  return {
    instanceId,
    generation,
    managementHost,
    keyring: readWorkerKey(fleet.worker_vars_file_ref),
    ca: readFileSync(fleet.callback.ca_file_ref),
    clientCert: readFileSync(fleet.callback.relay_client_cert_file_ref),
    clientKey: readFileSync(fleet.callback.relay_client_key_file_ref),
  };
}

function operationId(prefix, substrate, generation, action, index = 0) {
  return `${prefix}-${substrate}-${generation}-${action}-${index}`.slice(0, 127);
}

function dispatchGatePaths(runtime, id) {
  const digest = sha256(id);
  const root = join(runtime.fleet.run_root, "management-state", "dispatch-gates");
  return {
    digest,
    request: join(root, `${digest}.request.json`),
    reached: join(root, `${digest}.reached.json`),
  };
}

export function prepareDispatchGate(runtime, id, phase) {
  if (!["during_dispatch", "after_dispatch"].includes(phase)) throw new Error("qualification dispatch gate phase is invalid");
  const paths = dispatchGatePaths(runtime, id);
  rmSync(paths.reached, { force: true });
  if (existsSync(paths.request)) throw new Error("qualification dispatch gate request already exists");
  atomicJson(paths.request, {
    schema_version: DISPATCH_GATE_SCHEMA,
    operation_id_sha256: paths.digest,
    phase,
  });
  return paths;
}

export async function waitDispatchGate(runtime, id, phase, expectedPid) {
  const paths = dispatchGatePaths(runtime, id);
  const reached = await waitFor(() => existsSync(paths.reached) ? protectedJson(paths.reached, "qualification dispatch gate event") : false, {
    timeoutMs: 30_000,
    intervalMs: 10,
    description: `${phase} management dispatch gate`,
  });
  const allowed = ["schema_version", "operation_id_sha256", "phase", "management_pid", "reached_at"];
  if (JSON.stringify(Object.keys(reached).sort()) !== JSON.stringify(allowed.sort())
      || reached.schema_version !== DISPATCH_GATE_SCHEMA
      || reached.operation_id_sha256 !== paths.digest
      || reached.phase !== phase
      || !Number.isSafeInteger(reached.management_pid)
      || reached.management_pid !== expectedPid
      || !validTimestamp(reached.reached_at)) {
    throw new Error("qualification dispatch gate event does not bind the exact phase and management process");
  }
  return reached;
}

export function clearDispatchGate(runtime, id) {
  const paths = dispatchGatePaths(runtime, id);
  rmSync(paths.request, { force: true });
  rmSync(paths.reached, { force: true });
}

function provisionPayload(config, substrate, name) {
  return substrate === "qemu"
    ? { name, runtime: "qemu", provider: "libvirt", start: false, agentshare: false, labels: { "agentic-run-id": config.run_id } }
    : { name, runtime: "docker", image: config.docker_image_ref, start: false, agentshare: false, labels: { "agentic-run-id": config.run_id } };
}

function persistOrchestrationInventory(runtime, now = new Date(), forcedState = null) {
  runtime.orchestrationInventory.updated_at = now.toISOString();
  runtime.orchestrationInventory.state = forcedState ?? (runtime.orchestrationInventory.resources.some((entry) => entry.status !== "removed")
    || runtime.orchestrationInventory.faults.some((entry) => entry.status !== "healed") ? "active" : "prepared");
  const errors = validateOrchestrationInventory(runtime.orchestrationInventory, runtime.config, { expectedHostSha256: runtime.orchestrationInventory.host_sha256 });
  if (errors.length) throw new Error(errors.join("; "));
  if (runtime.persistInventory) {
    runtime.persistInventory(runtime.config.inventory_path, runtime.orchestrationInventory);
  } else {
    const expectedJournalHeadSha256 = runtime.persistedJournalHeadSha256;
    commitOrchestrationInventory(runtime.config.inventory_path, runtime.orchestrationInventory, {
      config: runtime.config,
      ...(expectedJournalHeadSha256 !== undefined ? { expectedJournalHeadSha256 } : {}),
    });
  }
  runtime.persistedJournalHeadSha256 = runtime.orchestrationInventory.journal_head_sha256;
}

function planProviderResource(runtime, { instanceId, name, substrate }, now = new Date()) {
  const existing = runtime.orchestrationInventory.resources.find((entry) => entry.instance_id === instanceId);
  if (isOrchestrationInventoryV2(runtime.orchestrationInventory)) {
    if (existing && (existing.name !== name || existing.substrate !== substrate)) {
      throw new Error("provider resource identity changed after inventory persistence");
    }
    runtime.providerResources.set(instanceId, { instanceId, name, substrate });
    return existing ?? null;
  }
  if (existing) {
    if (existing.name !== name || existing.substrate !== substrate) throw new Error("provider resource identity changed after inventory persistence");
    runtime.providerResources.set(instanceId, { instanceId, name, substrate });
    return existing;
  }
  const timestamp = now.toISOString();
  const record = { scenario_id: runtime.scenarioId, instance_id: instanceId, name, substrate, status: "planned", planned_at: timestamp, updated_at: timestamp };
  runtime.orchestrationInventory.resources.push(record);
  persistOrchestrationInventory(runtime, now);
  runtime.providerResources.set(instanceId, { instanceId, name, substrate });
  return record;
}

export function observeOrchestrationProvider(runtime, { instanceId, name, substrate }, now = new Date()) {
  const owned = runtime.providerResources?.get(instanceId);
  if (!owned || owned.instanceId !== instanceId || owned.name !== name || owned.substrate !== substrate) {
    throw new Error("provider observation target is not owned by this orchestration run");
  }
  const execute = runtime.runCommand ?? run;
  const base = {
    observed_at: now.toISOString(),
    substrate,
    target_name_sha256: sha256(name),
  };
  const absent = (providerStoragePresent = false) => ({
    ...base,
    owned: true,
    present: false,
    state: "absent",
    provider_storage_present: providerStoragePresent,
    provider_identity_sha256: null,
    configuration_sha256: null,
  });

  if (substrate === "qemu") {
    const names = execute("virsh", ["-c", runtime.config.libvirt_uri, "list", "--all", "--name"], { timeout: 30_000 })
      .split(/\r?\n/)
      .filter(Boolean);
    const matches = names.filter((candidate) => candidate === name);
    const storagePresent = (runtime.pathExists ?? existsSync)(join(runtime.config.vm_storage_dir, name));
    if (matches.length === 0) return absent(storagePresent);
    if (matches.length !== 1) throw new Error("libvirt provider observation is ambiguous");
    const uuid = execute("virsh", ["-c", runtime.config.libvirt_uri, "domuuid", name], { timeout: 30_000 }).trim().toLowerCase();
    if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(uuid)) throw new Error("libvirt provider identity is invalid");
    const state = execute("virsh", ["-c", runtime.config.libvirt_uri, "domstate", name], { timeout: 30_000 }).trim().toLowerCase().replace(/\s+/g, " ");
    if (!["running", "idle", "paused", "in shutdown", "shut off", "crashed", "pmsuspended", "blocked", "nostate"].includes(state)) throw new Error("libvirt provider state is invalid");
    const xml = execute("virsh", ["-c", runtime.config.libvirt_uri, "dumpxml", "--inactive", name], { timeout: 30_000 });
    const diskSourcePaths = [...xml.matchAll(/<source\b[^>]*\bfile=(?:"([^"]+)"|'([^']+)')[^>]*>/g)]
      .map((match) => (match[1] ?? match[2])
        .replaceAll("&quot;", '"')
        .replaceAll("&apos;", "'")
        .replaceAll("&lt;", "<")
        .replaceAll("&gt;", ">")
        .replaceAll("&amp;", "&"));
    if (runtime.runId !== undefined) {
      let externalRunOwner;
      try {
        externalRunOwner = execute("virsh", ["-c", runtime.config.libvirt_uri, "desc", name, "--config", "--title"], { timeout: 30_000 }).trim();
      } catch {
        throw new Error("QEMU external run ownership metadata verification failed");
      }
      if (externalRunOwner !== `agentic-run-id=${runtime.runId}`) throw new Error("QEMU external run ownership metadata does not match the exact run");
    }
    const observation = {
      ...base,
      owned: true,
      present: true,
      state,
      provider_storage_present: storagePresent,
      provider_id: uuid,
      disk_source_paths: diskSourcePaths,
      provider_identity_sha256: sha256(`qemu:${uuid}`),
      configuration_sha256: sha256(canonicalJson({ xml, storage_path: join(runtime.config.vm_storage_dir, name) })),
    };
    if (runtime.runId !== undefined) {
      const storagePath = join(runtime.config.vm_storage_dir, name);
      const storage = lstatSync(storagePath);
      if (!storage.isDirectory() || storage.isSymbolicLink()) throw new Error("libvirt provider storage identity is not an exact directory");
      observation.storage_path = storagePath;
      observation.storage_device = String(storage.dev);
      observation.storage_inode = String(storage.ino);
      observation.storage_uid = String(storage.uid);
      observation.storage_gid = String(storage.gid);
      observation.ownership_binding_sha256 = sha256(canonicalJson({ run_id: runtime.runId, instance_id: instanceId, name, provider_id: uuid }));
      observation.provider_storage_identity_sha256 = sha256(canonicalJson({
        path: storagePath,
        device: String(storage.dev),
        inode: String(storage.ino),
        uid: storage.uid,
        gid: storage.gid,
      }));
    }
    return observation;
  }

  if (substrate === "docker") {
    const ids = execute("docker", ["ps", "--all", "--filter", `label=agentic-instance-id=${instanceId}`, "--format", "{{.ID}}"], { timeout: 30_000 })
      .split(/\r?\n/)
      .filter(Boolean);
    if (ids.length === 0) return absent();
    if (ids.length !== 1 || !/^[0-9a-f]{12,64}$/.test(ids[0])) throw new Error("Docker provider observation is missing or ambiguous");
    const inspected = execute("docker", [
      "inspect",
      "--format",
      '{{.Id}}|{{.State.Status}}|{{.Config.Image}}|{{index .Config.Labels "agentic-instance-id"}}|{{index .Config.Labels "agentic-source"}}|{{index .Config.Labels "agentic-managed-network"}}',
      ids[0],
    ], { timeout: 30_000 }).split("|");
    if (![5, 6].includes(inspected.length) || !/^[0-9a-f]{64}$/.test(inspected[0]) || inspected[3] !== instanceId || inspected[4] !== "admin-v2") {
      throw new Error("Docker provider identity is not bound to the owned management resource");
    }
    const state = inspected[1];
    if (!["created", "running", "restarting", "exited", "paused", "dead", "removing"].includes(state)) throw new Error("Docker provider state is invalid");
    const observation = {
      ...base,
      owned: true,
      present: true,
      state,
      provider_storage_present: false,
      provider_id: inspected[0],
      managed_network: inspected[5] && inspected[5] !== "<no value>" ? inspected[5] : null,
      provider_identity_sha256: sha256(`docker:${inspected[0]}`),
      configuration_sha256: sha256(canonicalJson({ image: inspected[2], instance_id: inspected[3], source: inspected[4], managed_network: inspected[5] && inspected[5] !== "<no value>" ? inspected[5] : null })),
    };
    if (runtime.runId !== undefined) {
      const providerLabels = JSON.parse(execute("docker", ["inspect", "--format", "{{json .Config.Labels}}", inspected[0]], { timeout: 30_000 }));
      if (!providerLabels || typeof providerLabels !== "object" || Array.isArray(providerLabels)
          || providerLabels["agentic-instance-id"] !== instanceId || providerLabels["agentic-source"] !== "admin-v2"
          || providerLabels["agentic-run-id"] !== runtime.runId) {
        throw new Error("Docker provider labels are not bound to the exact owned management resource");
      }
      const networkName = observation.managed_network;
      if (!/^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$/.test(networkName ?? "")) throw new Error("Docker provider has no exact managed-network binding");
      const networkParts = execute("docker", ["network", "inspect", "--format", "{{.Id}}|{{json .Labels}}", networkName], { timeout: 30_000 }).split("|");
      if (networkParts.length !== 2 || !/^[0-9a-f]{12,64}$/.test(networkParts[0])) throw new Error("Docker managed-network identity is invalid");
      const networkLabels = JSON.parse(networkParts[1]);
      if (!networkLabels || typeof networkLabels !== "object" || Array.isArray(networkLabels)
          || networkLabels["agentic-run-id"] !== runtime.runId) throw new Error("Docker managed-network labels lack exact external run ownership");
      observation.provider_labels = providerLabels;
      observation.ownership_binding_sha256 = sha256(canonicalJson({ run_id: runtime.runId, labels: providerLabels }));
      observation.managed_network_id = networkParts[0];
      observation.managed_network_labels = networkLabels;
      observation.managed_network_identity_sha256 = sha256(`docker-network:${networkParts[0]}`);
      observation.managed_network_configuration_sha256 = sha256(canonicalJson(networkLabels));
      observation.configuration_sha256 = sha256(canonicalJson({ image: inspected[2], labels: providerLabels, managed_network_id: networkParts[0] }));
    }
    return observation;
  }
  throw new Error("provider observation substrate is outside the fixed allowlist");
}

function markProviderResourceRemoved(runtime, instanceId, substrate, now = new Date()) {
  const record = runtime.orchestrationInventory.resources.find((entry) => entry.instance_id === instanceId && entry.substrate === substrate);
  if (!record) throw new Error("provider cleanup target is absent from the orchestration inventory");
  const timestamp = now.toISOString();
  record.status = "removed";
  record.removed_at = timestamp;
  record.updated_at = timestamp;
  persistOrchestrationInventory(runtime, now);
}

function providerMutationSubject(runtime, { instanceId, generation, operationId: id, action, payload }) {
  const provisionSubstrate = action === "provision"
    ? payload.runtime === "qemu" ? "qemu" : payload.runtime === "docker" ? "docker" : null
    : null;
  const owned = runtime.providerResources.get(instanceId);
  const name = action === "provision" ? payload.name : owned?.name;
  const substrate = action === "provision" ? provisionSubstrate : owned?.substrate;
  if (!/^celld-[a-z0-9-]{1,62}$/.test(name ?? "") || !SUBSTRATES.includes(substrate)
      || (owned && (owned.name !== name || owned.substrate !== substrate))) {
    throw new Error("provider mutation target is not an exact-owned orchestration resource");
  }
  runtime.providerResources.set(instanceId, { instanceId, name, substrate });
  return {
    instance_id: instanceId,
    name,
    substrate,
    operation_id: id,
    generation,
    action,
    request_sha256: requestHash({ operationId: id, instanceId, generation, action, payload }),
  };
}

function planProviderCleanup(runtime, resource, now = new Date(), allowConflictWithMutationId = null) {
  if (!isOrchestrationInventoryV2(runtime.orchestrationInventory)) return null;
  const subject = {
    instance_id: resource.instanceId,
    name: resource.name,
    substrate: resource.substrate,
    operation_id: `cleanup-${randomUUID()}`,
    generation: 1,
    action: "cleanup",
    request_sha256: sha256(canonicalJson({ run_id: runtime.runId ?? runtime.config.run_id, scenario_id: runtime.scenarioId, instance_id: resource.instanceId, action: "cleanup" })),
  };
  const planned = planOrchestrationMutation(runtime.orchestrationInventory, {
    mutation: "provider_cleanup",
    scenarioId: runtime.scenarioId,
    subjectType: "provider_resource",
    subject,
    allowConflictWithMutationId,
  }, now);
  persistOrchestrationInventory(runtime, now);
  return planned.entry;
}

function completeProviderCleanup(runtime, plan, observation, now = new Date()) {
  if (!plan) return;
  finishOrchestrationMutation(runtime.orchestrationInventory, plan, {
    outcome: "absent",
    observedIdentitySha256: null,
    observedConfigurationSha256: null,
  }, now);
  persistOrchestrationInventory(runtime, now);
}

function planFault(runtime, { kind, target }, now = new Date()) {
  if (!FAULT_KINDS.has(kind) || !/^(?:management|celld-[a-z0-9-]{1,80})$/.test(target)) throw new Error("fault target is outside the fixed orchestration allowlist");
  const timestamp = now.toISOString();
  const record = { id: randomBytes(16).toString("hex"), scenario_id: runtime.scenarioId, kind, target, status: "planned", planned_at: timestamp, updated_at: timestamp };
  runtime.orchestrationInventory.faults.push(record);
  persistOrchestrationInventory(runtime, now);
  return record;
}

function markFault(runtime, record, status, now = new Date()) {
  if (!runtime.orchestrationInventory.faults.includes(record) || !["applied", "healed"].includes(status)) throw new Error("fault record is not owned by this run");
  const timestamp = now.toISOString();
  record.status = status;
  record.updated_at = timestamp;
  if (status === "applied") record.applied_at = timestamp;
  if (status === "healed") record.healed_at = timestamp;
  persistOrchestrationInventory(runtime, now);
}

export async function resolveOrchestrationFaultTarget({ runtime, fault, plan }) {
  const subject = fault ?? plan?.subject;
  if (!subject || !FAULT_KINDS.has(subject.kind)) throw new Error("fault target resolution lacks an exact persisted subject");
  if (subject.kind === "management_process_kill") {
    if (subject.target !== "management" || !runtime.management?.processHandle || !Number.isSafeInteger(runtime.management.processHandle.pid)) {
      throw new Error("management fault target is not an exact owned process");
    }
    const spawnIdentity = runtime.management.processHandle.spawn_identity;
    if (!spawnIdentity || spawnIdentity.run_id !== runtime.runId
        || !SHA256.test(spawnIdentity.executable_sha256 ?? "")
        || !/^[0-9]+$/.test(spawnIdentity.process_start_time_ticks ?? "")) {
      throw new Error("management fault process has foreign or unverifiable run ownership");
    }
    const providerId = String(runtime.management.processHandle.pid);
    return {
      owned: true,
      provider_id: providerId,
      spawn_identity: { ...spawnIdentity },
      target_identity_sha256: sha256(canonicalJson({ pid: providerId, ...spawnIdentity })),
      target_ownership_sha256: sha256(canonicalJson(spawnIdentity)),
    };
  }
  const ownedContainers = new Set([
    ...(runtime.fleet?.nodes ?? []).map((node) => node.name),
    ...(runtime.fleet?.nodes ?? []).map((_node, index) => relayName(runtime.fleet, index)),
  ]);
  if (!ownedContainers.has(subject.target)) throw new Error("container fault target is not exact-owned");
  const execute = runtime.runCommand ?? run;
  const parts = execute("docker", ["inspect", "--format", "{{.Id}}|{{json .Config.Labels}}", subject.target], { timeout: 30_000 }).split("|");
  if (parts.length !== 2 || !/^[0-9a-f]{64}$/.test(parts[0])) throw new Error("container fault identity is invalid");
  const labels = JSON.parse(parts[1]);
  if (!labels || typeof labels !== "object" || Array.isArray(labels)) throw new Error("container fault ownership labels are invalid");
  const requiredLabels = {
    "dev.agentic-sandbox.repository": "roctinam/agentic-sandbox",
    "dev.agentic-sandbox.workflow": "celld-qualification",
    "dev.agentic-sandbox.run": runtime.runId,
    "dev.agentic-sandbox.scope": "celld-qualification",
  };
  for (const [key, expected] of Object.entries(requiredLabels)) {
    if (typeof expected !== "string" || labels[key] !== expected) {
      throw new Error(`container fault target has foreign or missing authoritative ${key} ownership label`);
    }
  }
  return {
    owned: true,
    provider_id: parts[0],
    labels,
    target_identity_sha256: sha256(`docker:${parts[0]}`),
    target_ownership_sha256: sha256(canonicalJson({ run_id: runtime.runId, target: subject.target, labels })),
  };
}

export async function observeOrchestrationFaultTarget({ runtime, plan, fault }) {
  const subject = plan?.subject ?? fault;
  if (!subject || !FAULT_KINDS.has(subject.kind)) throw new Error("fault observation lacks an exact persisted subject");
  const isHeal = plan?.mutation === "fault_heal";
  const binding = runtime.runId !== undefined ? await resolveOrchestrationFaultTarget({ runtime, fault: subject, plan }) : null;
  if (subject.kind === "management_process_kill") {
    if (subject.target !== "management" || !runtime.management?.processHandle) throw new Error("management fault observation target is not exact-owned");
    const handle = runtime.management.processHandle;
    const absent = handle.exitCode !== null || handle.signalCode !== null || handle.killed;
    return { owned: true, present: absent, ...(binding ?? {}) };
  }
  const ownedContainers = new Set([
    ...(runtime.fleet?.nodes ?? []).map((node) => node.name),
    ...(runtime.fleet?.nodes ?? []).map((_node, index) => relayName(runtime.fleet, index)),
  ]);
  if (!ownedContainers.has(subject.target)) throw new Error("container fault observation target is not exact-owned");
  const execute = runtime.runCommand ?? run;
  if (subject.kind === "callback_response_loss") {
    if (isHeal) {
      const running = execute("docker", ["inspect", "--format", "{{.State.Running}}", subject.target], { timeout: 30_000 }).trim() === "true";
      return { owned: true, present: !running, ...(binding ?? {}) };
    }
    const baseline = binding?.provider_id
      ? runtime.callbackResponseLossBaselines?.get(binding.provider_id)
      : null;
    if (baseline) {
      const logs = exactCallbackRelayLogs(runtime, binding.provider_id);
      return {
        owned: true,
        present: relayMarkerCount(logs, "Celld callback relay injected one response loss") > baseline.injected,
        ...(binding ?? {}),
      };
    }
    let logs;
    if (runtime.runCommand) {
      logs = execute("docker", ["logs", "--since", plan.recorded_at, subject.target], { timeout: 30_000 });
    } else {
      const result = spawnSync("docker", ["logs", "--since", plan.recorded_at, subject.target], { encoding: "utf8", shell: false, timeout: 30_000 });
      if (result.error || result.status !== 0) throw new Error("callback response-loss observation could not read the exact relay log");
      logs = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
    }
    return { owned: true, present: logs.includes("Celld callback relay injected one response loss"), ...(binding ?? {}) };
  }
  const format = subject.kind === "callback_relay_pause" ? "{{.State.Paused}}" : "{{.State.Running}}";
  const state = execute("docker", ["inspect", "--format", format, subject.target], { timeout: 30_000 }).trim() === "true";
  const active = subject.kind === "callback_relay_pause" ? state : !state;
  return { owned: true, present: active, ...(binding ?? {}) };
}

function exactFaultBinding(binding, description) {
  if (!binding || binding.owned !== true || !SHA256.test(binding.target_identity_sha256 ?? "")
      || !SHA256.test(binding.target_ownership_sha256 ?? "")) {
    throw new Error(`${description} lacks an exact owned fault identity binding`);
  }
  return binding;
}

function sameFaultBinding(left, right) {
  return left?.target_identity_sha256 === right?.target_identity_sha256
    && left?.target_ownership_sha256 === right?.target_ownership_sha256
    && (left?.provider_id === undefined || right?.provider_id === undefined || left.provider_id === right.provider_id);
}

function exactCallbackRelayLogs(runtime, providerId) {
  const args = ["logs", "--tail", "8192", providerId];
  if (runtime.runCommand) return runtime.runCommand("docker", args, { timeout: 30_000 });
  const result = spawnSync("docker", args, { encoding: "utf8", shell: false, timeout: 30_000, maxBuffer: 1024 * 1024 });
  if (result.error || result.status !== 0) throw new Error("callback response-loss observation could not read the exact bounded relay log");
  return `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
}

function relayMarkerCount(logs, marker) {
  return logs.split(marker).length - 1;
}

export async function signalExactCallbackResponseLoss(runtime, binding) {
  const exact = exactFaultBinding(binding, "callback response-loss signal");
  if (!/^[0-9a-f]{64}$/.test(exact.provider_id ?? "")
      || exact.target_identity_sha256 !== sha256(`docker:${exact.provider_id}`)) {
    throw new Error("callback response-loss signal lacks the persisted exact container identity");
  }
  const execute = runtime.runCommand ?? run;
  const before = exactCallbackRelayLogs(runtime, exact.provider_id);
  const armedBefore = relayMarkerCount(before, "Celld callback relay armed one response loss");
  execute("docker", ["kill", "--signal", "SIGUSR1", exact.provider_id], { timeout: 30_000 });
  const armedLogs = await waitFor(() => {
    const logs = exactCallbackRelayLogs(runtime, exact.provider_id);
    return relayMarkerCount(logs, "Celld callback relay armed one response loss") > armedBefore ? logs : false;
  }, { timeoutMs: 10_000, intervalMs: 25, description: "exact callback response-loss arming" });
  runtime.callbackResponseLossBaselines ??= new Map();
  runtime.callbackResponseLossBaselines.set(exact.provider_id, {
    armed: relayMarkerCount(armedLogs, "Celld callback relay armed one response loss"),
    injected: relayMarkerCount(armedLogs, "Celld callback relay injected one response loss"),
  });
}

export async function applyPlannedOrchestrationFault(runtime, fault, apply) {
  if (typeof apply !== "function") throw new Error("fault apply callback is required");
  if (isOrchestrationInventoryV2(runtime.orchestrationInventory)) {
    if (typeof runtime.observeFaultTarget !== "function") throw new Error("an exact fault observer is required before fault apply");
    const resolvedBinding = runtime.resolveFaultTarget
      ? exactFaultBinding(await runtime.resolveFaultTarget({ runtime, fault }), "fault authorization")
      : null;
    const subject = {
      fault_id: newFaultId(), kind: fault.kind, target: fault.target,
      ...(resolvedBinding ? {
        target_identity_sha256: resolvedBinding.target_identity_sha256,
        target_ownership_sha256: resolvedBinding.target_ownership_sha256,
      } : {}),
    };
    const planned = planOrchestrationMutation(runtime.orchestrationInventory, {
      mutation: "fault_apply",
      scenarioId: runtime.scenarioId,
      subjectType: "fault",
      subject,
    });
    persistOrchestrationInventory(runtime);
    const effectBinding = runtime.revalidateFaultTarget
      ? exactFaultBinding(await runtime.revalidateFaultTarget({ runtime, fault, plan: planned.entry, authorized: resolvedBinding }), "fault effect revalidation")
      : resolvedBinding;
    if (resolvedBinding && !sameFaultBinding(resolvedBinding, effectBinding)) throw new Error("fault effect target identity or ownership binding was substituted");
    await apply(effectBinding);
    const observation = await runtime.observeFaultTarget({ runtime, plan: planned.entry, fault: planned.materialized, subject });
    if (!observation || observation.owned !== true || observation.present !== true) {
      throw new Error("fault apply effect was not independently observed present on the exact-owned target");
    }
    if (resolvedBinding && !sameFaultBinding(resolvedBinding, observation)) throw new Error("fault terminal observation substituted the authorized identity binding");
    const completed = finishOrchestrationMutation(runtime.orchestrationInventory, planned.entry, { outcome: "applied" });
    persistOrchestrationInventory(runtime);
    return completed.materialized;
  }
  const record = planFault(runtime, fault);
  await apply();
  markFault(runtime, record, "applied");
  return record;
}

export async function healPlannedOrchestrationFault(runtime, record, heal) {
  if (typeof heal !== "function") throw new Error("fault heal callback is required");
  if (isOrchestrationInventoryV2(runtime.orchestrationInventory)) {
    if (!runtime.orchestrationInventory.faults.includes(record) || record.status !== "applied") throw new Error("fault record is not an applied exact-owned fault");
    if (typeof runtime.observeFaultTarget !== "function") throw new Error("an exact fault observer is required before fault heal");
    const planned = planOrchestrationMutation(runtime.orchestrationInventory, {
      mutation: "fault_heal",
      scenarioId: runtime.scenarioId,
      subjectType: "fault",
      subject: {
        fault_id: record.id, kind: record.kind, target: record.target,
        ...(record.target_identity_sha256 ? { target_identity_sha256: record.target_identity_sha256 } : {}),
        ...(record.target_ownership_sha256 ? { target_ownership_sha256: record.target_ownership_sha256 } : {}),
      },
    });
    persistOrchestrationInventory(runtime);
    const authorizedBinding = record.target_identity_sha256 ? {
      owned: true,
      target_identity_sha256: record.target_identity_sha256,
      target_ownership_sha256: record.target_ownership_sha256,
    } : null;
    let effectBinding;
    try {
      effectBinding = runtime.revalidateFaultTarget
        ? exactFaultBinding(await runtime.revalidateFaultTarget({ runtime, fault: record, plan: planned.entry, authorized: authorizedBinding }), "fault heal effect revalidation")
        : null;
    } catch (error) {
      throw annotateDriverError(error, { errorCode: "CELLD_FAULT_HEAL_REVALIDATION_FAILED" });
    }
    let targetTransition;
    if (authorizedBinding && !sameFaultBinding(authorizedBinding, effectBinding)) {
      if (record.kind !== "management_process_kill" || effectBinding?.spawn_identity?.run_id !== runtime.runId) {
        throw new Error("fault heal effect target identity or ownership binding was substituted");
      }
      targetTransition = {
        kind: "management_process_replacement",
        previous_identity_sha256: authorizedBinding.target_identity_sha256,
        replacement_identity_sha256: effectBinding.target_identity_sha256,
        replacement_ownership_sha256: effectBinding.target_ownership_sha256,
        replacement_provider_id: effectBinding.provider_id,
      };
    }
    await heal(effectBinding);
    let observation;
    try {
      observation = await runtime.observeFaultTarget({ runtime, plan: planned.entry, fault: record, subject: planned.entry.subject });
    } catch (error) {
      throw annotateDriverError(error, { errorCode: "CELLD_FAULT_HEAL_OBSERVATION_FAILED" });
    }
    if (!observation || observation.owned !== true || observation.present !== false) {
      throw annotateDriverError(new Error("fault heal effect was not independently observed absent on the exact-owned target"), {
        errorCode: "CELLD_FAULT_HEAL_OBSERVATION_PRESENT",
        evidenceSha256: sha256(canonicalJson({ owned: observation?.owned === true, present: observation?.present ?? null })),
      });
    }
    if (authorizedBinding && !sameFaultBinding(targetTransition ? effectBinding : authorizedBinding, observation)) {
      throw annotateDriverError(new Error("fault heal terminal observation substituted the authorized identity binding"), {
        errorCode: "CELLD_FAULT_HEAL_OBSERVATION_SUBSTITUTED",
        evidenceSha256: sha256(canonicalJson({
          target_identity_matches: (targetTransition ? effectBinding : authorizedBinding)?.target_identity_sha256 === observation.target_identity_sha256,
          target_ownership_matches: (targetTransition ? effectBinding : authorizedBinding)?.target_ownership_sha256 === observation.target_ownership_sha256,
          provider_identity_matches: effectBinding?.provider_id === undefined || observation.provider_id === undefined || effectBinding.provider_id === observation.provider_id,
        })),
      });
    }
    try {
      finishOrchestrationMutation(runtime.orchestrationInventory, planned.entry, { outcome: "healed", targetTransition });
      persistOrchestrationInventory(runtime);
    } catch (error) {
      throw annotateDriverError(error, { errorCode: "CELLD_FAULT_HEAL_JOURNAL_TERMINAL_FAILED" });
    }
    return;
  }
  await heal();
  markFault(runtime, record, "healed");
}

async function healLiveOrchestrationFaultEffect({ runtime, fault, binding }) {
  if (fault.kind === "management_process_kill") {
    const handle = runtime.management?.processHandle;
    if (!handle || handle.exitCode !== null || handle.signalCode !== null || handle.killed) {
      runtime.management = await restartManagement(
        runtime.management,
        runtime.config,
        runtime.fleet,
        runtime.managementHost,
        runtime.workerEndpoint,
      );
    }
    return;
  }
  const exact = exactFaultBinding(binding, "live cleanup fault heal");
  if (!/^[0-9a-f]{64}$/.test(exact.provider_id ?? "")) {
    throw new Error("live cleanup fault heal lacks the exact container identity");
  }
  if (fault.kind === "callback_response_loss") return;
  if (fault.kind === "callback_relay_pause") {
    run("docker", ["unpause", exact.provider_id], { timeout: 30_000 });
    return;
  }
  if (fault.kind === "fleet_node_stop") {
    run("docker", ["start", exact.provider_id], { timeout: 30_000 });
    await waitFor(() => diagnoseFleet(runtime.fleetPath).status === "READY", {
      timeoutMs: 30_000,
      intervalMs: 250,
      description: "three-node fleet cleanup heal",
    });
    return;
  }
  throw new Error("live cleanup fault kind is outside the fixed allowlist");
}

export async function cleanupOwnedOrchestrationFaults(runtime, dependencies = {}) {
  if (!runtime) return [];
  if (!isOrchestrationInventoryV2(runtime.orchestrationInventory)) {
    throw new Error("orchestration inventory v1 is read-only during fault cleanup; upgrade to v2 first");
  }
  const observeFault = dependencies.observeFaultTarget ?? runtime.observeFaultTarget;
  const healFaultEffect = dependencies.healFaultEffect ?? healLiveOrchestrationFaultEffect;
  if (typeof observeFault !== "function" || typeof healFaultEffect !== "function") {
    throw new Error("live fault cleanup requires exact observation and heal adapters");
  }
  const inventory = runtime.orchestrationInventory;
  for (const plan of inventory.journal.filter((entry) => entry.event === "planned"
    && entry.mutation === "fault_apply"
    && inventory.incomplete_mutation_ids.includes(entry.mutation_id))) {
    const fault = exactRecoveryFault(runtime, plan);
    const observed = exactRecoveryFaultObservation(
      plan.subject,
      await observeFault({ runtime, plan, fault, subject: plan.subject }),
      true,
    );
    finishOrchestrationMutation(inventory, plan, {
      event: "recovered",
      outcome: observed.active ? "applied" : "not_observed",
    });
    persistOrchestrationInventory(runtime);
  }
  const assertions = [];
  for (const fault of inventory.faults.filter((entry) => entry.status === "applied")) {
    // A management heal authorizes the replacement spawn identity. Start that
    // replacement before planning the heal so the journal records an explicit
    // same-run identity transition instead of authorizing the dead process.
    if (fault.kind === "management_process_kill") {
      await healFaultEffect({ runtime, fault, binding: null });
    }
    await healPlannedOrchestrationFault(runtime, fault, async (binding) => {
      if (fault.kind === "management_process_kill") return;
      await healFaultEffect({ runtime, fault, binding });
    });
    assertions.push(`fault ${fault.kind} ${fault.id} healed`);
  }
  if (inventory.incomplete_mutation_ids.some((mutationId) => inventory.journal.some((entry) => entry.event === "planned"
    && entry.mutation_id === mutationId
    && entry.subject_type === "fault"))) {
    throw new OrchestrationCleanupResidueError("live fault cleanup retains an incomplete fault mutation");
  }
  return assertions;
}

export async function issueCommand(runtime, { instanceId, generation, operationId: id, action, payload }) {
  const journaledAction = isOrchestrationInventoryV2(runtime.orchestrationInventory) && ACTIONS.includes(action);
  let mutationPlan = null;
  if (journaledAction) {
    const subject = providerMutationSubject(runtime, { instanceId, generation, operationId: id, action, payload });
    mutationPlan = planOrchestrationMutation(runtime.orchestrationInventory, {
      mutation: "provider_action",
      scenarioId: runtime.scenarioId,
      subjectType: "provider_resource",
      subject,
    }).entry;
    persistOrchestrationInventory(runtime);
  } else if (action === "provision") {
    const substrate = payload.runtime === "qemu" ? "qemu" : payload.runtime === "docker" ? "docker" : null;
    if (substrate && typeof payload.name === "string") planProviderResource(runtime, { instanceId, name: payload.name, substrate });
  }
  const sender = runtime.sendWorkerCommand ?? sendWorkerCommand;
  const result = await sender({ endpoint: runtime.workerEndpoint, varsFile: runtime.fleet.worker_vars_file_ref, instanceId, operationId: id, generation, action, payload });
  if (![200, 202].includes(result.status)) throw new Error(`Worker rejected ${action} with ${result.status}:${result.body?.error?.code ?? "unknown"}`);
  const effect = result.body?.effects?.find((candidate) => candidate.operation_id === id);
  if (!effect) throw new Error("Worker acknowledgement omitted the original effect identity");
  return effect;
}

export async function completeObservedProviderMutation(runtime, { effect, instanceId, generation, action, observation }, now = new Date()) {
  if (!isOrchestrationInventoryV2(runtime.orchestrationInventory)) return null;
  const operationIdValue = effect?.operation_id;
  const plan = runtime.orchestrationInventory.journal.find((entry) => entry.event === "planned"
    && runtime.orchestrationInventory.incomplete_mutation_ids.includes(entry.mutation_id)
    && entry.mutation === "provider_action"
    && entry.subject.instance_id === instanceId
    && entry.subject.generation === generation
    && entry.subject.action === action
    && entry.subject.operation_id === operationIdValue);
  if (!plan) throw new Error("provider terminal observation has no exact incomplete mutation plan");
  const record = runtime.orchestrationInventory.resources.find((resource) => resource.instance_id === instanceId);
  if (!record || record.name !== plan.subject.name || record.substrate !== plan.subject.substrate) {
    throw new Error("provider terminal observation target is not the persisted exact-owned resource");
  }
  const presentTerminal = action !== "destroy";
  const expectedStates = {
    provision: new Set(["created", "shut off"]),
    start: new Set(["running"]),
    stop: new Set(["exited", "shut off"]),
  };
  if (!observation || observation.owned !== true) {
    throw new Error("provider effect terminal observation lacks explicit exact-owned authorization");
  }
  if (action === "provision" && record.substrate === "qemu" && observation.provider_storage_present !== true) {
    throw new Error("QEMU provider provision terminal lacks exact-owned storage evidence");
  }
  if (observation.present !== presentTerminal
      || (action === "destroy" && observation.provider_storage_present !== false)
      || (presentTerminal && !expectedStates[action]?.has(observation.state))) {
    throw new Error("provider effect terminal state was not independently observed");
  }
  if (presentTerminal) {
    if (!SHA256.test(observation.provider_identity_sha256 ?? "") || !SHA256.test(observation.configuration_sha256 ?? "")) {
      throw new Error("provider effect observation lacks immutable identity and configuration digests");
    }
    if (runtime.runId !== undefined && !hasProviderEffectBinding(record.substrate, observation)) {
      throw new Error("provider effect observation lacks immutable run-owned destructive bindings");
    }
    if (action !== "provision" && record.provider_identity_sha256 && record.provider_identity_sha256 !== observation.provider_identity_sha256) {
      throw new Error("provider effect observation substituted the persisted immutable identity");
    }
    if (action !== "provision" && record.configuration_sha256 && record.configuration_sha256 !== observation.configuration_sha256) {
      throw new Error("provider effect observation substituted the persisted immutable configuration");
    }
    for (const field of [
      "ownership_binding_sha256", "managed_network_identity_sha256", "managed_network_configuration_sha256", "provider_storage_identity_sha256",
    ]) {
      if (observation[field] !== undefined && !SHA256.test(observation[field])) throw new Error(`provider effect observation ${field} is invalid`);
    }
  }
  const terminal = finishOrchestrationMutation(runtime.orchestrationInventory, plan, {
    outcome: presentTerminal ? "effect_observed" : "absent",
    observedIdentitySha256: presentTerminal ? observation.provider_identity_sha256 : null,
    observedConfigurationSha256: presentTerminal ? observation.configuration_sha256 : null,
    ...(presentTerminal ? {
      observedProviderId: observation.provider_id,
      observedOwnershipBindingSha256: observation.ownership_binding_sha256,
      observedManagedNetworkId: observation.managed_network_id,
      observedManagedNetworkIdentitySha256: observation.managed_network_identity_sha256,
      observedManagedNetworkConfigurationSha256: observation.managed_network_configuration_sha256,
      observedProviderStorageIdentitySha256: observation.provider_storage_identity_sha256,
      observedStoragePath: observation.storage_path,
      observedStorageDevice: observation.storage_device,
      observedStorageInode: observation.storage_inode,
      observedStorageUid: observation.storage_uid,
      observedStorageGid: observation.storage_gid,
    } : {}),
  }, now);
  if (presentTerminal) {
    for (const field of [
      "ownership_binding_sha256", "managed_network_identity_sha256", "managed_network_configuration_sha256", "provider_storage_identity_sha256",
      "storage_device", "storage_inode", "storage_uid", "storage_gid",
    ]) {
      if (observation[field] !== undefined) terminal.materialized[field] = observation[field];
    }
  }
  persistOrchestrationInventory(runtime, now);
  return terminal.materialized;
}

export async function waitForObservedProviderEffect(runtime, { resource, action }, {
  observeProvider = async ({ resource: ownedResource }) => observeOrchestrationProvider(runtime, ownedResource),
  timeoutMs = runtime?.providerConvergenceTimeoutMs ?? 180_000,
  intervalMs = runtime?.providerConvergenceIntervalMs ?? 250,
} = {}) {
  if (!resource || !ACTIONS.includes(action) || !SUBSTRATES.includes(resource.substrate)
      || !Number.isSafeInteger(timeoutMs) || timeoutMs < 1
      || !Number.isSafeInteger(intervalMs) || intervalMs < 0) {
    throw new Error("provider effect convergence request is invalid");
  }
  const expectedStates = {
    provision: new Set(["created", "shut off"]),
    start: new Set(["running"]),
    stop: new Set(["exited", "shut off"]),
  };
  const deadline = Date.now() + timeoutMs;
  let latest = null;
  do {
    const observation = await observeProvider({ runtime, resource, action });
    if (!observation || observation.owned !== true || observation.ambiguous === true) {
      throw new Error("provider effect convergence observation is not explicitly exact-owned");
    }
    latest = observation;
    const presentTerminal = action !== "destroy";
    const storageTerminal = action === "provision" && resource.substrate === "qemu"
      ? observation.provider_storage_present === true
      : action === "destroy"
        ? observation.provider_storage_present === false
        : true;
    if (observation.present === presentTerminal
        && storageTerminal
        && (!presentTerminal || expectedStates[action]?.has(observation.state))) {
      return observation;
    }
    if (Date.now() >= deadline) break;
    await sleep(intervalMs);
  } while (Date.now() < deadline);
  throw new Error(`timed out waiting for independently observed ${resource.substrate} ${action} provider convergence from ${latest?.state ?? "unknown"}`);
}

function exactRecoveryProviderResource(runtime, plan) {
  const subject = plan.subject;
  const resource = runtime.providerResources?.get(subject.instance_id);
  if (!resource || resource.instanceId !== subject.instance_id || resource.name !== subject.name || resource.substrate !== subject.substrate) {
    throw new Error("restart recovery provider target is not exact-owned by this orchestration run");
  }
  return resource;
}

function exactRecoveryFault(runtime, plan) {
  const subject = plan.subject;
  const fault = runtime.orchestrationInventory.faults.find((entry) => entry.id === subject.fault_id);
  if (!fault || fault.kind !== subject.kind || fault.target !== subject.target) throw new Error("restart recovery fault target is not exact-owned by this orchestration run");
  return fault;
}

function exactRecoveryProviderObservation(record, observation) {
  if (!record.provider_identity_sha256 && !record.configuration_sha256
      && observation && observation.owned === true && observation.ambiguous !== true
      && typeof observation.present === "boolean") {
    if (observation.present) return observation;
    if (observation.provider_storage_present !== true) return { ...observation, provider_storage_present: false };
  }
  return exactCleanupObservation(record, observation);
}

function hasProviderEffectBinding(substrate, observation) {
  if (!observation || observation.owned !== true || !SHA256.test(observation.provider_identity_sha256 ?? "")
      || !SHA256.test(observation.configuration_sha256 ?? "") || !SHA256.test(observation.ownership_binding_sha256 ?? "")) return false;
  if (substrate === "docker") {
    return SHA256.test(observation.managed_network_identity_sha256 ?? "")
      && SHA256.test(observation.managed_network_configuration_sha256 ?? "");
  }
  return observation.provider_storage_present === true && SHA256.test(observation.provider_storage_identity_sha256 ?? "")
    && /^(?:0|[1-9][0-9]*)$/.test(observation.storage_device ?? "")
    && /^[1-9][0-9]*$/.test(observation.storage_inode ?? "")
    && /^(?:0|[1-9][0-9]*)$/.test(observation.storage_uid ?? "")
    && /^(?:0|[1-9][0-9]*)$/.test(observation.storage_gid ?? "");
}

function copyProviderEffectBinding(record, observation) {
  for (const field of [
    "provider_id", "ownership_binding_sha256", "managed_network_id", "managed_network_identity_sha256",
    "managed_network_configuration_sha256", "provider_storage_identity_sha256", "storage_path",
    "storage_device", "storage_inode", "storage_uid", "storage_gid",
  ]) if (observation[field] !== undefined) record[field] = observation[field];
}

function faultObservationActive(observation) {
  if (!observation || observation.owned !== true
      || (typeof observation.present !== "boolean" && typeof observation.healed !== "boolean")) {
    throw new Error("restart recovery fault observation is missing or unowned");
  }
  return typeof observation.present === "boolean" ? observation.present : !observation.healed;
}

function exactRecoveryFaultObservation(subject, observation, requireBinding) {
  const active = faultObservationActive(observation);
  const bound = SHA256.test(subject?.target_identity_sha256 ?? "") && SHA256.test(subject?.target_ownership_sha256 ?? "");
  if (requireBinding && !bound) throw new Error("restart recovery refuses an unbound destructive fault plan");
  if (bound && (!sameFaultBinding(subject, observation))) throw new Error("restart recovery observed a substituted fault identity binding");
  return { observation, active };
}

function incompleteProviderMutationFor(inventory, instanceId) {
  return inventory.journal.some((entry) => entry.event === "planned"
    && inventory.incomplete_mutation_ids.includes(entry.mutation_id)
    && entry.subject_type === "provider_resource"
    && entry.subject.instance_id === instanceId);
}

function incompleteProviderActionPlanFor(inventory, instanceId) {
  const plans = inventory.journal.filter((entry) => entry.event === "planned"
    && entry.mutation === "provider_action"
    && inventory.incomplete_mutation_ids.includes(entry.mutation_id)
    && entry.subject_type === "provider_resource"
    && entry.subject.instance_id === instanceId);
  if (plans.length > 1) throw new Error("provider cleanup target has multiple incomplete provider action plans");
  return plans[0] ?? null;
}

function completeRecoveredProviderEffect(runtime, original, record, observation) {
  const terminal = finishOrchestrationMutation(runtime.orchestrationInventory, original, {
    event: "recovered",
    outcome: "effect_observed",
    observedIdentitySha256: observation.provider_identity_sha256,
    observedConfigurationSha256: observation.configuration_sha256,
    observedProviderId: observation.provider_id,
    observedOwnershipBindingSha256: observation.ownership_binding_sha256,
    observedManagedNetworkId: observation.managed_network_id,
    observedManagedNetworkIdentitySha256: observation.managed_network_identity_sha256,
    observedManagedNetworkConfigurationSha256: observation.managed_network_configuration_sha256,
    observedProviderStorageIdentitySha256: observation.provider_storage_identity_sha256,
    observedStoragePath: observation.storage_path,
    observedStorageDevice: observation.storage_device,
    observedStorageInode: observation.storage_inode,
    observedStorageUid: observation.storage_uid,
    observedStorageGid: observation.storage_gid,
  });
  copyProviderEffectBinding(terminal.materialized, observation);
  persistOrchestrationInventory(runtime);
  return terminal.materialized;
}

async function reconcileRecoveryFault(runtime, plan, fault, dependencies, { requireBinding = true } = {}) {
  const inventory = runtime.orchestrationInventory;
  const observeFault = dependencies.observeFaultTarget ?? dependencies.observeFault ?? runtime.observeFaultTarget;
  const healFault = dependencies.healFaultTarget ?? dependencies.healFault ?? runtime.healFaultTarget;
  if (typeof observeFault !== "function") throw new Error("restart recovery requires an exact fault observation adapter");
  const firstResult = exactRecoveryFaultObservation(
    plan.subject,
    await observeFault({ runtime, plan, fault, subject: plan.subject }),
    requireBinding,
  );
  if (!firstResult.active) {
    if (inventory.incomplete_mutation_ids.includes(plan.mutation_id)) {
      finishOrchestrationMutation(inventory, plan, {
        event: "recovered",
        outcome: plan.mutation === "fault_heal" ? "healed" : "not_observed",
      });
      persistOrchestrationInventory(runtime);
    }
    return;
  }
  if (typeof healFault !== "function") throw new Error("restart recovery requires an exact fault heal adapter");
  let healPlan = plan;
  if (plan.mutation === "fault_apply") {
    healPlan = inventory.journal.find((entry) => entry.event === "planned"
      && entry.mutation === "fault_heal"
      && entry.subject.fault_id === plan.subject.fault_id
      && inventory.incomplete_mutation_ids.includes(entry.mutation_id));
    if (!healPlan) {
      healPlan = planOrchestrationMutation(inventory, {
        mutation: "fault_heal",
        scenarioId: plan.scenario_id,
        subjectType: "fault",
        subject: plan.subject,
        allowConflictWithMutationId: plan.mutation_id,
      }).entry;
      persistOrchestrationInventory(runtime);
    }
    finishOrchestrationMutation(inventory, plan, { event: "recovered", outcome: "applied" });
    persistOrchestrationInventory(runtime);
  }
  await healFault({ runtime, plan: healPlan, fault, observation: firstResult.observation, subject: healPlan.subject });
  const finalResult = exactRecoveryFaultObservation(
    healPlan.subject,
    await observeFault({ runtime, plan: healPlan, fault, subject: healPlan.subject }),
    requireBinding,
  );
  if (finalResult.active) throw new Error("restart recovery fault heal did not reach an exact healed observation");
  finishOrchestrationMutation(inventory, healPlan, { event: "recovered", outcome: "healed" });
  persistOrchestrationInventory(runtime);
}

export async function recoverOrchestrationInventory(runtime, dependencies = {}) {
  const inventory = runtime?.orchestrationInventory;
  if (!inventory || !runtime?.config) throw new Error("restart recovery requires an orchestration runtime and inventory");
  const errors = validateOrchestrationInventory(inventory, runtime.config, { expectedHostSha256: inventory.host_sha256 });
  if (errors.length) throw new Error(errors.join("; "));
  if (!isOrchestrationInventoryV2(inventory)) {
    throw new Error("orchestration inventory v1 is read-only during recovery; upgrade to v2 before cleanup");
  }

  const pendingIds = [...inventory.incomplete_mutation_ids];
  const recovered = [];
  const activeFaults = inventory.faults.filter((fault) => ["applied", "heal_pending"].includes(fault.status));
  const activeResources = inventory.resources.filter((resource) => resource.status !== "removed");
  for (const record of inventory.resources.filter((resource) => resource.status === "removed" && resource.substrate === "qemu")) {
    const cleanupPlan = latestProviderCleanupPlan(inventory, record.instance_id);
    if (cleanupPlan) {
      reconcileQemuQuarantineResidue(runtime, {
        instanceId: record.instance_id,
        name: record.name,
        substrate: record.substrate,
      }, record, cleanupPlan, dependencies);
    }
  }
  if (pendingIds.length === 0 && activeFaults.length === 0 && activeResources.length === 0) {
    return { status: "PASS", run_id: inventory.run_id, schema_version: inventory.schema_version, recovered_mutation_ids: [] };
  }
  try {
    persistOrchestrationInventory(runtime, new Date(), "recovering");
  } catch (error) {
    if (error instanceof OrchestrationCleanupResidueError) throw error;
    throw new OrchestrationCleanupResidueError(`restart recovery could not acquire the inventory lifecycle exclusion: ${error.message}`, { cause: error });
  }
  try {
    for (const mutationId of pendingIds) {
      const original = inventory.journal.find((entry) => entry.event === "planned" && entry.mutation_id === mutationId);
      if (!original || inventory.journal.some((entry) => entry.event !== "planned" && entry.mutation_id === mutationId)) continue;
      if (original.subject_type === "provider_resource") {
        const resource = exactRecoveryProviderResource(runtime, original);
        const record = inventory.resources.find((entry) => entry.instance_id === resource.instanceId);
        if (!record) throw new OrchestrationCleanupResidueError("restart recovery resource is absent from the materialized inventory");
        const observe = dependencies.observeProviderResource
          ?? (async () => observeOrchestrationProvider(runtime, resource));
        const remove = dependencies.removeProviderResource
          ?? (async ({ plan, record: ownedRecord, observation }) => removeExactlyObservedProvider(runtime, resource, ownedRecord, observation, { plan }));
        const first = exactRecoveryProviderObservation(record, await observe({ runtime, plan: original, resource, record, subject: original.subject }));
        let cleanupPlan = incompleteProviderCleanupPlan(inventory, resource.instanceId);
        const newlyBoundEffect = original.mutation === "provider_action"
          && original.subject.action === "provision"
          && first.present
          && !record.provider_identity_sha256
          && hasProviderEffectBinding(resource.substrate, first);
        if (first.present && !record.provider_identity_sha256 && !cleanupPlan && !newlyBoundEffect && runtime.runId !== undefined) {
          throw new OrchestrationCleanupResidueError("restart recovery refuses an unbound provider plan before any destructive effect");
        }
        if (newlyBoundEffect) completeRecoveredProviderEffect(runtime, original, record, first);
        if (first.present) {
          if (!cleanupPlan) cleanupPlan = planProviderCleanup(runtime, resource, new Date(), newlyBoundEffect ? null : original.mutation_id);
          await remove({ runtime, plan: cleanupPlan, resource, record, observation: first });
          const finalObservation = exactRecoveryProviderObservation(record, await observe({ runtime, plan: cleanupPlan, resource, record, subject: original.subject }));
          if (finalObservation.present) throw new OrchestrationCleanupResidueError("restart recovery provider cleanup did not reach exact observed absence");
          reconcileQemuQuarantineResidue(runtime, resource, record, cleanupPlan, dependencies);
          completeProviderCleanup(runtime, cleanupPlan, finalObservation);
        } else if (cleanupPlan && inventory.incomplete_mutation_ids.includes(cleanupPlan.mutation_id)) {
          reconcileQemuQuarantineResidue(runtime, resource, record, cleanupPlan, dependencies);
          completeProviderCleanup(runtime, cleanupPlan, first);
        }
        if (!inventory.journal.some((entry) => entry.event !== "planned" && entry.mutation_id === original.mutation_id)) {
          finishOrchestrationMutation(inventory, original, {
            event: "recovered",
            outcome: "absent",
            observedIdentitySha256: null,
            observedConfigurationSha256: null,
          });
          persistOrchestrationInventory(runtime);
        }
        if (!incompleteProviderMutationFor(inventory, resource.instanceId)) runtime.providerResources.delete(resource.instanceId);
        recovered.push(mutationId);
        continue;
      }

      const fault = exactRecoveryFault(runtime, original);
      await reconcileRecoveryFault(runtime, original, fault, dependencies, { requireBinding: true });
      recovered.push(mutationId);
    }
    if (inventory.resources.some((resource) => resource.status !== "removed")) {
      await cleanupOwnedProviderResources(runtime, dependencies);
    }
    for (const fault of inventory.faults.filter((entry) => entry.status === "applied")) {
      const observeFault = dependencies.observeFaultTarget ?? dependencies.observeFault ?? runtime.observeFaultTarget;
      const healFault = dependencies.healFaultTarget ?? dependencies.healFault ?? runtime.healFaultTarget;
      if (typeof observeFault !== "function" || typeof healFault !== "function") {
        throw new Error("restart recovery requires exact fault observation and heal adapters");
      }
      const subject = {
        fault_id: fault.id,
        kind: fault.kind,
        target: fault.target,
        ...(fault.target_identity_sha256 ? { target_identity_sha256: fault.target_identity_sha256 } : {}),
        ...(fault.target_ownership_sha256 ? { target_ownership_sha256: fault.target_ownership_sha256 } : {}),
      };
      const firstResult = exactRecoveryFaultObservation(
        subject,
        await observeFault({ runtime, plan: null, fault, subject }),
        false,
      );
      const plan = planOrchestrationMutation(inventory, {
        mutation: "fault_heal",
        scenarioId: fault.scenario_id,
        subjectType: "fault",
        subject,
      }).entry;
      persistOrchestrationInventory(runtime);
      if (firstResult.active) await healFault({ runtime, plan, fault, observation: firstResult.observation, subject });
      const finalResult = firstResult.active
        ? exactRecoveryFaultObservation(subject, await observeFault({ runtime, plan, fault, subject }), false)
        : firstResult;
      if (finalResult.active) throw new Error("restart recovery fault heal did not reach an exact healed observation");
      finishOrchestrationMutation(inventory, plan, { event: "recovered", outcome: "healed" });
      persistOrchestrationInventory(runtime);
      recovered.push(plan.mutation_id);
    }
  } catch (error) {
    markCleanupResidue(runtime, error);
  }
  return { status: "PASS", run_id: inventory.run_id, schema_version: inventory.schema_version, recovered_mutation_ids: recovered };
}

async function terminalCallback(runtime, instanceId, generation, effect) {
  const context = callbackContext(runtime.fleet, runtime.managementHost, instanceId, generation);
  return waitFor(async () => {
    const response = await callbackRequest(context, effect);
    if (response.status !== 200 && response.status !== 202) throw new Error(`callback returned ${response.status}:${response.body?.error?.code ?? "unknown"}`);
    return ["succeeded", "failed", "rejected"].includes(response.body?.status) ? response : false;
  }, { timeoutMs: effect.action === "provision" ? 900_000 : 180_000, intervalMs: 500, description: `${effect.action} provider terminal state` });
}

async function waitCellEffect(runtime, instanceId, generation, operationIdValue, acceptedStatuses) {
  return waitFor(async () => {
    const cell = await getWorkerOperation({ endpoint: runtime.workerEndpoint, varsFile: runtime.fleet.worker_vars_file_ref, instanceId, operationId: operationIdValue, generation });
    if (cell.status !== 200) return false;
    const effect = cell.body.effects?.find((candidate) => candidate.operation_id === operationIdValue);
    return effect && acceptedStatuses.includes(effect.status) ? { cell: cell.body, effect } : false;
  }, { timeoutMs: 900_000, intervalMs: 250, description: `Worker effect ${operationIdValue}` });
}

function providerTerminalErrorCode(substrate, action, terminalCode) {
  const suffix = String(terminalCode ?? "unknown")
    .toUpperCase()
    .replace(/[^A-Z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .slice(0, 64) || "UNKNOWN";
  return `CELLD_PROVIDER_TERMINAL_${substrate.toUpperCase()}_${action.toUpperCase()}_${suffix}`;
}

async function runOneEffectCampaign(runtime, { prefix, substrate, instanceId, generation, action, payload, repeats = 0 }) {
  let phase = "issue";
  try {
    const id = operationId(prefix, substrate, generation, action);
    const effect = await issueCommand(runtime, { instanceId, generation, operationId: id, action, payload });
    phase = "terminal-callback";
    const terminal = await terminalCallback(runtime, instanceId, generation, effect);
    if (terminal.body.status !== "succeeded") {
      throw annotateDriverError(
        new Error(`${substrate} ${action} did not succeed: ${terminal.body.terminal_code ?? "unknown"}`),
        { errorCode: providerTerminalErrorCode(substrate, action, terminal.body.terminal_code) },
      );
    }
    let replayStatuses = [];
    if (repeats > 0) {
      phase = "replay";
      const context = callbackContext(runtime.fleet, runtime.managementHost, instanceId, generation);
      context.agent = new HttpsAgent({ keepAlive: true, maxSockets: 24, ca: context.ca, cert: context.clientCert, key: context.clientKey, rejectUnauthorized: true });
      try {
        replayStatuses = await parallelRepeat(repeats, 24, async () => {
          const replay = await callbackRequest(context, effect);
          const resultPresent = Object.hasOwn(replay.body ?? {}, "result") && Object.hasOwn(terminal.body, "result");
          return {
            status: replay.status,
            management_operation_id_matches: replay.body?.management_operation_id === terminal.body.management_operation_id,
            terminal_status_matches: replay.body?.status === terminal.body.status,
            terminal_code_matches: replay.body?.terminal_code === terminal.body.terminal_code,
            result_matches: resultPresent && canonicalJson(replay.body.result) === canonicalJson(terminal.body.result),
            provider_dispatch_count_matches: replay.body?.provider_dispatch_count === terminal.body.provider_dispatch_count,
          };
        });
      } finally {
        context.agent.destroy();
      }
    }
    phase = "worker-terminal";
    const cell = await waitCellEffect(runtime, instanceId, generation, id, ["succeeded"]);
    let providerObservation = null;
    if (ACTIONS.includes(action)) {
      phase = "provider-observation";
      const resource = runtime.providerResources.get(instanceId);
      if (!resource || resource.substrate !== substrate) throw new Error("provider terminal observation target is absent from the exact-run inventory");
      providerObservation = await waitForObservedProviderEffect(runtime, { resource, action });
      phase = "inventory-terminal";
      await completeObservedProviderMutation(runtime, { effect, instanceId, generation, action, observation: providerObservation });
    }
    return { id, effect, terminal: terminal.body, replayStatuses, cell: cell.cell, providerObservation };
  } catch (error) {
    throw annotateDriverError(error, {
      operation: `orchestration.provider.${substrate}.${action}.${phase}`,
      errorCode: `CELLD_PROVIDER_CAMPAIGN_${substrate.toUpperCase()}_${action.toUpperCase()}`,
    });
  }
}

async function runUat003(runtime, timeline) {
  const managementIds = new Set();
  const replayCases = [];
  const collisionCases = [];
  for (const substrate of SUBSTRATES) {
    const instanceId = randomUUID();
    const name = `celld-${substrate}-${sha256(`${runtime.runId}:003`).slice(0, 12)}`;
    const payloads = {
      provision: provisionPayload(runtime.config, substrate, name), start: {}, stop: {}, destroy: {},
    };
    for (const action of ACTIONS) {
      if (action === "provision") planProviderResource(runtime, { instanceId, name, substrate });
      const providerBefore = observeOrchestrationProvider(runtime, { instanceId, name, substrate });
      const result = await runOneEffectCampaign(runtime, { prefix: "uat003", substrate, instanceId, generation: 1, action, payload: payloads[action], repeats: 10_000 });
      const providerAfter = observeOrchestrationProvider(runtime, { instanceId, name, substrate });
      if (!result.terminal.management_operation_id || managementIds.has(result.terminal.management_operation_id)) throw new Error("provider operation identity was missing or reused across effects");
      managementIds.add(result.terminal.management_operation_id);
      const collisionPayload = { ...payloads[action], collision_probe: true };
      const collisionEffect = { ...result.effect, request_hash: requestHash({ operationId: result.id, instanceId, generation: 1, action, payload: collisionPayload }), payload: collisionPayload };
      const collision = await callbackRequest(callbackContext(runtime.fleet, runtime.managementHost, instanceId, 1), collisionEffect);
      const postCollisionReplay = await callbackRequest(callbackContext(runtime.fleet, runtime.managementHost, instanceId, 1), result.effect);
      const postCollisionCell = await waitCellEffect(runtime, instanceId, 1, result.id, ["succeeded"]);
      const providerPostCollision = observeOrchestrationProvider(runtime, { instanceId, name, substrate });
      const replayCase = {
        substrate,
        action,
        operation_id_sha256: sha256(result.id),
        management_operation_id_sha256: sha256(result.terminal.management_operation_id),
        replay_count: result.replayStatuses.length,
        replay_http_200: result.replayStatuses.filter((replay) => replay.status === 200).length,
        replay_management_operation_matches: result.replayStatuses.filter((replay) => replay.management_operation_id_matches).length,
        replay_terminal_status_matches: result.replayStatuses.filter((replay) => replay.terminal_status_matches).length,
        replay_terminal_code_matches: result.replayStatuses.filter((replay) => replay.terminal_code_matches).length,
        replay_result_matches: result.replayStatuses.filter((replay) => replay.result_matches).length,
        replay_provider_dispatch_count_matches: result.replayStatuses.filter((replay) => replay.provider_dispatch_count_matches).length,
        effect_records: result.cell.effects.filter((effect) => effect.operation_id === result.id).length,
        provider_dispatch_count: result.terminal.provider_dispatch_count,
        provider_before: providerBefore,
        provider_after: providerAfter,
      };
      const collisionCase = {
        substrate,
        action,
        operation_id_sha256: sha256(result.id),
        response_status: collision.status,
        response_code: collision.body?.error?.code ?? "missing",
        post_collision_replay_status: postCollisionReplay.status,
        post_collision_terminal_matches: postCollisionReplay.body?.status === result.terminal.status,
        effect_records_before: result.cell.effects.filter((effect) => effect.operation_id === result.id).length,
        effect_records_after: postCollisionCell.cell.effects.filter((effect) => effect.operation_id === result.id).length,
        provider_dispatch_count_before: result.terminal.provider_dispatch_count,
        provider_dispatch_count_after_observed: Number.isInteger(postCollisionReplay.body?.provider_dispatch_count),
        provider_dispatch_count_after: postCollisionReplay.body?.provider_dispatch_count ?? 0,
        provider_before_collision: providerAfter,
        provider_after_collision: providerPostCollision,
      };
      replayCases.push(replayCase);
      collisionCases.push(collisionCase);
      timeline.push({ scenario: "UAT-CELLD-003", kind: "replay_case", ...replayCase });
      timeline.push({ scenario: "UAT-CELLD-003", kind: "collision_case", ...collisionCase });
    }
  }
  return {
    assertions: [
      { id: "CELLD.003.ONE_EFFECT", measurements: { cases: replayCases } },
      { id: "CELLD.003.COLLISION", measurements: { cases: collisionCases } },
    ],
    faults: [{ kind: "operation_identity_collision", attempts: collisionCases.length }],
    metrics: [{ name: "callback_replays", value: 80_000, unit: "requests" }],
  };
}

async function runUat004(runtime, timeline) {
  const cases = [];
  const providerCases = [];
  let recoveryPhase = "setup";
  try {
    for (const substrate of SUBSTRATES) {
    const instanceId = randomUUID();
    const name = `celld-recovery-${substrate}-${sha256(`${runtime.runId}:004`).slice(0, 8)}`;
    recoveryPhase = `${substrate}-setup-provision`;
    await runOneEffectCampaign(runtime, { prefix: "uat004-setup", substrate, instanceId, generation: 1, action: "provision", payload: provisionPayload(runtime.config, substrate, name) });
    recoveryPhase = `${substrate}-setup-start`;
    await runOneEffectCampaign(runtime, { prefix: "uat004-setup", substrate, instanceId, generation: 1, action: "start", payload: {} });
    recoveryPhase = `${substrate}-provider-baseline`;
    const providerObservationBefore = observeOrchestrationProvider(runtime, { instanceId, name, substrate });
    for (const crashPoint of CRASH_POINTS) {
      for (let index = 0; index < 100; index += 1) {
        const id = operationId("uat004", substrate, 1, crashPoint, index);
        const providerBeforeFault = observeOrchestrationProvider(runtime, { instanceId, name, substrate });
        const ownershipBefore = await observeCelldOwnership(runtime, { instanceId });
        const managementPid = runtime.management.processHandle.pid;
        let commandSentAt = null;
        let acknowledgedAt = null;
        let effect = null;
        let phaseEvidence = null;
        let managementFault = null;
        let nodeFault = null;
        let ownershipAfterLoss = null;
        let recoveryMs = null;
        let campaignError = null;
        try {
          recoveryPhase = `${substrate}-${crashPoint}-management-fault`;
          if (crashPoint === "before_dispatch") {
            managementFault = await applyPlannedOrchestrationFault(runtime, { kind: "management_process_kill", target: "management" }, () => stopManagementAndWait(runtime.management, "SIGKILL"));
            phaseEvidence = {
              schema_version: CRASH_PHASE_EVIDENCE_SCHEMA,
              operation_id_sha256: sha256(id),
              phase: crashPoint,
              observer: "management_process_absent",
              management_pid: managementPid,
              reached_at: managementFault.applied_at,
            };
          } else {
            prepareDispatchGate(runtime, id, crashPoint);
          }

          commandSentAt = new Date().toISOString();
          recoveryPhase = `${substrate}-${crashPoint}-issue-command`;
          effect = await issueCommand(runtime, {
            instanceId,
            generation: 1,
            operationId: id,
            action: "observe",
            payload: { substrate, crash_point: crashPoint, trial: index + 1 },
          });
          acknowledgedAt = new Date().toISOString();

          if (crashPoint !== "before_dispatch") {
            recoveryPhase = `${substrate}-${crashPoint}-dispatch-gate`;
            const reached = await waitDispatchGate(runtime, id, crashPoint, managementPid);
            phaseEvidence = {
              schema_version: CRASH_PHASE_EVIDENCE_SCHEMA,
              operation_id_sha256: reached.operation_id_sha256,
              phase: reached.phase,
              observer: "management_dispatch_gate",
              management_pid: reached.management_pid,
              reached_at: reached.reached_at,
            };
            managementFault = await applyPlannedOrchestrationFault(runtime, { kind: "management_process_kill", target: "management" }, () => stopManagementAndWait(runtime.management, "SIGKILL"));
            clearDispatchGate(runtime, id);
          }

          const faultStarted = Date.parse(managementFault.applied_at);
          recoveryPhase = `${substrate}-${crashPoint}-owner-fault`;
          nodeFault = await applyPlannedOrchestrationFault(runtime, { kind: "fleet_node_stop", target: ownershipBefore.owner_target }, (target) => run("docker", ["stop", "--time", "5", target.provider_id], { timeout: 30_000 }));
          const fallbackIndex = runtime.fleet.nodes.findIndex((node) => node.name !== ownershipBefore.owner_target);
          if (fallbackIndex < 0) throw new Error("owner-loss campaign has no surviving Worker endpoint");
          await replaceFleetWorkerAccess(runtime, fallbackIndex);
          recoveryPhase = `${substrate}-${crashPoint}-owner-takeover`;
          ownershipAfterLoss = await waitFor(async () => {
            const observation = await observeCelldOwnership(runtime, { instanceId });
            return observation.owner_target !== ownershipBefore.owner_target && observation.owner_epoch > ownershipBefore.owner_epoch
              ? observation
              : false;
          }, { timeoutMs: 30_000, intervalMs: 250, description: "Celld owner takeover and epoch advance" });

          recoveryPhase = `${substrate}-${crashPoint}-management-restart`;
          runtime.management = await restartManagement(runtime.management, runtime.config, runtime.fleet, runtime.managementHost, runtime.workerEndpoint);
          await healPlannedOrchestrationFault(runtime, managementFault, async () => {});
          recoveryPhase = `${substrate}-${crashPoint}-terminal-callback`;
          const terminal = await terminalCallback(runtime, instanceId, 1, effect);
          recoveryPhase = `${substrate}-${crashPoint}-worker-terminal`;
          const cell = await waitCellEffect(runtime, instanceId, 1, id, ["succeeded"]);
          recoveryMs = Date.now() - faultStarted;
          if (terminal.body.status !== "succeeded") throw new Error("restart trial did not converge to success");
          effect = { ...effect, terminal, cell };
        } catch (error) {
          campaignError = annotateDriverError(error, {
            operation: `orchestration.uat004.${recoveryPhase}`,
            errorCode: `CELLD_RECOVERY_CAMPAIGN_${recoveryPhase.toUpperCase().replace(/[^A-Z0-9]+/g, "_").slice(0, 96)}`,
          });
          throw campaignError;
        } finally {
          try {
            clearDispatchGate(runtime, id);
            if (runtime.management.processHandle.exitCode !== null || runtime.management.processHandle.signalCode !== null || runtime.management.processHandle.killed) {
              recoveryPhase = `${substrate}-${crashPoint}-finally-management-restart`;
              runtime.management = await restartManagement(runtime.management, runtime.config, runtime.fleet, runtime.managementHost, runtime.workerEndpoint);
            }
            recoveryPhase = `${substrate}-${crashPoint}-finally-management-heal`;
            if (managementFault?.status === "applied") await healPlannedOrchestrationFault(runtime, managementFault, async () => {});
            if (nodeFault?.status === "applied") {
              recoveryPhase = `${substrate}-${crashPoint}-finally-owner-heal-authorize`;
              await healPlannedOrchestrationFault(runtime, nodeFault, async (target) => {
                recoveryPhase = `${substrate}-${crashPoint}-finally-owner-heal-provider-start`;
                run("docker", ["start", target.provider_id], { timeout: 30_000 });
                recoveryPhase = `${substrate}-${crashPoint}-finally-owner-heal-diagnosis`;
                let latestDiagnosis = null;
                try {
                  await waitFor(() => {
                    latestDiagnosis = diagnoseFleet(runtime.fleetPath);
                    return latestDiagnosis.status === "READY";
                  }, { timeoutMs: 30_000, intervalMs: 250, description: "three-node fleet heal" });
                } catch (error) {
                  throw annotateDriverError(error, {
                    operation: `orchestration.uat004.${recoveryPhase}`,
                    errorCode: latestDiagnosis?.failure?.reason_code ?? "CELLD_DIAGNOSIS_OWNER_HEAL_NOT_READY",
                    evidenceSha256: latestDiagnosis?.failure?.evidence_sha256,
                  });
                }
                recoveryPhase = `${substrate}-${crashPoint}-finally-owner-heal-terminal-observation`;
              });
            }
          } catch (error) {
            throw annotateDriverError(error, {
              operation: `orchestration.uat004.${recoveryPhase}`,
              errorCode: `CELLD_RECOVERY_CLEANUP_${recoveryPhase.toUpperCase().replace(/[^A-Z0-9]+/g, "_").slice(0, 96)}`,
              campaignError,
            });
          }
        }

        recoveryPhase = `${substrate}-${crashPoint}-healed-ownership`;
        const ownershipAfterHeal = await waitFor(async () => {
          const observation = await observeCelldOwnership(runtime, { instanceId });
          return observation.live_nodes === 3 ? observation : false;
        }, { timeoutMs: 30_000, intervalMs: 250, description: "healed three-node owner observation" });
        recoveryPhase = `${substrate}-${crashPoint}-healed-control`;
        const baseline = await runOneEffectCampaign(runtime, {
          prefix: `uat004-heal-${crashPoint}-${index + 1}`,
          substrate,
          instanceId,
          generation: 1,
          action: "observe",
          payload: { substrate, crash_point: crashPoint, trial: index + 1, healed_control: true },
        });
        if (baseline.terminal.provider_dispatch_count !== 1) throw new Error("healed control crossed the provider dispatch boundary more than once");
        const providerAfterHeal = observeOrchestrationProvider(runtime, { instanceId, name, substrate });
        const record = {
          substrate,
          crash_point: crashPoint,
          trial: index + 1,
          operation_id_sha256: sha256(id),
          command_sent_at: commandSentAt,
          acknowledged_at: acknowledgedAt,
          acknowledged: true,
          phase_evidence: phaseEvidence,
          management_fault_applied_at: managementFault.applied_at,
          management_fault_id_sha256: sha256(managementFault.id),
          owner_fault_applied_at: nodeFault.applied_at,
          owner_fault_id_sha256: sha256(nodeFault.id),
          independently_faulted: true,
          terminal_status: effect.terminal.body.status,
          effect_records: effect.cell.cell.effects.filter((candidate) => candidate.operation_id === id).length,
          provider_dispatch_count: effect.terminal.body.provider_dispatch_count,
          recovery_ms: recoveryMs,
          fault_target_sha256: sha256(ownershipBefore.owner_target),
          owner_before: celldOwnershipEvidence(ownershipBefore),
          owner_after_loss: celldOwnershipEvidence(ownershipAfterLoss),
          owner_after_heal: celldOwnershipEvidence(ownershipAfterHeal),
          baseline_after_heal_succeeded: baseline.terminal.status === "succeeded",
          baseline_provider_dispatch_count: baseline.terminal.provider_dispatch_count,
          provider_before_fault: providerBeforeFault,
          provider_after_heal: providerAfterHeal,
        };
        cases.push(record);
        timeline.push({ scenario: "UAT-CELLD-004", kind: "recovery_trial", ...record });
      }
    }
    const providerObservationAfter = observeOrchestrationProvider(runtime, { instanceId, name, substrate });
    const providerCase = { substrate, provider_before: providerObservationBefore, provider_after: providerObservationAfter };
    providerCases.push(providerCase);
    timeline.push({ scenario: "UAT-CELLD-004", kind: "provider_inventory", ...providerCase });
    await runOneEffectCampaign(runtime, { prefix: "uat004-clean", substrate, instanceId, generation: 1, action: "stop", payload: {} });
    await runOneEffectCampaign(runtime, { prefix: "uat004-clean", substrate, instanceId, generation: 1, action: "destroy", payload: {} });
    }
    const diagnosis = diagnoseFleet(runtime.fleetPath);
    return {
      assertions: [
        { id: "CELLD.004.NO_LOSS", measurements: { cases } },
        { id: "CELLD.004.RECOVERY", measurements: { cases, provider_cases: providerCases, components_healthy: diagnosis.status === "READY", inventory_restored: diagnosis.membership?.running === 3 } },
      ],
      faults: CRASH_POINTS.map((kind) => ({
        kind: `owner_and_management_${kind}`,
        acknowledged_intents: 200,
        fault_batches: 200,
        management_faults: 200,
        owner_faults: 200,
        independent_per_intent: true,
      })),
      metrics: [{ name: "recovery_samples", value: cases.length, unit: "trials" }],
    };
  } catch (error) {
    throw annotateDriverError(error, {
      operation: `orchestration.uat004.${recoveryPhase}`,
      errorCode: `CELLD_RECOVERY_CAMPAIGN_${recoveryPhase.toUpperCase().replace(/[^A-Z0-9]+/g, "_").slice(0, 96)}`,
    });
  }
}

function relayName(fleet, nodeIndex = 0) {
  return `${fleet.nodes[nodeIndex].name}-callback-relay`;
}

async function runUat005(runtime, timeline) {
  const cases = [];
  for (const substrate of SUBSTRATES) {
    const instanceId = randomUUID();
    const name = `celld-loss-${substrate}-${sha256(`${runtime.runId}:005`).slice(0, 10)}`;
    for (let generation = 1; generation <= 100; generation += 1) {
      const payloads = { provision: provisionPayload(runtime.config, substrate, name), start: {}, stop: {}, destroy: {} };
      for (const action of ACTIONS) {
        let responseLossPhase = "plan-resource";
        try {
          if (action === "provision") planProviderResource(runtime, { instanceId, name, substrate });
          responseLossPhase = "provider-before";
          const providerBefore = observeOrchestrationProvider(runtime, { instanceId, name, substrate });
          const id = operationId("uat005", substrate, generation, action);
          const started = Date.now();
          let originalEffect;
          let unknown;
          let terminal;
          responseLossPhase = "fault-apply";
          const responseLossFault = await applyPlannedOrchestrationFault(runtime, { kind: "callback_response_loss", target: relayName(runtime.fleet) }, async (target) => {
            responseLossPhase = "response-loss-arm";
            await signalExactCallbackResponseLoss(runtime, target);
            responseLossPhase = "issue-command";
            originalEffect = await issueCommand(runtime, { instanceId, generation, operationId: id, action, payload: payloads[action] });
            responseLossPhase = "worker-terminal";
            terminal = await waitCellEffect(runtime, instanceId, generation, id, ["succeeded", "failed", "rejected"]);
            responseLossPhase = "durable-unknown-observation";
            unknown = durableEffectHistoryObservation(terminal.cell, id, "effect_unknown");
            responseLossPhase = "fault-terminal-observation";
          });
          responseLossPhase = "terminal-status";
          if (terminal.effect.status !== "succeeded") throw new Error(`${substrate} ${action} response-loss recovery failed`);
          responseLossPhase = "management-replay";
          const managementReplay = await callbackRequest(callbackContext(runtime.fleet, runtime.managementHost, instanceId, generation), originalEffect);
          responseLossPhase = "provider-after";
          const providerAfter = await waitForObservedProviderEffect(runtime, {
            resource: { instanceId, name, substrate },
            action,
          });
          responseLossPhase = "provider-mutation-terminal";
          await completeObservedProviderMutation(runtime, { effect: originalEffect, instanceId, generation, action, observation: providerAfter });
          const record = {
            substrate,
            action,
            trial: generation,
            operation_id_sha256: sha256(id),
            original_id_match: unknown.operation_id === id && terminal.effect.operation_id === id,
            replacement_id_observed: unknown.operation_id !== id || terminal.effect.operation_id !== id,
            effect_records: terminal.cell.effects.filter((effect) => effect.operation_id === id).length,
            unknown_event_count: unknown.count,
            unknown_event_sequence: unknown.sequence,
            unknown_event_sha256: unknown.sha256,
            management_replay_status: managementReplay.status,
            management_replay_terminal_matches: managementReplay.body?.status === terminal.effect.status,
            provider_dispatch_count_observed: Number.isInteger(managementReplay.body?.provider_dispatch_count),
            provider_dispatch_count: managementReplay.body?.provider_dispatch_count ?? 0,
            attempts: terminal.effect.attempts,
            unknown_observed: unknown.kind === "effect_unknown",
            convergence_ms: Date.now() - started,
            provider_before: providerBefore,
            provider_after: providerAfter,
          };
          cases.push(record);
          timeline.push({ scenario: "UAT-CELLD-005", kind: "response_loss_trial", ...record });
          responseLossPhase = "fault-heal";
          await healPlannedOrchestrationFault(runtime, responseLossFault, async () => {});
        } catch (error) {
          const phase = responseLossPhase.toUpperCase().replace(/[^A-Z0-9]+/g, "_");
          throw annotateDriverError(error, {
            operation: `orchestration.uat005.${substrate}-${action}-${responseLossPhase}`,
            errorCode: `CELLD_RESPONSE_LOSS_${substrate.toUpperCase()}_${action.toUpperCase()}_${phase}`.slice(0, 160),
          });
        }
      }
    }
  }
  const proxyHealed = run("docker", ["inspect", "--format", "{{.State.Running}}", relayName(runtime.fleet)]) === "true";
  return {
    assertions: [
      { id: "CELLD.005.ORIGINAL_ID", measurements: { cases } },
      { id: "CELLD.005.NO_SECOND_EFFECT", measurements: { cases, proxy_healed: proxyHealed } },
    ],
    faults: [{ kind: "post_effect_response_loss", signal: "SIGUSR1", trials: cases.length, healed: proxyHealed }],
    metrics: [{ name: "unknown_convergence_samples", value: cases.length, unit: "trials" }],
  };
}

async function runUat006(runtime, timeline) {
  const staleCases = [];
  const activeCases = [];
  for (const substrate of SUBSTRATES) {
    const instanceId = randomUUID();
    const name = `celld-fence-${substrate}-${sha256(`${runtime.runId}:006`).slice(0, 9)}`;
    for (const action of ACTIONS) await runOneEffectCampaign(runtime, { prefix: "uat006-old", substrate, instanceId, generation: 1, action, payload: action === "provision" ? provisionPayload(runtime.config, substrate, name) : {} });
    await runOneEffectCampaign(runtime, { prefix: "uat006-active", substrate, instanceId, generation: 2, action: "provision", payload: provisionPayload(runtime.config, substrate, name) });
    const providerBefore = observeOrchestrationProvider(runtime, { instanceId, name, substrate });
    const partitionFault = await applyPlannedOrchestrationFault(runtime, { kind: "callback_relay_pause", target: relayName(runtime.fleet) }, (target) => run("docker", ["pause", target.provider_id]));
    let future;
    try {
      for (const action of ["stop", "destroy"]) {
        for (let index = 0; index < 100; index += 1) {
          const id = operationId("uat006-stale", substrate, 1, action, index);
          const payload = {};
          const effect = { operation_id: id, request_hash: requestHash({ operationId: id, instanceId, generation: 1, action, payload }), action, generation: 1, payload, status: "pending", attempts: 0, retry_at: null, terminal_code: null, management_operation_id: null };
          const response = await callbackRequest(callbackContext(runtime.fleet, runtime.managementHost, instanceId, 1), effect);
          staleCases.push({
            substrate,
            action,
            trial: index + 1,
            operation_id_sha256: sha256(id),
            response_status: response.status,
            response_code: response.body?.error?.code ?? "missing",
            provider_dispatch_count_delta_observed: Number.isInteger(response.body?.provider_dispatch_count_delta),
            provider_dispatch_count_delta: response.body?.provider_dispatch_count_delta ?? -1,
          });
        }
      }
      future = await sendWorkerCommand({ endpoint: runtime.workerEndpoint, varsFile: runtime.fleet.worker_vars_file_ref, instanceId, operationId: operationId("uat006-future", substrate, 4, "destroy"), generation: 4, action: "destroy", payload: {} });
      if (future.status !== 409 || future.body?.error?.code !== "cell.generation_fenced") throw new Error("future generation was not fenced by the active cell");
    } finally {
      await healPlannedOrchestrationFault(runtime, partitionFault, (target) => run("docker", ["unpause", target.provider_id]));
    }
    const baseline = await runOneEffectCampaign(runtime, {
      prefix: "uat006-heal",
      substrate,
      instanceId,
      generation: 2,
      action: "observe",
      payload: { substrate, healed_control: true },
    });
    const providerAfter = observeOrchestrationProvider(runtime, { instanceId, name, substrate });
    await runOneEffectCampaign(runtime, { prefix: "uat006-clean", substrate, instanceId, generation: 2, action: "destroy", payload: {} });
    const partitionHealed = run("docker", ["inspect", "--format", "{{.State.Running}}", relayName(runtime.fleet)]) === "true";
    const activeCase = {
      substrate,
      future_response_status: future.status,
      future_response_code: future.body.error.code,
      provider_before: providerBefore,
      provider_after: providerAfter,
      partition_applied: true,
      partition_healed: partitionHealed,
      baseline_after_heal_succeeded: baseline.terminal.status === "succeeded",
      baseline_provider_dispatch_count: baseline.terminal.provider_dispatch_count,
    };
    activeCases.push(activeCase);
    for (const entry of staleCases.filter((candidate) => candidate.substrate === substrate)) {
      entry.provider_before = providerBefore;
      entry.provider_after = providerAfter;
      timeline.push({ scenario: "UAT-CELLD-006", kind: "stale_trial", ...entry });
    }
    timeline.push({ scenario: "UAT-CELLD-006", kind: "active_generation_case", ...activeCase });
  }
  return {
    assertions: [
      { id: "CELLD.006.PRE_PROVIDER", measurements: { cases: staleCases } },
      { id: "CELLD.006.ACTIVE_SAFE", measurements: { cases: activeCases } },
    ],
    faults: [{ kind: "callback_network_partition", controller: "relay_pause", healed: activeCases.every((entry) => entry.partition_healed) }],
    metrics: [{ name: "stale_generation_attempts", value: staleCases.length, unit: "requests" }],
  };
}

function artifact(path, relativePath, mimeType) {
  const bytes = readFileSync(path);
  return { path: relativePath, mime_type: mimeType, sha256: sha256(bytes), bytes: bytes.length, contains_restricted_data: false };
}

function cleanupProviderResourcesLegacy(runtime) {
  if (!runtime) return [];
  const assertions = [];
  for (const resource of runtime.providerResources.values()) {
    if (!/^celld-[a-z0-9-]{1,62}$/.test(resource.name) || !/^[0-9a-f-]{36}$/.test(resource.instanceId)) throw new Error("refusing an unsafe provider cleanup identity");
    const cleanupPlan = planProviderCleanup(runtime, resource);
    if (resource.substrate === "docker") {
      const ids = run("docker", ["ps", "--all", "--filter", `label=agentic-instance-id=${resource.instanceId}`, "--format", "{{.ID}}"])
        .split(/\r?\n/).filter(Boolean);
      for (const id of ids) {
        const network = run("docker", ["inspect", "--format", "{{ index .Config.Labels \"agentic-managed-network\" }}", id]);
        run("docker", ["rm", "--force", "--volumes", id]);
        if (network && network !== "<no value>") {
          const inspect = spawnSync("docker", ["network", "inspect", network], { encoding: "utf8", shell: false });
          if (inspect.status === 0) run("docker", ["network", "rm", network]);
        }
      }
      const remaining = run("docker", ["ps", "--all", "--filter", `label=agentic-instance-id=${resource.instanceId}`, "--format", "{{.ID}}"]).split(/\r?\n/).filter(Boolean);
      if (remaining.length !== 0) throw new Error("Docker provider cleanup did not reach exact absence");
      if (cleanupPlan) completeProviderCleanup(runtime, cleanupPlan, { present: false });
      else markProviderResourceRemoved(runtime, resource.instanceId, resource.substrate);
      runtime.providerResources.delete(resource.instanceId);
      assertions.push(`Docker provider identity ${sha256(resource.instanceId)} absent`);
      continue;
    }
    const domain = spawnSync("virsh", ["-c", runtime.config.libvirt_uri, "dominfo", resource.name], { encoding: "utf8", shell: false });
    const directory = join(runtime.config.vm_storage_dir, resource.name);
    if (domain.status === 0 || existsSync(directory)) {
      run(join(REPO_ROOT, "scripts/destroy-vm.sh"), [resource.name, "--force"], { env: { ...process.env, AGENTIC_BACKEND: "libvirt", LIBVIRT_DEFAULT_URI: runtime.config.libvirt_uri, VM_STORAGE_DIR: runtime.config.vm_storage_dir }, timeout: 180_000 });
    }
    const remainingDomain = spawnSync("virsh", ["-c", runtime.config.libvirt_uri, "dominfo", resource.name], { encoding: "utf8", shell: false });
    if (remainingDomain.status === 0 || existsSync(directory)) throw new Error("QEMU provider cleanup did not reach exact absence");
    if (cleanupPlan) completeProviderCleanup(runtime, cleanupPlan, { present: false });
    else markProviderResourceRemoved(runtime, resource.instanceId, resource.substrate);
    runtime.providerResources.delete(resource.instanceId);
    assertions.push(`QEMU provider identity ${sha256(resource.instanceId)} absent`);
  }
  return assertions;
}

function exactCleanupObservation(resource, observation) {
  if (!observation || typeof observation.present !== "boolean" || observation.owned !== true || observation.ambiguous === true) {
    throw new OrchestrationCleanupResidueError("provider cleanup observation is ambiguous or foreign");
  }
  if (!observation.present) {
    if (observation.provider_storage_present !== false) {
      throw new OrchestrationCleanupResidueError("provider domain is absent while owned storage residue remains");
    }
    return observation;
  }
  if (!SHA256.test(observation.provider_identity_sha256 ?? "") || !SHA256.test(observation.configuration_sha256 ?? "")) {
    throw new OrchestrationCleanupResidueError("provider cleanup observation lacks immutable identity evidence");
  }
  if (resource.provider_identity_sha256 && observation.provider_identity_sha256 !== resource.provider_identity_sha256) {
    throw new OrchestrationCleanupResidueError("provider cleanup observed a substituted immutable identity");
  }
  if (resource.configuration_sha256 && observation.configuration_sha256 !== resource.configuration_sha256) {
    throw new OrchestrationCleanupResidueError("provider cleanup observed a substituted immutable configuration");
  }
  for (const field of [
    "ownership_binding_sha256", "managed_network_identity_sha256", "managed_network_configuration_sha256", "provider_storage_identity_sha256",
    "storage_device", "storage_inode", "storage_uid", "storage_gid",
  ]) {
    if (resource[field] && observation[field] !== resource[field]) {
      throw new OrchestrationCleanupResidueError(`provider cleanup observed a substituted ${field} binding`);
    }
  }
  return observation;
}

async function destroyExactlyObservedProvider(runtime, resource, observation) {
  if (resource.substrate === "docker") {
    if (!/^[0-9a-f]{64}$/.test(observation.provider_id ?? "")) {
      throw new OrchestrationCleanupResidueError("Docker cleanup lacks the exact observed container identity");
    }
    if (observation.managed_network_id !== null && observation.managed_network_id !== undefined
        && !/^[0-9a-f]{12,64}$/.test(observation.managed_network_id)) {
      throw new OrchestrationCleanupResidueError("Docker cleanup observed an unsafe managed-network identity");
    }
    run("docker", ["rm", "--force", "--volumes", observation.provider_id]);
    if (observation.managed_network_id) run("docker", ["network", "rm", observation.managed_network_id]);
    return;
  }
  throw new OrchestrationCleanupResidueError("provider cleanup substrate is outside the fixed allowlist");
}

function validateExactQemuStorage(resource, observation) {
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(observation.provider_id ?? "")) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup lacks the exact observed libvirt UUID");
  }
  if (!isAbsolute(observation.storage_path ?? "") || resolve(observation.storage_path) !== observation.storage_path
      || basename(observation.storage_path) !== resource.name) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup storage path is not the exact provider directory");
  }
  let metadata;
  try {
    metadata = lstatSync(observation.storage_path);
  } catch (error) {
    throw new OrchestrationCleanupResidueError(`QEMU cleanup cannot verify the exact storage directory: ${error.message}`, { cause: error });
  }
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup storage path is not a protected exact directory");
  }
  if (String(metadata.dev) !== observation.storage_device || String(metadata.ino) !== observation.storage_inode
      || String(metadata.uid) !== observation.storage_uid || String(metadata.gid) !== observation.storage_gid) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup storage path no longer has its persisted exact owner/device/inode binding");
  }
  if (!Array.isArray(observation.disk_source_paths) || observation.disk_source_paths.length === 0) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup lacks confined XML disk source paths");
  }
  const prefix = `${observation.storage_path}${sep}`;
  for (const diskPath of observation.disk_source_paths) {
    if (!isAbsolute(diskPath ?? "") || resolve(diskPath) !== diskPath || !diskPath.startsWith(prefix)) {
      throw new OrchestrationCleanupResidueError("QEMU cleanup XML disk path escapes the exact storage directory confinement");
    }
  }
  return { path: observation.storage_path, metadata };
}

function sameFileIdentity(left, right) {
  return left?.dev === right?.dev && left?.ino === right?.ino;
}

function exactPersistedQemuCleanupPlan(runtime, resource, record, plan, { requireIncomplete = true } = {}) {
  const inventory = runtime?.orchestrationInventory;
  if (!isOrchestrationInventoryV2(inventory) || !plan) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup requires a persisted journal-owned quarantine plan");
  }
  let durableInventory;
  try {
    durableInventory = loadProtectedOrchestrationInventory(runtime.config.inventory_path, runtime.config, {
      expectedHostSha256: inventory.host_sha256,
    });
  } catch (error) {
    throw new OrchestrationCleanupResidueError(`QEMU cleanup cannot load the protected durable inventory authority: ${error.message}`, { cause: error });
  }
  if (!isOrchestrationInventoryV2(durableInventory)
      || durableInventory.journal_head_sha256 !== inventory.journal_head_sha256
      || runtime.persistedJournalHeadSha256 !== durableInventory.journal_head_sha256) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup in-memory journal head is not the exact protected durable inventory head");
  }
  const persisted = inventory.journal.find((entry) => entry.event === "planned"
    && entry.mutation === "provider_cleanup"
    && entry.mutation_id === plan.mutation_id);
  const durablePlan = durableInventory.journal.find((entry) => entry.event === "planned"
    && entry.mutation === "provider_cleanup"
    && entry.mutation_id === plan.mutation_id);
  let exactPlan = false;
  try {
    exactPlan = Boolean(persisted) && Boolean(durablePlan)
      && canonicalJson(persisted) === canonicalJson(plan)
      && canonicalJson(durablePlan) === canonicalJson(plan);
  } catch { exactPlan = false; }
  if (!persisted || !durablePlan || !exactPlan
      || (requireIncomplete && (!inventory.incomplete_mutation_ids.includes(persisted.mutation_id)
        || !durableInventory.incomplete_mutation_ids.includes(persisted.mutation_id)))) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup quarantine plan is not the exact active persisted journal entry");
  }
  const materialized = inventory.resources.find((entry) => entry.instance_id === resource.instanceId);
  const durableMaterialized = durableInventory.resources.find((entry) => entry.instance_id === resource.instanceId);
  let exactMaterialized = false;
  try { exactMaterialized = Boolean(materialized) && Boolean(durableMaterialized) && canonicalJson(materialized) === canonicalJson(durableMaterialized); } catch { exactMaterialized = false; }
  if (!materialized || !durableMaterialized || !exactMaterialized
      || persisted.subject?.instance_id !== resource.instanceId
      || persisted.subject?.name !== resource.name
      || persisted.subject?.substrate !== "qemu"
      || materialized.name !== resource.name
      || materialized.substrate !== "qemu"
      || materialized.provider_id !== record.provider_id
      || materialized.provider_identity_sha256 !== record.provider_identity_sha256
      || materialized.configuration_sha256 !== record.configuration_sha256
      || materialized.ownership_binding_sha256 !== record.ownership_binding_sha256
      || materialized.provider_storage_identity_sha256 !== record.provider_storage_identity_sha256
      || materialized.storage_device !== record.storage_device
      || materialized.storage_inode !== record.storage_inode
      || materialized.storage_uid !== record.storage_uid
      || materialized.storage_gid !== record.storage_gid
      || materialized.storage_path !== record.storage_path
      || materialized.storage_quarantine_path !== persisted.subject.storage_quarantine_path
      || materialized.storage_quarantine_identity_sha256 !== persisted.subject.storage_quarantine_identity_sha256
      || materialized.storage_final_capture_path !== persisted.subject.storage_final_capture_path
      || materialized.storage_final_capture_identity_sha256 !== persisted.subject.storage_final_capture_identity_sha256
      || materialized.storage_expected_device !== persisted.subject.storage_expected_device
      || materialized.storage_expected_inode !== persisted.subject.storage_expected_inode
      || materialized.storage_expected_uid !== persisted.subject.storage_expected_uid
      || materialized.storage_expected_gid !== persisted.subject.storage_expected_gid
      || persisted.subject.storage_expected_device !== record.storage_device
      || persisted.subject.storage_expected_inode !== record.storage_inode
      || persisted.subject.storage_expected_uid !== record.storage_uid
      || persisted.subject.storage_expected_gid !== record.storage_gid) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup quarantine authority is not bound to the exact materialized provider identity");
  }
  return persisted;
}

function exactQemuQuarantineBinding(storage, record, plan) {
  if (!plan) throw new OrchestrationCleanupResidueError("QEMU cleanup lacks a persisted journal-owned storage quarantine plan");
  const expectedPath = join(dirname(storage.path), `.${basename(storage.path)}.cleanup-${plan.mutation_id}`);
  if (plan.mutation !== "provider_cleanup"
      || plan.subject?.storage_quarantine_path !== expectedPath
      || plan.subject?.storage_quarantine_identity_sha256 !== record.provider_storage_identity_sha256) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup lacks the exact journal-owned storage quarantine binding");
  }
  return { path: expectedPath };
}

function qemuCleanupHelperRequest(runtime, plan) {
  const subject = plan?.subject;
  if (!subject || subject.storage_final_capture_path !== join(QEMU_CLEANUP_CAPTURE_ROOT, runtime.runId, `${basename(subject.storage_quarantine_path ?? "")}.final`)) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup helper request lacks its exact deterministic durable capture binding");
  }
  return {
    schema_version: "agentic-sandbox.celld-qemu-cleanup-helper/v1",
    operation: "capture-delete",
    run_id: runtime.runId,
    source_path: subject.storage_quarantine_path,
    capture_path: subject.storage_final_capture_path,
    expected_uid: subject.storage_expected_uid,
    expected_gid: subject.storage_expected_gid,
    expected_device: subject.storage_expected_device,
    expected_inode: subject.storage_expected_inode,
  };
}

function invokeInstalledQemuCleanupHelper(runtime, request) {
  if (!verifyQemuCleanupHelperInstallation(runtime.config)) {
    throw annotateDriverError(new OrchestrationCleanupResidueError("installed QEMU cleanup helper is not the exact root-owned reviewed binary"), {
      operation: "orchestration.cleanup-provider.qemu-helper.verify-installation",
      errorCode: "CELLD_ORCHESTRATION_QEMU_HELPER_INSTALL_INVALID",
    });
  }
  const input = `${JSON.stringify(request)}\n`;
  if (Buffer.byteLength(input) > QEMU_CLEANUP_HELPER_MAX_INPUT) {
    throw annotateDriverError(new OrchestrationCleanupResidueError("QEMU cleanup helper request exceeds its fixed input bound"), {
      operation: "orchestration.cleanup-provider.qemu-helper.request-bound",
      errorCode: "CELLD_ORCHESTRATION_QEMU_HELPER_REQUEST_TOO_LARGE",
    });
  }
  const result = spawnSync("/usr/bin/sudo", ["-n", "--", QEMU_CLEANUP_HELPER_PATH], {
    encoding: "utf8",
    shell: false,
    input,
    maxBuffer: QEMU_CLEANUP_HELPER_MAX_OUTPUT,
    timeout: 120_000,
  });
  let response = null;
  try {
    if (Buffer.byteLength(result.stdout ?? "") <= QEMU_CLEANUP_HELPER_MAX_OUTPUT) response = JSON.parse(result.stdout);
  } catch { response = null; }
  if (result.error || result.status !== 0
      || response?.schema_version !== "agentic-sandbox.celld-qemu-cleanup-helper-result/v1"
      || !["deleted", "absent"].includes(response?.status)
      || response.capture_path !== request.capture_path) {
    throw annotateDriverError(new OrchestrationCleanupResidueError("privileged QEMU cleanup helper refused or failed the exact journal-owned capture"), {
      operation: "orchestration.cleanup-provider.qemu-helper",
      errorCode: "CELLD_ORCHESTRATION_QEMU_HELPER_FAILED",
      code: result.error?.code,
      signal: result.signal,
      exitStatus: result.status,
      stdoutSha256: sha256(result.stdout ?? ""),
      stderrSha256: sha256(result.stderr ?? ""),
      timedOut: result.error?.code === "ETIMEDOUT",
    });
  }
  return response;
}

function invokeQemuCleanupHelper(runtime, plan, dependencies = {}) {
  const request = qemuCleanupHelperRequest(runtime, plan);
  const adapter = dependencies.qemuCleanupHelper ?? runtime.qemuCleanupHelper;
  let response;
  try {
    response = adapter ? adapter({ runtime, plan, request, dependencies }) : invokeInstalledQemuCleanupHelper(runtime, request);
  } catch (error) {
    if (error instanceof OrchestrationCleanupResidueError) throw error;
    throw new OrchestrationCleanupResidueError("privileged QEMU cleanup helper did not converge", { cause: error });
  }
  if (response?.schema_version !== "agentic-sandbox.celld-qemu-cleanup-helper-result/v1"
      || !["deleted", "absent"].includes(response?.status)
      || response.capture_path !== request.capture_path) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup adapter returned invalid bounded evidence");
  }
  return response;
}

function lstatIfPresent(path) {
  try { return lstatSync(path); } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
}

function qemuFinalCapturePath(path, operation) {
  const name = basename(path);
  const quarantine = operation === "rmdir"
    ? /^(\..+\.cleanup-)[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.exec(name)
    : null;
  return quarantine
    ? join(dirname(path), `${quarantine[1]}${randomUUID()}`)
    : join(dirname(path), `.celld-qemu-delete-capture-${randomUUID()}`);
}

function captureExactQemuFinalName(path, expectedMetadata, operation, { beforeFinalNameOperation, beforeCapturedPathDeletion } = {}) {
  if (!new Set(["unlink", "rmdir"]).has(operation)) throw new Error("QEMU final name operation is invalid");
  if (beforeFinalNameOperation !== undefined && typeof beforeFinalNameOperation !== "function") {
    throw new Error("QEMU final name operation seam must be a function");
  }
  if (beforeCapturedPathDeletion !== undefined && typeof beforeCapturedPathDeletion !== "function") {
    throw new Error("QEMU captured path deletion seam must be a function");
  }
  beforeFinalNameOperation?.({ operation, path });
  const capturePath = qemuFinalCapturePath(path, operation);
  if (lstatIfPresent(capturePath) !== null) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup final capture path is unexpectedly occupied");
  }
  try {
    renameSync(path, capturePath);
  } catch (error) {
    throw new OrchestrationCleanupResidueError(`QEMU cleanup could not atomically capture the exact final pathname: ${error.message}`, { cause: error });
  }
  let captured;
  try { captured = lstatSync(capturePath); } catch (error) {
    throw new OrchestrationCleanupResidueError(`QEMU cleanup cannot verify its atomically captured final pathname: ${error.message}`, { cause: error });
  }
  if (!sameFileIdentity(captured, expectedMetadata)) {
    let restoreError = null;
    try {
      if (lstatIfPresent(path) === null) renameSync(capturePath, path);
    } catch (error) {
      restoreError = error;
    }
    throw new OrchestrationCleanupResidueError(`QEMU cleanup captured a substituted final pathname and preserved it${restoreError ? `; restoration failed: ${restoreError.message}` : ""}`);
  }
  beforeCapturedPathDeletion?.({
    operation,
    originalPath: path,
    capturedPath: capturePath,
    device: String(captured.dev),
    inode: String(captured.ino),
  });
  let afterHook;
  try { afterHook = lstatSync(capturePath); } catch (error) {
    throw new OrchestrationCleanupResidueError(`QEMU cleanup cannot verify the captured final pathname before deletion: ${error.message}`, { cause: error });
  }
  if (sameFileIdentity(afterHook, expectedMetadata)) return capturePath;
  let restoreError = null;
  try {
    if (lstatIfPresent(path) === null) renameSync(capturePath, path);
  } catch (error) {
    restoreError = error;
  }
  throw new OrchestrationCleanupResidueError(`QEMU cleanup captured a substituted final pathname and preserved it${restoreError ? `; restoration failed: ${restoreError.message}` : ""}`);
}

function removeDirectoryContentsByDescriptor(descriptor, dependencies = {}) {
  const descriptorPath = `/proc/self/fd/${descriptor}`;
  for (const name of readdirSync(descriptorPath)) {
    const childPath = join(descriptorPath, name);
    const before = lstatSync(childPath);
    if (!before.isDirectory() || before.isSymbolicLink()) {
      const capturedPath = captureExactQemuFinalName(childPath, before, "unlink", dependencies);
      rmSync(capturedPath, { recursive: false, force: false });
      continue;
    }
    let childDescriptor = null;
    try {
      childDescriptor = openSync(childPath, constants.O_RDONLY | constants.O_DIRECTORY | (constants.O_NOFOLLOW ?? 0));
      const opened = fstatSync(childDescriptor);
      if (!opened.isDirectory() || !sameFileIdentity(before, opened)) {
        throw new OrchestrationCleanupResidueError("QEMU cleanup detected a substituted nested storage directory");
      }
      removeDirectoryContentsByDescriptor(childDescriptor, dependencies);
      const capturedPath = captureExactQemuFinalName(childPath, opened, "rmdir", dependencies);
      const captured = lstatSync(capturedPath);
      if (!captured.isDirectory() || captured.isSymbolicLink() || !sameFileIdentity(opened, captured)) {
        throw new OrchestrationCleanupResidueError("QEMU cleanup detected a substituted captured nested storage directory");
      }
      rmdirSync(capturedPath);
    } finally {
      if (childDescriptor !== null) closeSync(childDescriptor);
    }
  }
}

function removeInodeBoundDirectory(path, expectedMetadata, { beforeDelete, beforeFinalNameOperation, beforeCapturedPathDeletion } = {}) {
  if (beforeDelete !== undefined && typeof beforeDelete !== "function") {
    throw new Error("QEMU quarantine pre-delete seam must be a function");
  }
  let descriptor = null;
  try {
    descriptor = openSync(path, constants.O_RDONLY | constants.O_DIRECTORY | (constants.O_NOFOLLOW ?? 0));
    const opened = fstatSync(descriptor);
    if (!opened.isDirectory() || !sameFileIdentity(opened, expectedMetadata)) {
      throw new OrchestrationCleanupResidueError("QEMU cleanup quarantine descriptor has a substituted storage identity");
    }
    beforeDelete?.({ path, device: String(opened.dev), inode: String(opened.ino) });
    const afterHook = lstatSync(path);
    if (!afterHook.isDirectory() || afterHook.isSymbolicLink() || !sameFileIdentity(opened, afterHook)) {
      throw new OrchestrationCleanupResidueError("QEMU cleanup detected a substituted quarantine pathname before deletion");
    }
    removeDirectoryContentsByDescriptor(descriptor, { beforeFinalNameOperation, beforeCapturedPathDeletion });
    const capturedPath = captureExactQemuFinalName(path, opened, "rmdir", { beforeFinalNameOperation, beforeCapturedPathDeletion });
    const captured = lstatSync(capturedPath);
    if (!captured.isDirectory() || captured.isSymbolicLink() || !sameFileIdentity(opened, captured)) {
      throw new OrchestrationCleanupResidueError("QEMU cleanup detected a substituted captured quarantine directory");
    }
    const beforeRmdir = lstatSync(capturedPath);
    if (!beforeRmdir.isDirectory() || beforeRmdir.isSymbolicLink() || !sameFileIdentity(opened, beforeRmdir)) {
      throw new OrchestrationCleanupResidueError("QEMU cleanup detected a substituted captured quarantine directory after deletion");
    }
    rmdirSync(capturedPath);
  } finally {
    if (descriptor !== null) closeSync(descriptor);
  }
}

function containsQemuPostCaptureTestResidue(path) {
  for (const name of readdirSync(path)) {
    if (name.startsWith(".celld-qemu-authorized-residue-")) return true;
    const childPath = join(path, name);
    const metadata = lstatSync(childPath);
    if (metadata.isDirectory() && !metadata.isSymbolicLink() && containsQemuPostCaptureTestResidue(childPath)) {
      return true;
    }
  }
  return false;
}

// This adapter exists only so the unprivileged Node test process can exercise
// pre-helper race classification without sudo or the fixed /build layout. The
// qualification/runtime path never selects it and it refuses the production VM
// root. Destructive authority remains exclusively in the installed Rust helper.
export function qemuCleanupHelperTestAdapter({ request, dependencies = {} }) {
  if (!process.env.NODE_TEST_CONTEXT || request.source_path.startsWith("/build/agentic-sandbox/vms/")) {
    throw new OrchestrationCleanupResidueError("unprivileged QEMU cleanup test adapter is unavailable outside an isolated Node test fixture");
  }
  const physicalCapturePath = dependencies.qemuCleanupPhysicalCapturePath?.(request)
    ?? `${request.source_path}.final`;
  const tombstonePath = `${physicalCapturePath}.deleted`;
  const tombstoneBody = `${JSON.stringify({
    schema_version: "agentic-sandbox.celld-qemu-cleanup-helper-result/v1",
    status: "deleted",
    source_path: request.source_path,
    capture_path: request.capture_path,
    expected_uid: request.expected_uid,
    expected_gid: request.expected_gid,
    expected_device: request.expected_device,
    expected_inode: request.expected_inode,
  })}\n`;
  const sourceMetadata = lstatIfPresent(request.source_path);
  const captureMetadata = lstatIfPresent(physicalCapturePath);
  const tombstoneMetadata = lstatIfPresent(tombstonePath);
  if (tombstoneMetadata && (sourceMetadata || captureMetadata)) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup test adapter tombstone conflicts with live storage");
  }
  if (!sourceMetadata && !captureMetadata) {
    if (!tombstoneMetadata || readFileSync(tombstonePath, "utf8") !== tombstoneBody) {
      throw new OrchestrationCleanupResidueError("QEMU cleanup test adapter refuses absent storage without a helper tombstone");
    }
    return {
      schema_version: "agentic-sandbox.celld-qemu-cleanup-helper-result/v1",
      status: "absent",
      capture_path: request.capture_path,
    };
  }
  if (sourceMetadata && captureMetadata) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup test adapter found source and capture simultaneously");
  }
  let metadata = captureMetadata;
  if (sourceMetadata) {
    if (String(sourceMetadata.dev) !== request.expected_device || String(sourceMetadata.ino) !== request.expected_inode
        || String(sourceMetadata.uid) !== request.expected_uid || String(sourceMetadata.gid) !== request.expected_gid) {
      throw new OrchestrationCleanupResidueError("QEMU cleanup test adapter observed substituted storage identity");
    }
    if (lstatIfPresent(physicalCapturePath) !== null) {
      throw new OrchestrationCleanupResidueError("QEMU cleanup test adapter capture path is occupied");
    }
    renameSync(request.source_path, physicalCapturePath);
    metadata = lstatSync(physicalCapturePath);
    if (!sameFileIdentity(sourceMetadata, metadata)) {
      throw new OrchestrationCleanupResidueError("QEMU cleanup test adapter captured a substituted storage identity");
    }
  }
  if (String(metadata.dev) !== request.expected_device || String(metadata.ino) !== request.expected_inode
      || String(metadata.uid) !== request.expected_uid || String(metadata.gid) !== request.expected_gid) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup test adapter observed substituted capture identity");
  }
  if (containsQemuPostCaptureTestResidue(physicalCapturePath)) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup test adapter found prior post-capture substitution residue");
  }
  dependencies.beforeQemuCapturedPathDeletion?.({
    operation: "rmdir",
    originalPath: request.source_path,
    capturedPath: physicalCapturePath,
    logicalCapturePath: request.capture_path,
    device: String(metadata.dev),
    inode: String(metadata.ino),
  });
  const afterHook = lstatSync(physicalCapturePath);
  if (!sameFileIdentity(metadata, afterHook)) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup test adapter detected substituted root capture before deletion");
  }
  removeInodeBoundDirectory(physicalCapturePath, metadata, {
    beforeDelete: dependencies.beforeQuarantineDelete,
    beforeFinalNameOperation: dependencies.beforeQemuFinalNameOperation,
    beforeCapturedPathDeletion: dependencies.beforeQemuCapturedPathDeletion,
  });
  writeFileSync(tombstonePath, tombstoneBody, { mode: 0o600, flag: "wx" });
  return {
    schema_version: "agentic-sandbox.celld-qemu-cleanup-helper-result/v1",
    status: "deleted",
    capture_path: request.capture_path,
  };
}

function removeQuarantinedQemuStorage(runtime, storage, record, plan, dependencies = {}) {
  let current;
  try {
    current = lstatSync(storage.path);
  } catch (error) {
    throw new OrchestrationCleanupResidueError(`QEMU cleanup storage pathname changed after undefine: ${error.message}`, { cause: error });
  }
  if (!current.isDirectory() || current.isSymbolicLink() || !sameFileIdentity(current, storage.metadata)) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup detected a substituted storage pathname after undefine");
  }
  const quarantine = exactQemuQuarantineBinding(storage, record, plan).path;
  try {
    renameSync(storage.path, quarantine);
    const quarantined = lstatSync(quarantine);
    if (!quarantined.isDirectory() || quarantined.isSymbolicLink() || !sameFileIdentity(quarantined, storage.metadata)) {
      if (!existsSync(storage.path)) renameSync(quarantine, storage.path);
      throw new OrchestrationCleanupResidueError("QEMU cleanup quarantine captured a substituted storage identity");
    }
    invokeQemuCleanupHelper(runtime, plan, dependencies);
  } catch (error) {
    if (error instanceof OrchestrationCleanupResidueError) throw error;
    throw new OrchestrationCleanupResidueError(`QEMU cleanup could not remove the inode-verified exact storage quarantine: ${error.message}`, { cause: error });
  }
}

function compareObservedProviderBinding(resource, record, authorized, current) {
  exactCleanupObservation(record, current);
  const fields = [
    "provider_id",
    "provider_identity_sha256",
    "configuration_sha256",
    "ownership_binding_sha256",
    ...(resource.substrate === "docker" ? [
      "managed_network_id",
      "managed_network_identity_sha256",
      "managed_network_configuration_sha256",
      "provider_labels",
      "managed_network_labels",
    ] : [
      "provider_storage_identity_sha256",
      "storage_path",
      "storage_device",
      "storage_inode",
      "storage_uid",
      "storage_gid",
      "disk_source_paths",
    ]),
  ];
  for (const field of fields) {
    const expected = authorized?.[field];
    const observed = current?.[field];
    if (expected === undefined || observed === undefined || canonicalJson(expected) !== canonicalJson(observed)) {
      throw new OrchestrationCleanupResidueError(`provider cleanup observed a substituted ${field} identity binding`);
    }
  }
  if (resource.substrate === "qemu" && (authorized.provider_storage_present !== true || current.provider_storage_present !== true)) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup lacks an exact owned storage binding");
  }
  return current;
}

export async function removeExactlyObservedProvider(runtime, resource, record, authorizedObservation, dependencies = {}) {
  const qemuPlan = resource.substrate === "qemu"
    ? exactPersistedQemuCleanupPlan(runtime, resource, record, dependencies.plan)
    : null;
  const observeEffectTarget = dependencies.observeProviderEffectTarget
    ?? (async () => observeOrchestrationProvider(runtime, resource));
  const current = compareObservedProviderBinding(
    resource,
    record,
    authorizedObservation,
    await observeEffectTarget({ runtime, resource, record, authorizedObservation }),
  );
  if (dependencies.destroyProviderTarget !== undefined) {
    if (typeof dependencies.destroyProviderTarget !== "function") throw new Error("provider cleanup destroy adapter must be a function");
    await dependencies.destroyProviderTarget({ runtime, resource, record, observation: current });
    return;
  }
  if (resource.substrate === "docker") {
    await destroyExactlyObservedProvider(runtime, resource, current);
    return;
  }
  if (resource.substrate !== "qemu") {
    throw new OrchestrationCleanupResidueError("provider cleanup substrate is outside the fixed allowlist");
  }
  validateExactQemuStorage(resource, current);
  if (!["shut off", "crashed"].includes(current.state)) {
    run("virsh", ["-c", runtime.config.libvirt_uri, "destroy", current.provider_id], { timeout: 180_000 });
  }
  const convergenceDeadline = Date.now() + (runtime.providerCleanupConvergenceTimeoutMs ?? 30_000);
  let beforeUndefine;
  do {
    beforeUndefine = compareObservedProviderBinding(
      resource,
      record,
      authorizedObservation,
      await observeEffectTarget({ runtime, resource, record, authorizedObservation }),
    );
    if (["shut off", "crashed"].includes(beforeUndefine.state)) break;
    if (Date.now() >= convergenceDeadline) break;
    await sleep(runtime.providerCleanupConvergenceIntervalMs ?? 250);
  } while (Date.now() < convergenceDeadline);
  const storage = validateExactQemuStorage(resource, beforeUndefine);
  if (!["shut off", "crashed"].includes(beforeUndefine.state)) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup did not observe the exact domain stopped before undefine");
  }
  run("virsh", ["-c", runtime.config.libvirt_uri, "undefine", beforeUndefine.provider_id, "--nvram"], { timeout: 180_000 });
  removeQuarantinedQemuStorage(runtime, storage, record, qemuPlan, dependencies);
}

function incompleteProviderCleanupPlan(inventory, instanceId) {
  return inventory.journal.find((entry) => entry.event === "planned"
    && entry.mutation === "provider_cleanup"
    && entry.subject.instance_id === instanceId
    && inventory.incomplete_mutation_ids.includes(entry.mutation_id));
}

function latestProviderCleanupPlan(inventory, instanceId) {
  for (let index = inventory.journal.length - 1; index >= 0; index -= 1) {
    const entry = inventory.journal[index];
    if (entry.event === "planned" && entry.mutation === "provider_cleanup" && entry.subject.instance_id === instanceId) return entry;
  }
  return null;
}

function assertNoUnplannedQemuCleanupResidue(runtime, resource, record) {
  if (!isAbsolute(record.storage_path ?? "")
      || resolve(dirname(record.storage_path)) !== resolve(runtime.config.vm_storage_dir ?? "")
      || basename(record.storage_path) !== resource.name) {
    throw new OrchestrationCleanupResidueError("removed QEMU resource has an invalid persisted storage boundary");
  }
  if (lstatIfPresent(record.storage_path) !== null) {
    throw new OrchestrationCleanupResidueError("removed QEMU resource retains its original storage pathname without cleanup authority");
  }
  const escapedName = resource.name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const candidatePattern = new RegExp(`^\\.${escapedName}\\.cleanup-[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$`);
  let candidates;
  try {
    candidates = readdirSync(dirname(record.storage_path)).filter((name) => candidatePattern.test(name));
  } catch (error) {
    throw new OrchestrationCleanupResidueError(`removed QEMU resource cleanup cannot enumerate its exact storage boundary: ${error.message}`, { cause: error });
  }
  if (candidates.length !== 0) {
    throw new OrchestrationCleanupResidueError("removed QEMU resource retains an unplanned storage quarantine");
  }
}

function qemuStorageIdentityForPersistedPath(record, path) {
  let metadata;
  try { metadata = lstatSync(path); } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) return null;
  return sha256(canonicalJson({
    path: record.storage_path,
    device: String(metadata.dev),
    inode: String(metadata.ino),
    uid: metadata.uid,
    gid: metadata.gid,
  }));
}

function reconcileQemuQuarantineResidue(runtime, resource, record, cleanupPlan, dependencies = {}) {
  if (record.substrate !== "qemu") return;
  if (!cleanupPlan) throw new OrchestrationCleanupResidueError("QEMU cleanup residue has no persisted quarantine plan");
  if (!isAbsolute(record.storage_path ?? "") || resolve(dirname(record.storage_path)) !== resolve(runtime.config.vm_storage_dir ?? "")
      || basename(record.storage_path) !== resource.name) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup residue is outside the exact configured storage root");
  }
  const persistedPlan = exactPersistedQemuCleanupPlan(runtime, resource, record, cleanupPlan, { requireIncomplete: false });
  const binding = exactQemuQuarantineBinding({ path: record.storage_path }, record, persistedPlan);
  const candidatePattern = new RegExp(`^\\.${resource.name.replace(/[.*+?^${}()|[\\]\\]/g, "\\$&")}\\.cleanup-[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$`);
  const parent = dirname(record.storage_path);
  let candidates;
  try {
    candidates = readdirSync(parent).filter((name) => candidatePattern.test(name)).map((name) => join(parent, name));
  } catch (error) {
    throw new OrchestrationCleanupResidueError(`QEMU cleanup cannot enumerate exact storage quarantine residue: ${error.message}`, { cause: error });
  }
  const unknown = candidates.filter((path) => path !== binding.path);
  if (unknown.length !== 0) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup found an unbound storage quarantine residue");
  }
  if (candidates.includes(binding.path)
      && qemuStorageIdentityForPersistedPath(record, binding.path) !== cleanupPlan.subject.storage_quarantine_identity_sha256) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup journal-owned storage quarantine has a substituted persisted identity");
  }
  invokeQemuCleanupHelper(runtime, persistedPlan, dependencies);
  let originalIdentity;
  try { originalIdentity = qemuStorageIdentityForPersistedPath(record, record.storage_path); } catch (error) {
    throw new OrchestrationCleanupResidueError(`QEMU cleanup cannot classify original storage residue: ${error.message}`, { cause: error });
  }
  if (originalIdentity === record.provider_storage_identity_sha256) {
    throw new OrchestrationCleanupResidueError("QEMU cleanup retains the authorized original storage identity");
  }
}

function markCleanupResidue(runtime, error) {
  try { persistOrchestrationInventory(runtime, new Date(), "cleanup_residue"); } catch (persistError) {
    throw annotateDriverError(
      new OrchestrationCleanupResidueError(`${error.message}; cleanup residue persistence failed: ${persistError.message}`, { cause: error }),
      { errorCode: error.errorCode ?? "CELLD_PROVIDER_CLEANUP_RESIDUE_PERSISTENCE" },
    );
  }
  if (error instanceof OrchestrationCleanupResidueError) throw error;
  throw new OrchestrationCleanupResidueError(error.message, { cause: error });
}

export async function cleanupOwnedProviderResources(runtime, dependencies = {}) {
  if (!runtime) return [];
  if (!isOrchestrationInventoryV2(runtime.orchestrationInventory)) {
    throw new Error("orchestration inventory v1 is read-only during provider cleanup; upgrade to v2 first");
  }
  const observe = dependencies.observeProviderResource
    ?? (async ({ resource }) => observeOrchestrationProvider(runtime, resource));
  const remove = dependencies.removeProviderResource
    ?? (async ({ plan, resource, record, observation }) => removeExactlyObservedProvider(runtime, resource, record, observation, { plan }));
  const assertions = [];
  const failures = [];
  for (const record of runtime.orchestrationInventory.resources) {
    let cleanupPhase = "INITIAL_OBSERVATION";
    try {
      const resource = runtime.providerResources?.get(record.instance_id) ?? {
        instanceId: record.instance_id,
        name: record.name,
        substrate: record.substrate,
      };
      if (record.status === "removed") {
        cleanupPhase = "REMOVED_RESOURCE_RECONCILIATION";
        if (record.substrate === "qemu") {
          const cleanupPlan = latestProviderCleanupPlan(runtime.orchestrationInventory, record.instance_id);
          if (cleanupPlan) reconcileQemuQuarantineResidue(runtime, resource, record, cleanupPlan, dependencies);
          else assertNoUnplannedQemuCleanupResidue(runtime, resource, record);
        }
        runtime.providerResources?.delete(record.instance_id);
        continue;
      }
      const first = exactCleanupObservation(record, await observe({ runtime, resource, record }));
      cleanupPhase = "PLAN";
      let cleanupPlan = incompleteProviderCleanupPlan(runtime.orchestrationInventory, record.instance_id);
      const interruptedActionPlan = incompleteProviderActionPlanFor(runtime.orchestrationInventory, record.instance_id);
      const newlyBoundEffect = interruptedActionPlan?.mutation === "provider_action"
        && interruptedActionPlan.subject.action === "provision"
        && first.present
        && !record.provider_identity_sha256
        && hasProviderEffectBinding(resource.substrate, first);
      if (first.present && !record.provider_identity_sha256 && !cleanupPlan && !newlyBoundEffect && runtime.runId !== undefined) {
        throw new OrchestrationCleanupResidueError("provider cleanup refuses an unbound provider plan before any destructive effect");
      }
      if (newlyBoundEffect) completeRecoveredProviderEffect(runtime, interruptedActionPlan, record, first);
      if (!first.present && interruptedActionPlan && !cleanupPlan) {
        finishOrchestrationMutation(runtime.orchestrationInventory, interruptedActionPlan, {
          event: "recovered",
          outcome: "absent",
          observedIdentitySha256: null,
          observedConfigurationSha256: null,
        });
        persistOrchestrationInventory(runtime);
        runtime.providerResources?.delete(record.instance_id);
        assertions.push(`${record.substrate === "docker" ? "Docker" : "QEMU"} interrupted provider identity ${sha256(record.instance_id)} absent`);
        continue;
      }
      if (!cleanupPlan) {
        cleanupPlan = planProviderCleanup(runtime, resource, new Date(),
          newlyBoundEffect ? null : interruptedActionPlan?.mutation_id ?? null);
      }
      if (first.present) {
        cleanupPhase = "REMOVE";
        await remove({ runtime, plan: cleanupPlan, resource, record, observation: first });
        cleanupPhase = "FINAL_OBSERVATION";
        const finalObservation = exactCleanupObservation(record, await observe({ runtime, plan: cleanupPlan, resource, record }));
        if (finalObservation.present) throw new OrchestrationCleanupResidueError("provider cleanup did not reach exact observed absence");
      }
      cleanupPhase = "QUARANTINE_RECONCILIATION";
      reconcileQemuQuarantineResidue(runtime, resource, record, cleanupPlan, dependencies);
      cleanupPhase = "TERMINAL_COMMIT";
      completeProviderCleanup(runtime, cleanupPlan, { present: false, owned: true, provider_storage_present: false });
      if (interruptedActionPlan
          && runtime.orchestrationInventory.incomplete_mutation_ids.includes(interruptedActionPlan.mutation_id)) {
        finishOrchestrationMutation(runtime.orchestrationInventory, interruptedActionPlan, {
          event: "recovered",
          outcome: "absent",
          observedIdentitySha256: null,
          observedConfigurationSha256: null,
        });
        persistOrchestrationInventory(runtime);
      }
      runtime.providerResources?.delete(record.instance_id);
      assertions.push(`${record.substrate === "docker" ? "Docker" : "QEMU"} provider identity ${sha256(record.instance_id)} absent`);
    } catch (error) {
      const failure = error instanceof OrchestrationCleanupResidueError
        ? error
        : new OrchestrationCleanupResidueError(error.message, { cause: error });
      failures.push(annotateDriverError(failure, { errorCode: `CELLD_PROVIDER_CLEANUP_${cleanupPhase}` }));
    }
  }
  if (failures.length) markCleanupResidue(runtime, failures[0]);
  return assertions;
}

function unavailable({ scenarioId, runId, profile, startedAt, reasonCode }) {
  return {
    schema_version: OBSERVATION_SCHEMA, driver_id: DRIVER_ID, run_id: runId, scenario_id: scenarioId,
    started_at: startedAt, ended_at: new Date().toISOString(), mutation_started: false,
    prerequisites: [{ id: "CELLD_ORCHESTRATION", status: "unavailable", reason_code: reasonCode }], assertions: [],
    identities: { profile_id: profile.profile_id, sandbox_git: profile.expected_sandbox_git, environment_host_sha256: profile.environment.host_sha256, driver_version: DRIVER_VERSION },
    metrics: [], faults: [], artifacts: [], cleanup: { status: "not_required", assertions: [] },
  };
}

export function selectOrchestrationRunFailure({ campaignError = null, cleanupErrors = [] } = {}) {
  const cleanupError = cleanupErrors.find((error) => error instanceof OrchestrationCleanupResidueError || error?.exitCode === 4) ?? null;
  if (cleanupError) {
    cleanupError.campaignError = campaignError;
    return cleanupError;
  }
  return campaignError ?? cleanupErrors[0] ?? null;
}

function prerequisiteReason(config) {
  if (process.platform !== "linux") return "CELLD_ORCHESTRATION_LINUX_REQUIRED";
  for (const [program, args, code] of [["docker", ["compose", "version"], "CELLD_DOCKER_UNAVAILABLE"], ["virsh", ["-c", config.libvirt_uri, "version"], "CELLD_LIBVIRT_UNAVAILABLE"], ["openssl", ["version"], "CELLD_OPENSSL_UNAVAILABLE"]]) if (!available(program, args)) return code;
  if (executable(config.management_binary_path, "management binary")) return "CELLD_MANAGEMENT_BINARY_UNAVAILABLE";
  if (executable(config.agent_client_binary_path, "agent client binary")) return "CELLD_AGENT_CLIENT_BINARY_UNAVAILABLE";
  if (executable(config.callback_relay_binary_path, "callback relay")) return "CELLD_CALLBACK_RELAY_UNAVAILABLE";
  if (!verifyQemuCleanupHelperInstallation(config)) return "CELLD_QEMU_CLEANUP_HELPER_UNAVAILABLE";
  if (spawnSync("docker", ["image", "inspect", config.docker_image_ref], { encoding: "utf8", shell: false }).status !== 0) return "CELLD_DOCKER_IMAGE_UNAVAILABLE";
  return null;
}

export async function executeOrchestrationDriver({ scenarioId, runId, liveProfilePath, artifactDir }, dependencies = {}) {
  const startedAt = new Date().toISOString();
  const errorFields = { scenarioId };
  const profile = await withDriverOperation("orchestration.load-profile", errorFields, () => protectedJson(liveProfilePath, "live profile"));
  const profileErrors = await withDriverOperation("orchestration.validate-profile", errorFields, () => validateLiveProfile(profile));
  if (profileErrors.length) throw driverOperationError("orchestration.validate-profile", errorFields, profileErrors.join("; "));
  const entry = profile.drivers?.[DRIVER_ID];
  if (!entry?.enabled) return unavailable({ scenarioId, runId, profile, startedAt, reasonCode: "CELLD_ORCHESTRATION_DRIVER_DISABLED" });
  const git = await withDriverOperation("orchestration.git-identity", errorFields, () => dependencies.gitCommit?.() ?? run("git", ["rev-parse", "HEAD"]));
  if (!SCENARIOS.has(scenarioId) || profile.run_id !== runId || profile.expected_sandbox_git !== git) throw driverOperationError("orchestration.validate-identity", errorFields, "live orchestration identity does not match the requested run");
  const observedHostSha256 = sha256(await withDriverOperation("orchestration.host-identity", errorFields, () => dependencies.hostname?.() ?? hostname()));
  if (profile.environment.host_sha256 !== observedHostSha256) throw driverOperationError("orchestration.validate-host-identity", errorFields, "live orchestration host identity does not match the protected profile");
  const config = await withDriverOperation("orchestration.load-config", errorFields, () => protectedJson(entry.config_path, "orchestration config"));
  const configErrors = await withDriverOperation("orchestration.validate-config", errorFields, () => validateOrchestrationConfig(config));
  if (configErrors.length) throw driverOperationError("orchestration.validate-config", errorFields, configErrors.join("; "));
  if (config.run_id !== runId) throw driverOperationError("orchestration.validate-config", errorFields, "orchestration config run identity does not match the live profile");
  if (!profile.authorization.destructive_faults || profile.authorization.exact_run_owner !== profile.run_id) return unavailable({ scenarioId, runId, profile, startedAt, reasonCode: "CELLD_DESTRUCTIVE_AUTHORIZATION_REQUIRED" });
  const orchestrationInventory = await withDriverOperation("orchestration.load-authorized-inventory", errorFields, () => loadAuthorizedOrchestrationInventory(profile, config, observedHostSha256));
  const reason = await withDriverOperation("orchestration.check-prerequisites", errorFields, () => dependencies.prerequisiteReason ? dependencies.prerequisiteReason(config, scenarioId, profile) : prerequisiteReason(config));
  if (reason) return unavailable({ scenarioId, runId, profile, startedAt, reasonCode: reason });

  mkdirSync(artifactDir, { recursive: true, mode: 0o700 });
  chmodSync(artifactDir, 0o700);
  const timeline = [];
  let fixture = null;
  let fleet = null;
  let fleetPath = null;
  let workerAccess = null;
  let management = null;
  let runtime = null;
  let cleanupStatus = "failed";
  const cleanupAssertions = [];
  const cleanupErrors = [];
  let campaignError = null;
  let campaign;
  try {
    const scenarioRoot = join(config.working_root, scenarioId.toLowerCase(), runId);
    fixture = await withDriverOperation("orchestration.prepare-storage-fixture", errorFields, () => prepareFixture({ fixtureProfile: "titan-single-host-storage", runId, root: scenarioRoot }));
    await withDriverOperation("orchestration.start-storage-fixture", errorFields, () => startFixture(fixture));
    fleet = await withDriverOperation("orchestration.prepare-fleet", errorFields, () => prepareFleet({ storageConfigPath: join(scenarioRoot, "fixture.json") }));
    fleetPath = join(scenarioRoot, "fleet.json");
    const fleetErrors = await withDriverOperation("orchestration.validate-fleet", errorFields, () => validateFleetConfig(fleet));
    if (fleetErrors.length) throw driverOperationError("orchestration.validate-fleet", errorFields, fleetErrors.join("; "));
    await withDriverOperation("orchestration.deploy-worker", errorFields, () => deployFleetWorker(fleetPath));
    const diagnosis = await withDriverOperation("orchestration.start-fleet", errorFields, () => startFleet(fleetPath));
    if (diagnosis.status !== "READY") throw driverOperationError("orchestration.start-fleet", errorFields, "three-node Celld fleet is not ready");
    const managementHost = await withDriverOperation("orchestration.resolve-storage-gateway", errorFields, () => storageGateway(fleet));
    workerAccess = await withDriverOperation("orchestration.open-worker-access", errorFields, () => openFleetWorkerAccess(fleetPath));
    management = await withDriverOperation("orchestration.launch-management", errorFields, () => launchManagement(config, fleet, managementHost, { celldEndpoint: workerAccess.endpoint }));
    await withDriverOperation("orchestration.wait-management", errorFields, () => waitManagement(management, fleet));
    await withDriverOperation("orchestration.start-callback-relays", errorFields, () => startCallbackRelays(fleetPath, { relayBinaryPath: config.callback_relay_binary_path, enableFaultSignal: ["UAT-CELLD-005"].includes(scenarioId) }));
    const activeResources = orchestrationInventory.resources.filter((resource) => resource.status !== "removed").map((resource) => [resource.instance_id, { instanceId: resource.instance_id, name: resource.name, substrate: resource.substrate }]);
    runtime = {
      config, fleet, fleetPath, management, managementHost,
      workerAccess, workerAccesses: new Set([workerAccess]), workerEndpoint: workerAccess.endpoint,
      openFleetWorkerAccess: dependencies.openFleetWorkerAccess,
      runId, scenarioId,
      orchestrationInventory, providerResources: new Map(activeResources),
      persistedJournalHeadSha256: orchestrationInventory.journal_head_sha256,
      persistInventory: dependencies.persistInventory,
      sendWorkerCommand: dependencies.sendWorkerCommand,
      observeFaultTarget: dependencies.observeFaultTarget ?? observeOrchestrationFaultTarget,
      resolveFaultTarget: dependencies.resolveFaultTarget ?? resolveOrchestrationFaultTarget,
      revalidateFaultTarget: dependencies.revalidateFaultTarget ?? resolveOrchestrationFaultTarget,
    };
    const runner = dependencies.runScenario ?? ({
      "UAT-CELLD-003": runUat003, "UAT-CELLD-004": runUat004,
      "UAT-CELLD-005": runUat005, "UAT-CELLD-006": runUat006,
    })[scenarioId];
    campaign = await withDriverOperation(`orchestration.run-${scenarioId.toLowerCase()}`, errorFields, () => runner(runtime, timeline));
    management = runtime.management;
  } catch (error) {
    campaignError = error;
  } finally {
    const retainCleanupError = (kind, error) => {
      cleanupAssertions.push(`${kind} cleanup digest ${sha256(error.message)}`);
      cleanupErrors.push(annotateDriverError(error instanceof OrchestrationCleanupResidueError
        ? error
        : new OrchestrationCleanupResidueError(`${kind} cleanup failed`, { cause: error }), { operation: `orchestration.cleanup-${kind}`, scenarioId }));
    };
    try { cleanupAssertions.push(...await cleanupOwnedOrchestrationFaults(runtime)); } catch (error) { retainCleanupError("faults", error); }
    try { await stopManagementAndWait(runtime?.management ?? management, "SIGKILL"); cleanupAssertions.push("management process terminated"); } catch (error) { retainCleanupError("management", error); }
    const workerAccesses = runtime?.workerAccesses ?? new Set(workerAccess ? [workerAccess] : []);
    for (const access of workerAccesses) {
      try {
        await access.close();
        cleanupAssertions.push("Worker loopback access closed");
      } catch (error) { retainCleanupError("worker-access", error); }
    }
    try { cleanupAssertions.push(...await cleanupOwnedProviderResources(runtime)); } catch (error) { retainCleanupError("provider", error); }
    try { if (fleetPath && existsSync(fleetPath)) cleanupFleet(fleetPath); cleanupAssertions.push("exact fleet containers and protected fleet state removed"); } catch (error) { retainCleanupError("fleet", error); }
    try { if (fixture) cleanupFixture(fixture); cleanupAssertions.push("exact storage services, network, volumes, and run directory removed"); } catch (error) { retainCleanupError("storage", error); }
    cleanupStatus = cleanupAssertions.some((value) => value.includes("digest")) ? "failed" : "passed";
  }
  const selectedFailure = selectOrchestrationRunFailure({ campaignError, cleanupErrors });
  if (selectedFailure) throw selectedFailure;
  if (!campaign) throw driverOperationError("orchestration.run-campaign", errorFields, "orchestration campaign produced no measurements");

  const evidence = { schema_version: "agentic-sandbox.celld-orchestration-evidence/v1", run_id: runId, scenario_id: scenarioId, measurements: Object.fromEntries(campaign.assertions.map((item) => [item.id, item.measurements])), faults: campaign.faults, timeline_sha256: sha256(timeline.map((row) => JSON.stringify(row)).join("\n")) };
  const suffix = scenarioId.toLowerCase();
  const evidenceName = `orchestration-evidence-${suffix}.json`;
  const timelineName = `orchestration-timeline-${suffix}.jsonl`;
  const evidencePath = join(artifactDir, evidenceName);
  const timelinePath = join(artifactDir, timelineName);
  writeFileSync(evidencePath, `${JSON.stringify(evidence)}\n`, { mode: 0o600, flag: "wx" });
  writeFileSync(timelinePath, `${timeline.map((row) => JSON.stringify(row)).join("\n")}\n`, { mode: 0o600, flag: "wx" });
  chmodSync(evidencePath, 0o600); chmodSync(timelinePath, 0o600);
  const artifacts = [artifact(evidencePath, `artifacts/${evidenceName}`, "application/json"), artifact(timelinePath, `artifacts/${timelineName}`, "application/x-ndjson")];
  return {
    schema_version: OBSERVATION_SCHEMA, driver_id: DRIVER_ID, run_id: runId, scenario_id: scenarioId,
    started_at: startedAt, ended_at: new Date().toISOString(), mutation_started: true,
    prerequisites: [
      { id: "CELLD_ORCHESTRATION", status: "available", reason_code: "CELLD_ORCHESTRATION_READY" },
      { id: "CELLD_REAL_PROVIDERS", status: "available", reason_code: "CELLD_QEMU_DOCKER_READY" },
      { id: "CELLD_PRIVATE_CALLBACK", status: "available", reason_code: "CELLD_CALLBACK_MTLS_READY" },
    ],
    assertions: campaign.assertions.map((item) => ({ ...item, evidence_refs: artifacts.map((entryArtifact) => entryArtifact.path) })),
    identities: { profile_id: profile.profile_id, sandbox_git: profile.expected_sandbox_git, environment_host_sha256: profile.environment.host_sha256, driver_version: DRIVER_VERSION },
    metrics: campaign.metrics, faults: campaign.faults, artifacts,
    cleanup: { status: cleanupStatus, assertions: cleanupAssertions },
  };
}

export function loadProtectedOrchestrationRuntime(configPath, profilePath, options = {}) {
  const { requireDestructiveAuthorization = true } = options;
  const resolvedConfigPath = resolve(configPath);
  const config = protectedJson(resolvedConfigPath, "orchestration config");
  const errors = validateOrchestrationConfig(config);
  if (errors.length) throw new Error(errors.join("; "));
  if (resolvedConfigPath !== join(config.working_root, "orchestration.json")) throw new Error("orchestration config is not at the fixed run-root path");
  const profile = protectedJson(resolve(profilePath), "live profile");
  const profileErrors = validateLiveProfile(profile);
  if (profileErrors.length) throw new Error(profileErrors.join("; "));
  const entry = profile.drivers?.[DRIVER_ID];
  if (!entry?.enabled || resolve(entry.config_path ?? "") !== resolvedConfigPath || profile.run_id !== config.run_id) {
    throw new Error("protected live profile does not authorize the exact orchestration config");
  }
  const observedHostSha256 = sha256(hostname());
  if (profile.environment.host_sha256 !== observedHostSha256) throw new Error("protected live profile does not authorize this host");
  const orchestrationInventory = requireDestructiveAuthorization
    ? loadAuthorizedOrchestrationInventory(profile, config, observedHostSha256)
    : loadProfileBoundOrchestrationInventory(profile, config, observedHostSha256);
  if (!requireDestructiveAuthorization && !profileHasExactDestructiveAuthorization(profile) && orchestrationInventoryNeedsDestructiveRecovery(orchestrationInventory)) {
    throw new OrchestrationCleanupResidueError("exact-run destructive authorization is required for retained orchestration effects");
  }
  const activeResources = orchestrationInventory.resources
    .filter((resource) => resource.status !== "removed")
    .map((resource) => [resource.instance_id, { instanceId: resource.instance_id, name: resource.name, substrate: resource.substrate }]);
  return {
    config,
    profile,
    orchestrationInventory,
    providerResources: new Map(activeResources),
    runId: config.run_id,
    scenarioId: activeResources.length > 0 ? orchestrationInventory.resources.find((resource) => resource.status !== "removed").scenario_id : "UAT-CELLD-003",
    persistedJournalHeadSha256: orchestrationInventory.journal_head_sha256,
  };
}

async function recoverRetainedOrchestrationRun({ runId, orchestrationRoot, retainedRunRoot, profilePath, exactRunOwner }) {
  if (!RUN_ID.test(runId ?? "") || exactRunOwner !== runId) throw new Error("retained recovery requires the exact requested run owner");
  const root = resolve(orchestrationRoot);
  const retained = resolve(retainedRunRoot);
  if (root !== "/dev/shm/agentic-celld-orchestration" || retained !== join(root, runId)) {
    throw new Error("retained recovery path is not the exact same-run orchestration root");
  }
  if (!existsSync(retained)) {
    return { status: "PASS", run_id: runId, discovered_retained_inventory: false };
  }
  const protectedProfilePath = profilePath === null
    ? join(retained, "live-profile.json")
    : resolve(profilePath);
  if (profilePath !== null && protectedProfilePath !== join("/dev/shm/agentic-celld-storage", runId, "live-profile.json")) {
    throw new Error("retained recovery profile is outside the exact production storage run layout");
  }
  const runtime = loadProtectedOrchestrationRuntime(join(retained, "orchestration.json"), protectedProfilePath, { requireDestructiveAuthorization: false });
  if (runtime.runId !== runId || runtime.orchestrationInventory.owner?.run_id !== exactRunOwner) {
    throw new Error("retained orchestration inventory belongs to another run");
  }
  const result = await recoverOrchestrationInventory(runtime);
  for (const name of ["live-profile.json", "credential-provenance.json"]) {
    const path = join(retained, name);
    if (!existsSync(path)) continue;
    protectedJson(path, `retained ${name}`);
    rmSync(path, { force: false });
  }
  cleanupOrchestrationRoot(join(retained, "orchestration.json"));
  return { ...result, discovered_retained_inventory: true };
}

export function cleanupOrchestrationRoot(configPath, options = {}) {
  if (options.beforeRootDelete !== undefined && typeof options.beforeRootDelete !== "function") {
    throw new Error("orchestration cleanup root-deletion seam must be a function");
  }
  const config = protectedJson(resolve(configPath), "orchestration config");
  const errors = validateOrchestrationConfig(config);
  if (errors.length) throw new Error(errors.join("; "));
  if (resolve(configPath) !== join(config.working_root, "orchestration.json")) throw new Error("orchestration config is not at the fixed run-root path");
  let lifecycle;
  try {
    lifecycle = acquireOrchestrationInventoryLifecycle(config);
  } catch (error) {
    throw new OrchestrationCleanupResidueError(`orchestration cleanup could not acquire exact lifecycle exclusion: ${error.message}`, { cause: error });
  }
  try {
    const inventory = loadProtectedOrchestrationInventory(config.inventory_path, config, { expectedHostSha256: sha256(hostname()) });
    if (!isOrchestrationInventoryV2(inventory)) {
      throw new OrchestrationCleanupResidueError("orchestration inventory v1 is read-only and requires an explicit safe upgrade before cleanup");
    }
    const activeResources = inventory.resources.filter((resource) => resource.status !== "removed");
    const activeFaults = inventory.faults.filter((fault) => fault.status !== "healed");
    if (activeResources.length || activeFaults.length) {
      inventory.state = "cleanup_residue";
      inventory.updated_at = new Date().toISOString();
      commitOrchestrationInventory(config.inventory_path, inventory, {
        config,
        lifecycle,
        expectedJournalHeadSha256: inventory.journal_head_sha256,
      });
      throw new OrchestrationCleanupResidueError(`orchestration cleanup inventory retains ${activeResources.length} resources and ${activeFaults.length} faults`);
    }
    const grpcSocketNames = [];
    for (const entry of readdirSync(`/proc/self/fd/${lifecycle.root.descriptor}`, { withFileTypes: true })) {
      if (["orchestration.json", "orchestration-inventory.json", ".orchestration-inventory.lock"].includes(entry.name)) continue;
      if (/^grpc-[0-9a-f]{32}\.sock$/.test(entry.name)) {
        const socketPath = `/proc/self/fd/${lifecycle.root.descriptor}/${entry.name}`;
        const socketMetadata = lstatSync(socketPath);
        if (!entry.isSocket() || !socketMetadata.isSocket() || socketMetadata.isSymbolicLink() || socketMetadata.nlink !== 1) {
          throw new OrchestrationCleanupResidueError("orchestration cleanup found ambiguous managed gRPC socket residue");
        }
        grpcSocketNames.push(entry.name);
        continue;
      }
      if (!entry.isDirectory() || entry.isSymbolicLink()) throw new OrchestrationCleanupResidueError("orchestration cleanup found ambiguous residue");
      const scenarioRoot = `/proc/self/fd/${lifecycle.root.descriptor}/${entry.name}`;
      if (readdirSync(scenarioRoot).length !== 0) throw new OrchestrationCleanupResidueError(`orchestration scenario residue remains: ${entry.name}`);
    }
    inventory.state = "clean";
    inventory.updated_at = new Date().toISOString();
    commitOrchestrationInventory(config.inventory_path, inventory, { config, lifecycle });
    for (const name of grpcSocketNames) {
      const socketPath = `/proc/self/fd/${lifecycle.root.descriptor}/${name}`;
      const socketMetadata = lstatSync(socketPath);
      if (!socketMetadata.isSocket() || socketMetadata.isSymbolicLink() || socketMetadata.nlink !== 1) {
        throw new OrchestrationCleanupResidueError("orchestration managed gRPC socket identity changed before deletion");
      }
      rmSync(socketPath, { force: false });
    }
    options.beforeRootDelete?.();
    let pathMetadata;
    try {
      pathMetadata = lstatSync(config.working_root);
    } catch (error) {
      throw new OrchestrationCleanupResidueError(`orchestration cleanup root identity changed before deletion: ${error.message}`);
    }
    if (!pathMetadata.isDirectory() || pathMetadata.isSymbolicLink()
        || pathMetadata.dev !== lifecycle.root.metadata.dev || pathMetadata.ino !== lifecycle.root.metadata.ino) {
      throw new OrchestrationCleanupResidueError("orchestration cleanup detected a substituted run-root identity before deletion");
    }
    rmSync(config.working_root, { recursive: true, force: false });
    return { status: "PASS", run_id: config.run_id, residue: [] };
  } finally {
    releaseOrchestrationInventoryLifecycle(lifecycle);
  }
}

async function main(args) {
  if (args[0] === "cleanup") {
    if (args.includes("--profile")) loadProtectedOrchestrationRuntime(argument(args, "--config"), argument(args, "--profile"), { requireDestructiveAuthorization: false });
    process.stdout.write(`${JSON.stringify(cleanupOrchestrationRoot(argument(args, "--config")))}\n`);
    return;
  }
  if (args[0] === "recover") {
    const runtime = loadProtectedOrchestrationRuntime(argument(args, "--config"), argument(args, "--profile"), { requireDestructiveAuthorization: false });
    process.stdout.write(`${JSON.stringify(await recoverOrchestrationInventory(runtime))}\n`);
    return;
  }
  if (args[0] === "recover-retained") {
    process.stdout.write(`${JSON.stringify(await recoverRetainedOrchestrationRun({
      runId: argument(args, "--run-id"),
      orchestrationRoot: argument(args, "--orchestration-root"),
      retainedRunRoot: argument(args, "--retained-run-root"),
      profilePath: optionalArgument(args, "--profile"),
      exactRunOwner: argument(args, "--exact-run-owner"),
    }))}\n`);
    return;
  }
  if (args[0] === "prepare") {
    const result = prepareOrchestrationConfig({
      runId: argument(args, "--run-id"), workingRoot: argument(args, "--working-root"),
      managementBinaryPath: argument(args, "--management-binary"), callbackRelayBinaryPath: argument(args, "--relay-binary"),
      agentClientBinaryPath: argument(args, "--agent-client-binary"),
      dockerImageRef: argument(args, "--docker-image-ref"), agentshareRoot: argument(args, "--agentshare-root"),
      managementGrpcPort: Number(argument(args, "--management-grpc-port")),
    });
    process.stdout.write(`${JSON.stringify({ status: "PASS", config_path: result.path, inventory_path: result.inventoryPath, run_id: result.config.run_id })}\n`);
    return;
  }
  const observation = await executeOrchestrationDriver({
    scenarioId: argument(args, "--scenario-id"), runId: argument(args, "--run-id"),
    liveProfilePath: resolve(argument(args, "--profile")), artifactDir: resolve(argument(args, "--artifact-dir")),
  });
  process.stdout.write(`${JSON.stringify(observation)}\n`);
}

if (process.argv[1] && SCRIPT_PATH === resolve(process.argv[1])) {
  main(process.argv.slice(2)).catch((error) => {
    if (error instanceof OrchestrationCleanupResidueError || error?.exitCode === 4) error.exitCode = 4;
    emitLiveDriverError("CELLD_ORCHESTRATION_DRIVER_ERROR", error);
  });
}
