#!/usr/bin/env node

import { spawn, spawnSync } from "node:child_process";
import { createHash, createHmac, randomBytes, randomUUID } from "node:crypto";
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
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
import { getWorkerCell, sendWorkerCommand } from "./celld-worker-client.mjs";
import { validateLiveProfile } from "./celld-uat-live-protocol.mjs";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
const OBSERVATION_SCHEMA = "agentic-sandbox.celld-live-observation/v1";
const CONFIG_SCHEMA = "agentic-sandbox.celld-live-orchestration/v1";
const INVENTORY_SCHEMA = "agentic-sandbox.celld-orchestration-inventory/v1";
const DRIVER_ID = "celld-live-orchestration";
const DRIVER_VERSION = "celld-live-orchestration/v1";
const DISPATCH_GATE_SCHEMA = "agentic-sandbox.celld-dispatch-gate/v1";
const CRASH_PHASE_EVIDENCE_SCHEMA = "agentic-sandbox.celld-crash-phase-evidence/v1";
const ORCHESTRATION_OWNER = Object.freeze({ repository: "roctinam/agentic-sandbox", workflow: "celld-qualification.yml" });
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

function available(program, args = ["--version"]) {
  return spawnSync(program, args, { encoding: "utf8", shell: false, timeout: 15_000 }).status === 0;
}

function protectedJson(path, description) {
  if (!isAbsolute(path) || !existsSync(path)) throw new Error(`${description} is missing`);
  const metadata = lstatSync(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) throw new Error(`${description} must be a protected regular non-symlink file`);
  return JSON.parse(readFileSync(path, "utf8"));
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

export function validateOrchestrationInventory(inventory, config, { expectedHostSha256 } = {}) {
  const errors = [];
  if (!inventory || typeof inventory !== "object" || Array.isArray(inventory)) return ["inventory must be an object"];
  const allowed = new Set(["schema_version", "run_id", "working_root", "owner", "host_sha256", "created_at", "updated_at", "state", "resources", "faults"]);
  for (const key of Object.keys(inventory)) if (!allowed.has(key)) errors.push(`inventory.${key} is not allowed`);
  if (inventory.schema_version !== INVENTORY_SCHEMA) errors.push(`inventory.schema_version must be ${INVENTORY_SCHEMA}`);
  if (inventory.run_id !== config.run_id
      || inventory.working_root !== config.working_root
      || inventory.owner?.repository !== ORCHESTRATION_OWNER.repository
      || inventory.owner?.workflow !== ORCHESTRATION_OWNER.workflow
      || inventory.owner?.run_id !== config.run_id) errors.push("inventory run/owner does not match orchestration config");
  if (!SHA256.test(inventory.host_sha256 ?? "") || (expectedHostSha256 && inventory.host_sha256 !== expectedHostSha256)) errors.push("inventory host does not match the authorized host");
  if (!validTimestamp(inventory.created_at) || !validTimestamp(inventory.updated_at)) errors.push("inventory timestamps are invalid");
  if (!["prepared", "active", "cleanup_residue", "clean"].includes(inventory.state)) errors.push("inventory state is invalid");
  if (!Array.isArray(inventory.resources) || !Array.isArray(inventory.faults)) errors.push("inventory resources/faults must be arrays");

  const resourceKeys = new Set();
  for (const [index, resource] of (inventory.resources ?? []).entries()) {
    const allowedResource = new Set(["scenario_id", "instance_id", "name", "substrate", "status", "planned_at", "updated_at", "removed_at"]);
    for (const key of Object.keys(resource ?? {})) if (!allowedResource.has(key)) errors.push(`inventory.resources[${index}].${key} is not allowed`);
    const key = resource?.instance_id ?? "";
    if (resourceKeys.has(key)) errors.push(`inventory resource is duplicated: ${key}`);
    resourceKeys.add(key);
    if (!SCENARIOS.has(resource?.scenario_id)
        || !/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(resource?.instance_id ?? "")
        || !/^celld-[a-z0-9-]{1,62}$/.test(resource?.name ?? "")
        || !SUBSTRATES.includes(resource?.substrate)
        || !["planned", "removed"].includes(resource?.status)
        || !validTimestamp(resource?.planned_at)
        || !validTimestamp(resource?.updated_at)
        || (resource?.status === "removed" && !validTimestamp(resource?.removed_at))) errors.push(`inventory resource is invalid at index ${index}`);
  }

  const faultIds = new Set();
  for (const [index, fault] of (inventory.faults ?? []).entries()) {
    const allowedFault = new Set(["id", "scenario_id", "kind", "target", "status", "planned_at", "updated_at", "applied_at", "healed_at"]);
    for (const key of Object.keys(fault ?? {})) if (!allowedFault.has(key)) errors.push(`inventory.faults[${index}].${key} is not allowed`);
    if (faultIds.has(fault?.id)) errors.push(`inventory fault is duplicated: ${fault?.id}`);
    faultIds.add(fault?.id);
    if (!/^[0-9a-f]{32}$/.test(fault?.id ?? "")
        || !SCENARIOS.has(fault?.scenario_id)
        || !FAULT_KINDS.has(fault?.kind)
        || !/^(?:management|celld-[a-z0-9-]{1,80})$/.test(fault?.target ?? "")
        || !["planned", "applied", "healed"].includes(fault?.status)
        || !validTimestamp(fault?.planned_at)
        || !validTimestamp(fault?.updated_at)
        || (["applied", "healed"].includes(fault?.status) && !validTimestamp(fault?.applied_at))
        || (fault?.status === "healed" && !validTimestamp(fault?.healed_at))) errors.push(`inventory fault is invalid at index ${index}`);
  }
  return errors;
}

export function loadAuthorizedOrchestrationInventory(profile, config, expectedHostSha256) {
  if (profile.authorization?.destructive_faults !== true || profile.authorization?.exact_run_owner !== profile.run_id) throw new Error("exact-run destructive authorization is required");
  if (resolve(profile.authorization.inventory_path ?? "") !== resolve(config.inventory_path ?? "")) throw new Error("authorization inventory path is not the fixed orchestration inventory");
  const inventory = protectedJson(config.inventory_path, "orchestration inventory");
  const errors = validateOrchestrationInventory(inventory, config, { expectedHostSha256 });
  if (errors.length) throw new Error(errors.join("; "));
  return inventory;
}

function executable(path, description) {
  if (!isAbsolute(path) || !existsSync(path)) return `${description} is missing`;
  const metadata = lstatSync(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o111) === 0) return `${description} must be an executable regular non-symlink file`;
  return null;
}

export function validateOrchestrationConfig(config, { repoRoot = REPO_ROOT } = {}) {
  const errors = [];
  if (!config || typeof config !== "object" || Array.isArray(config)) return ["config must be an object"];
  const allowed = new Set([
    "schema_version", "run_id", "working_root", "inventory_path", "management_binary_path", "agent_client_binary_path",
    "callback_relay_binary_path", "docker_image_ref", "base_images_dir",
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
  const config = {
    schema_version: CONFIG_SCHEMA,
    run_id: runId,
    working_root: resolvedWorkingRoot,
    inventory_path: join(resolvedWorkingRoot, "orchestration-inventory.json"),
    management_binary_path: resolve(managementBinaryPath),
    agent_client_binary_path: resolve(agentClientBinaryPath),
    callback_relay_binary_path: resolve(callbackRelayBinaryPath),
    docker_image_ref: dockerImageRef,
    base_images_dir: "/build/agentic-sandbox/base-images",
    vm_storage_dir: "/build/agentic-sandbox/vms",
    agentshare_root: resolve(agentshareRoot),
    libvirt_uri: "qemu:///system",
    management_grpc_port: managementGrpcPort,
  };
  const errors = validateOrchestrationConfig(config);
  if (errors.length) throw new Error(errors.join("; "));
  if (existsSync(config.working_root)) throw new Error("orchestration working root already exists");
  const path = join(config.working_root, "orchestration.json");
  const timestamp = now.toISOString();
  const inventory = {
    schema_version: INVENTORY_SCHEMA,
    run_id: config.run_id,
    working_root: config.working_root,
    owner: { ...ORCHESTRATION_OWNER, run_id: config.run_id },
    host_sha256: sha256(host),
    created_at: timestamp,
    updated_at: timestamp,
    state: "prepared",
    resources: [],
    faults: [],
  };
  const inventoryErrors = validateOrchestrationInventory(inventory, config, { expectedHostSha256: inventory.host_sha256 });
  if (inventoryErrors.length) throw new Error(inventoryErrors.join("; "));
  try {
    mkdirSync(config.working_root, { recursive: true, mode: 0o700 });
    chmodSync(config.working_root, 0o700);
    writeFileSync(path, `${JSON.stringify(config, null, 2)}\n`, { mode: 0o600, flag: "wx" });
    chmodSync(path, 0o600);
    writeFileSync(config.inventory_path, `${JSON.stringify(inventory, null, 2)}\n`, { mode: 0o600, flag: "wx" });
    chmodSync(config.inventory_path, 0o600);
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

function storageGateway(config) {
  const document = JSON.parse(run("docker", ["network", "inspect", config.network.name]));
  const gateway = document?.[0]?.IPAM?.Config?.[0]?.Gateway;
  if (!/^(?:10\.|192\.168\.|172\.(?:1[6-9]|2\d|3[01])\.)/.test(gateway ?? "")) throw new Error("fleet network gateway is not private IPv4");
  return gateway;
}

function workerEndpoint(config, nodeIndex = 0) {
  const port = run("docker", ["port", config.nodes[nodeIndex].name, "8080/tcp"]);
  const match = /^127\.0\.0\.1:(\d+)$/.exec(port.trim());
  if (!match) throw new Error("Celld Worker endpoint is not host-loopback only");
  return `http://127.0.0.1:${match[1]}`;
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

function managementEnvironment(config, fleet, managementHost) {
  const stateRoot = join(fleet.run_root, "management-state");
  const secrets = join(stateRoot, "secrets");
  const dispatchGates = join(stateRoot, "dispatch-gates");
  mkdirSync(secrets, { recursive: true, mode: 0o700 });
  chmodSync(secrets, 0o700);
  mkdirSync(dispatchGates, { recursive: true, mode: 0o700 });
  chmodSync(dispatchGates, 0o700);
  const workerVars = readWorkerKey(fleet.worker_vars_file_ref);
  return {
    ...process.env,
    LISTEN_ADDR: `127.0.0.1:${config.management_grpc_port}`,
    SECRETS_DIR: secrets,
    AIWG_TLS_LISTEN: `${managementHost}:8122`,
    AIWG_TLS_CERT: fleet.callback.management_server_cert_file_ref,
    AIWG_TLS_KEY: fleet.callback.management_server_key_file_ref,
    AIWG_TLS_CLIENT_CA: fleet.callback.ca_file_ref,
    AIWG_TLS_CLIENT_AUTH: "required",
    AIWG_MTLS_ADMIN_ALLOWLIST: "",
    AGENTIC_CELLD_ENABLED: "1",
    AGENTIC_CELLD_ENDPOINT: workerEndpoint(fleet),
    AGENTIC_CELLD_AUTH_KEY_ID: workerVars.keyId,
    AGENTIC_CELLD_AUTH_KEY_FILE: fleet.callback.management_auth_key_file_ref,
    AGENTIC_CELLD_EFFECT_LEDGER_PATH: fleet.callback.effect_ledger_file_ref,
    AGENTIC_CELLD_QUALIFICATION_DISPATCH_GATE_DIR: dispatchGates,
    AGENTIC_CELLD_CALLBACK_MTLS_CN: fleet.callback.client_cn,
    AGENTIC_CELLD_VERSION: fleet.pins.celld.version,
    AGENTIC_CELLD_COMMIT: fleet.pins.celld.commit,
    AGENTIC_GRPC_UDS: join(secrets, "agent-grpc.sock"),
    AGENTIC_GRPC_VSOCK_PORT: "0",
    BASE_IMAGES_DIR: config.base_images_dir,
    VM_STORAGE_DIR: config.vm_storage_dir,
    AGENTSHARE_ROOT: config.agentshare_root,
    LIBVIRT_DEFAULT_URI: config.libvirt_uri,
    AGENTIC_BACKEND: "libvirt",
    AGENT_CLIENT_SOURCE_BIN: config.agent_client_binary_path,
    RUST_LOG: "info",
  };
}

function launchManagement(config, fleet, managementHost) {
  const logPath = join(fleet.run_root, "management-state", "management.log");
  const processHandle = spawn(config.management_binary_path, [], {
    cwd: REPO_ROOT,
    env: managementEnvironment(config, fleet, managementHost),
    shell: false,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const append = (chunk) => {
    const previous = existsSync(logPath) ? readFileSync(logPath) : Buffer.alloc(0);
    const combined = Buffer.concat([previous, chunk]).subarray(-1024 * 1024);
    writeFileSync(logPath, combined, { mode: 0o600 });
  };
  processHandle.stdout.on("data", append);
  processHandle.stderr.on("data", append);
  return { processHandle, logPath, managementHost };
}

function stopManagement(management, signal = "SIGTERM") {
  if (!management?.processHandle || management.processHandle.exitCode !== null || management.processHandle.signalCode !== null) return;
  management.processHandle.kill(signal);
}

async function stopManagementAndWait(management, signal = "SIGTERM") {
  if (!management?.processHandle || management.processHandle.exitCode !== null || management.processHandle.signalCode !== null) return;
  stopManagement(management, signal);
  try {
    await waitFor(() => management.processHandle.exitCode !== null || management.processHandle.signalCode !== null, { timeoutMs: 15_000, intervalMs: 100, description: "management process exit" });
  } catch {
    stopManagement(management, "SIGKILL");
    await waitFor(() => management.processHandle.exitCode !== null || management.processHandle.signalCode !== null, { timeoutMs: 15_000, intervalMs: 100, description: "forced management process exit" });
  }
}

async function waitManagement(management, fleet) {
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

async function restartManagement(management, config, fleet, managementHost) {
  await stopManagementAndWait(management, "SIGKILL");
  const restarted = launchManagement(config, fleet, managementHost);
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
    ? { name, runtime: "qemu", provider: "libvirt", start: false, agentshare: false }
    : { name, runtime: "docker", image: config.docker_image_ref, start: false, agentshare: false };
}

function persistOrchestrationInventory(runtime, now = new Date()) {
  runtime.orchestrationInventory.updated_at = now.toISOString();
  runtime.orchestrationInventory.state = runtime.orchestrationInventory.resources.some((entry) => entry.status !== "removed")
    || runtime.orchestrationInventory.faults.some((entry) => entry.status !== "healed") ? "active" : "prepared";
  const errors = validateOrchestrationInventory(runtime.orchestrationInventory, runtime.config, { expectedHostSha256: runtime.orchestrationInventory.host_sha256 });
  if (errors.length) throw new Error(errors.join("; "));
  (runtime.persistInventory ?? atomicJson)(runtime.config.inventory_path, runtime.orchestrationInventory);
}

function planProviderResource(runtime, { instanceId, name, substrate }, now = new Date()) {
  const existing = runtime.orchestrationInventory.resources.find((entry) => entry.instance_id === instanceId);
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
    return {
      ...base,
      present: true,
      state,
      provider_storage_present: storagePresent,
      provider_identity_sha256: sha256(`qemu:${uuid}`),
      configuration_sha256: sha256(xml),
    };
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
      '{{.Id}}|{{.State.Status}}|{{.Config.Image}}|{{index .Config.Labels "agentic-instance-id"}}|{{index .Config.Labels "agentic-source"}}',
      ids[0],
    ], { timeout: 30_000 }).split("|");
    if (inspected.length !== 5 || !/^[0-9a-f]{64}$/.test(inspected[0]) || inspected[3] !== instanceId || inspected[4] !== "admin-v2") {
      throw new Error("Docker provider identity is not bound to the owned management resource");
    }
    const state = inspected[1];
    if (!["created", "running", "restarting", "exited", "paused", "dead", "removing"].includes(state)) throw new Error("Docker provider state is invalid");
    return {
      ...base,
      present: true,
      state,
      provider_storage_present: false,
      provider_identity_sha256: sha256(`docker:${inspected[0]}`),
      configuration_sha256: sha256(canonicalJson({ image: inspected[2], instance_id: inspected[3], source: inspected[4] })),
    };
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

export async function applyPlannedOrchestrationFault(runtime, fault, apply) {
  if (typeof apply !== "function") throw new Error("fault apply callback is required");
  const record = planFault(runtime, fault);
  await apply();
  markFault(runtime, record, "applied");
  return record;
}

export async function healPlannedOrchestrationFault(runtime, record, heal) {
  if (typeof heal !== "function") throw new Error("fault heal callback is required");
  await heal();
  markFault(runtime, record, "healed");
}

export async function issueCommand(runtime, { instanceId, generation, operationId: id, action, payload }) {
  if (action === "provision") {
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
    const cell = await getWorkerCell({ endpoint: runtime.workerEndpoint, varsFile: runtime.fleet.worker_vars_file_ref, instanceId, operationId: `lookup-${randomBytes(8).toString("hex")}`, generation });
    if (cell.status !== 200) return false;
    const effect = cell.body.effects?.find((candidate) => candidate.operation_id === operationIdValue);
    return effect && acceptedStatuses.includes(effect.status) ? { cell: cell.body, effect } : false;
  }, { timeoutMs: 900_000, intervalMs: 250, description: `Worker effect ${operationIdValue}` });
}

async function runOneEffectCampaign(runtime, { prefix, substrate, instanceId, generation, action, payload, repeats = 0 }) {
  const id = operationId(prefix, substrate, generation, action);
  const effect = await issueCommand(runtime, { instanceId, generation, operationId: id, action, payload });
  const terminal = await terminalCallback(runtime, instanceId, generation, effect);
  if (terminal.body.status !== "succeeded") throw new Error(`${substrate} ${action} did not succeed: ${terminal.body.terminal_code ?? "unknown"}`);
  let replayStatuses = [];
  if (repeats > 0) {
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
  const cell = await waitCellEffect(runtime, instanceId, generation, id, ["succeeded"]);
  return { id, effect, terminal: terminal.body, replayStatuses, cell: cell.cell };
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
  for (const substrate of SUBSTRATES) {
    const instanceId = randomUUID();
    const name = `celld-recovery-${substrate}-${sha256(`${runtime.runId}:004`).slice(0, 8)}`;
    await runOneEffectCampaign(runtime, { prefix: "uat004-setup", substrate, instanceId, generation: 1, action: "provision", payload: provisionPayload(runtime.config, substrate, name) });
    await runOneEffectCampaign(runtime, { prefix: "uat004-setup", substrate, instanceId, generation: 1, action: "start", payload: {} });
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
        try {
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
          effect = await issueCommand(runtime, {
            instanceId,
            generation: 1,
            operationId: id,
            action: "observe",
            payload: { substrate, crash_point: crashPoint, trial: index + 1 },
          });
          acknowledgedAt = new Date().toISOString();

          if (crashPoint !== "before_dispatch") {
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
          nodeFault = await applyPlannedOrchestrationFault(runtime, { kind: "fleet_node_stop", target: ownershipBefore.owner_target }, () => run("docker", ["stop", "--time", "5", ownershipBefore.owner_target], { timeout: 30_000 }));
          const fallbackIndex = runtime.fleet.nodes.findIndex((node) => node.name !== ownershipBefore.owner_target);
          if (fallbackIndex < 0) throw new Error("owner-loss campaign has no surviving Worker endpoint");
          runtime.workerEndpoint = workerEndpoint(runtime.fleet, fallbackIndex);
          ownershipAfterLoss = await waitFor(async () => {
            const observation = await observeCelldOwnership(runtime, { instanceId });
            return observation.owner_target !== ownershipBefore.owner_target && observation.owner_epoch > ownershipBefore.owner_epoch
              ? observation
              : false;
          }, { timeoutMs: 30_000, intervalMs: 250, description: "Celld owner takeover and epoch advance" });

          runtime.management = await restartManagement(runtime.management, runtime.config, runtime.fleet, runtime.managementHost);
          await healPlannedOrchestrationFault(runtime, managementFault, async () => {});
          const terminal = await terminalCallback(runtime, instanceId, 1, effect);
          const cell = await waitCellEffect(runtime, instanceId, 1, id, ["succeeded"]);
          recoveryMs = Date.now() - faultStarted;
          if (terminal.body.status !== "succeeded") throw new Error("restart trial did not converge to success");
          effect = { ...effect, terminal, cell };
        } finally {
          clearDispatchGate(runtime, id);
          if (runtime.management.processHandle.exitCode !== null || runtime.management.processHandle.signalCode !== null || runtime.management.processHandle.killed) {
            runtime.management = await restartManagement(runtime.management, runtime.config, runtime.fleet, runtime.managementHost);
          }
          if (managementFault?.status === "applied") await healPlannedOrchestrationFault(runtime, managementFault, async () => {});
          if (nodeFault?.status === "applied") {
            await healPlannedOrchestrationFault(runtime, nodeFault, async () => {
              run("docker", ["start", ownershipBefore.owner_target], { timeout: 30_000 });
              await waitFor(() => diagnoseFleet(runtime.fleetPath).status === "READY", { timeoutMs: 30_000, intervalMs: 250, description: "three-node fleet heal" });
            });
          }
          runtime.workerEndpoint = workerEndpoint(runtime.fleet, 0);
        }

        const ownershipAfterHeal = await waitFor(async () => {
          const observation = await observeCelldOwnership(runtime, { instanceId });
          return observation.live_nodes === 3 ? observation : false;
        }, { timeoutMs: 30_000, intervalMs: 250, description: "healed three-node owner observation" });
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
        if (action === "provision") planProviderResource(runtime, { instanceId, name, substrate });
        const providerBefore = observeOrchestrationProvider(runtime, { instanceId, name, substrate });
        const responseLossFault = await applyPlannedOrchestrationFault(runtime, { kind: "callback_response_loss", target: relayName(runtime.fleet) }, () => run("docker", ["kill", "--signal", "SIGUSR1", relayName(runtime.fleet)]));
        const id = operationId("uat005", substrate, generation, action);
        const started = Date.now();
        const originalEffect = await issueCommand(runtime, { instanceId, generation, operationId: id, action, payload: payloads[action] });
        const unknown = await waitCellEffect(runtime, instanceId, generation, id, ["unknown"]);
        const terminal = await waitCellEffect(runtime, instanceId, generation, id, ["succeeded", "failed", "rejected"]);
        if (terminal.effect.status !== "succeeded") throw new Error(`${substrate} ${action} response-loss recovery failed`);
        const managementReplay = await callbackRequest(callbackContext(runtime.fleet, runtime.managementHost, instanceId, generation), originalEffect);
        const providerAfter = observeOrchestrationProvider(runtime, { instanceId, name, substrate });
        const record = {
          substrate,
          action,
          trial: generation,
          operation_id_sha256: sha256(id),
          original_id_match: unknown.effect.operation_id === id && terminal.effect.operation_id === id,
          replacement_id_observed: unknown.effect.operation_id !== id || terminal.effect.operation_id !== id,
          effect_records: terminal.cell.effects.filter((effect) => effect.operation_id === id).length,
          management_replay_status: managementReplay.status,
          management_replay_terminal_matches: managementReplay.body?.status === terminal.effect.status,
          provider_dispatch_count_observed: Number.isInteger(managementReplay.body?.provider_dispatch_count),
          provider_dispatch_count: managementReplay.body?.provider_dispatch_count ?? 0,
          attempts: terminal.effect.attempts,
          unknown_observed: true,
          convergence_ms: Date.now() - started,
          provider_before: providerBefore,
          provider_after: providerAfter,
        };
        cases.push(record);
        timeline.push({ scenario: "UAT-CELLD-005", kind: "response_loss_trial", ...record });
        await healPlannedOrchestrationFault(runtime, responseLossFault, async () => {});
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
    const partitionFault = await applyPlannedOrchestrationFault(runtime, { kind: "callback_relay_pause", target: relayName(runtime.fleet) }, () => run("docker", ["pause", relayName(runtime.fleet)]));
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
      await healPlannedOrchestrationFault(runtime, partitionFault, () => run("docker", ["unpause", relayName(runtime.fleet)]));
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

function cleanupProviderResources(runtime) {
  if (!runtime) return [];
  const assertions = [];
  for (const resource of runtime.providerResources.values()) {
    if (!/^celld-[a-z0-9-]{1,62}$/.test(resource.name) || !/^[0-9a-f-]{36}$/.test(resource.instanceId)) throw new Error("refusing an unsafe provider cleanup identity");
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
      markProviderResourceRemoved(runtime, resource.instanceId, resource.substrate);
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
    markProviderResourceRemoved(runtime, resource.instanceId, resource.substrate);
    runtime.providerResources.delete(resource.instanceId);
    assertions.push(`QEMU provider identity ${sha256(resource.instanceId)} absent`);
  }
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

function prerequisiteReason(config) {
  if (process.platform !== "linux") return "CELLD_ORCHESTRATION_LINUX_REQUIRED";
  for (const [program, args, code] of [["docker", ["compose", "version"], "CELLD_DOCKER_UNAVAILABLE"], ["virsh", ["-c", config.libvirt_uri, "version"], "CELLD_LIBVIRT_UNAVAILABLE"], ["openssl", ["version"], "CELLD_OPENSSL_UNAVAILABLE"]]) if (!available(program, args)) return code;
  if (executable(config.management_binary_path, "management binary")) return "CELLD_MANAGEMENT_BINARY_UNAVAILABLE";
  if (executable(config.agent_client_binary_path, "agent client binary")) return "CELLD_AGENT_CLIENT_BINARY_UNAVAILABLE";
  if (executable(config.callback_relay_binary_path, "callback relay")) return "CELLD_CALLBACK_RELAY_UNAVAILABLE";
  if (spawnSync("docker", ["image", "inspect", config.docker_image_ref], { encoding: "utf8", shell: false }).status !== 0) return "CELLD_DOCKER_IMAGE_UNAVAILABLE";
  return null;
}

export async function executeOrchestrationDriver({ scenarioId, runId, liveProfilePath, artifactDir }, dependencies = {}) {
  const startedAt = new Date().toISOString();
  const profile = protectedJson(liveProfilePath, "live profile");
  const profileErrors = validateLiveProfile(profile);
  if (profileErrors.length) throw new Error(profileErrors.join("; "));
  const entry = profile.drivers?.[DRIVER_ID];
  if (!entry?.enabled) return unavailable({ scenarioId, runId, profile, startedAt, reasonCode: "CELLD_ORCHESTRATION_DRIVER_DISABLED" });
  if (!SCENARIOS.has(scenarioId) || profile.run_id !== runId || profile.expected_sandbox_git !== (dependencies.gitCommit?.() ?? run("git", ["rev-parse", "HEAD"]))) throw new Error("live orchestration identity does not match the requested run");
  const observedHostSha256 = sha256(dependencies.hostname?.() ?? hostname());
  if (profile.environment.host_sha256 !== observedHostSha256) throw new Error("live orchestration host identity does not match the protected profile");
  const config = protectedJson(entry.config_path, "orchestration config");
  const configErrors = validateOrchestrationConfig(config);
  if (configErrors.length) throw new Error(configErrors.join("; "));
  if (config.run_id !== runId) throw new Error("orchestration config run identity does not match the live profile");
  if (!profile.authorization.destructive_faults || profile.authorization.exact_run_owner !== profile.run_id) return unavailable({ scenarioId, runId, profile, startedAt, reasonCode: "CELLD_DESTRUCTIVE_AUTHORIZATION_REQUIRED" });
  const orchestrationInventory = loadAuthorizedOrchestrationInventory(profile, config, observedHostSha256);
  const reason = dependencies.prerequisiteReason ? dependencies.prerequisiteReason(config, scenarioId, profile) : prerequisiteReason(config);
  if (reason) return unavailable({ scenarioId, runId, profile, startedAt, reasonCode: reason });

  mkdirSync(artifactDir, { recursive: true, mode: 0o700 });
  chmodSync(artifactDir, 0o700);
  const timeline = [];
  let fixture = null;
  let fleet = null;
  let fleetPath = null;
  let management = null;
  let runtime = null;
  let cleanupStatus = "failed";
  const cleanupAssertions = [];
  let campaign;
  try {
    const scenarioRoot = join(config.working_root, scenarioId.toLowerCase(), runId);
    fixture = prepareFixture({ fixtureProfile: "titan-single-host-storage", runId, root: scenarioRoot });
    startFixture(fixture);
    fleet = prepareFleet({ storageConfigPath: join(scenarioRoot, "fixture.json") });
    fleetPath = join(scenarioRoot, "fleet.json");
    const fleetErrors = validateFleetConfig(fleet);
    if (fleetErrors.length) throw new Error(fleetErrors.join("; "));
    await deployFleetWorker(fleetPath);
    const diagnosis = startFleet(fleetPath);
    if (diagnosis.status !== "READY") throw new Error("three-node Celld fleet is not ready");
    const managementHost = storageGateway(fleet);
    management = launchManagement(config, fleet, managementHost);
    await waitManagement(management, fleet);
    startCallbackRelays(fleetPath, { relayBinaryPath: config.callback_relay_binary_path, enableFaultSignal: ["UAT-CELLD-005"].includes(scenarioId) });
    const activeResources = orchestrationInventory.resources.filter((resource) => resource.status !== "removed").map((resource) => [resource.instance_id, { instanceId: resource.instance_id, name: resource.name, substrate: resource.substrate }]);
    runtime = {
      config, fleet, fleetPath, management, managementHost, workerEndpoint: workerEndpoint(fleet), runId, scenarioId,
      orchestrationInventory, providerResources: new Map(activeResources),
      persistInventory: dependencies.persistInventory,
      sendWorkerCommand: dependencies.sendWorkerCommand,
    };
    const runner = dependencies.runScenario ?? ({
      "UAT-CELLD-003": runUat003, "UAT-CELLD-004": runUat004,
      "UAT-CELLD-005": runUat005, "UAT-CELLD-006": runUat006,
    })[scenarioId];
    campaign = await runner(runtime, timeline);
    management = runtime.management;
  } finally {
    try { await stopManagementAndWait(runtime?.management ?? management, "SIGKILL"); cleanupAssertions.push("management process terminated"); } catch (error) { cleanupAssertions.push(`management cleanup digest ${sha256(error.message)}`); }
    try { cleanupAssertions.push(...cleanupProviderResources(runtime)); } catch (error) { cleanupAssertions.push(`provider cleanup digest ${sha256(error.message)}`); }
    try { if (fleetPath && existsSync(fleetPath)) cleanupFleet(fleetPath); cleanupAssertions.push("exact fleet containers and protected fleet state removed"); } catch (error) { cleanupAssertions.push(`fleet cleanup digest ${sha256(error.message)}`); }
    try { if (fixture) cleanupFixture(fixture); cleanupAssertions.push("exact storage services, network, volumes, and run directory removed"); } catch (error) { cleanupAssertions.push(`storage cleanup digest ${sha256(error.message)}`); }
    cleanupStatus = cleanupAssertions.some((value) => value.includes("digest")) ? "failed" : "passed";
  }
  if (!campaign) throw new Error("orchestration campaign produced no measurements");

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

export function cleanupOrchestrationRoot(configPath) {
  const config = protectedJson(resolve(configPath), "orchestration config");
  const errors = validateOrchestrationConfig(config);
  if (errors.length) throw new Error(errors.join("; "));
  if (resolve(configPath) !== join(config.working_root, "orchestration.json")) throw new Error("orchestration config is not at the fixed run-root path");
  const inventory = protectedJson(config.inventory_path, "orchestration inventory");
  const inventoryErrors = validateOrchestrationInventory(inventory, config, { expectedHostSha256: sha256(hostname()) });
  if (inventoryErrors.length) throw new Error(inventoryErrors.join("; "));
  const activeResources = inventory.resources.filter((resource) => resource.status !== "removed");
  const activeFaults = inventory.faults.filter((fault) => fault.status !== "healed");
  if (activeResources.length || activeFaults.length) {
    inventory.state = "cleanup_residue";
    inventory.updated_at = new Date().toISOString();
    atomicJson(config.inventory_path, inventory);
    throw new Error(`orchestration cleanup inventory retains ${activeResources.length} resources and ${activeFaults.length} faults`);
  }
  for (const entry of readdirSync(config.working_root, { withFileTypes: true })) {
    if (entry.name === "orchestration.json" || entry.name === "orchestration-inventory.json") continue;
    if (!entry.isDirectory() || entry.isSymbolicLink()) throw new Error("orchestration cleanup found ambiguous residue");
    const scenarioRoot = join(config.working_root, entry.name);
    if (readdirSync(scenarioRoot).length !== 0) throw new Error(`orchestration scenario residue remains: ${entry.name}`);
  }
  inventory.state = "clean";
  inventory.updated_at = new Date().toISOString();
  atomicJson(config.inventory_path, inventory);
  rmSync(config.working_root, { recursive: true, force: false });
  return { status: "PASS", run_id: config.run_id, residue: [] };
}

async function main(args) {
  if (args[0] === "cleanup") {
    process.stdout.write(`${JSON.stringify(cleanupOrchestrationRoot(argument(args, "--config")))}\n`);
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
    process.stderr.write(`CELLD_ORCHESTRATION_DRIVER_ERROR ${sha256(error.message)}\n`);
    process.exitCode = 3;
  });
}
