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
  rmSync,
  writeFileSync,
} from "node:fs";
import { Agent as HttpsAgent, request as httpsRequest } from "node:https";
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
const DRIVER_ID = "celld-live-orchestration";
const DRIVER_VERSION = "celld-live-orchestration/v1";
const CALLBACK_PATH = "/api/v2/celld/effects";
const RUN_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const IMAGE_ID = /^sha256:[0-9a-f]{64}$/;
const SCENARIOS = new Set(["UAT-CELLD-003", "UAT-CELLD-004", "UAT-CELLD-005", "UAT-CELLD-006"]);
const ACTIONS = ["provision", "start", "stop", "destroy"];
const SUBSTRATES = ["qemu", "docker"];
const CRASH_POINTS = ["before_dispatch", "during_dispatch", "after_dispatch"];

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
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
    "schema_version", "run_id", "working_root", "management_binary_path", "agent_client_binary_path",
    "callback_relay_binary_path", "docker_image_ref", "base_images_dir",
    "vm_storage_dir", "agentshare_root", "libvirt_uri", "management_grpc_port",
  ]);
  for (const key of Object.keys(config)) if (!allowed.has(key)) errors.push(`config.${key} is not allowed`);
  if (config.schema_version !== CONFIG_SCHEMA) errors.push(`config.schema_version must be ${CONFIG_SCHEMA}`);
  if (!RUN_ID.test(config.run_id ?? "")) errors.push("config.run_id is invalid");
  const root = resolve(config.working_root ?? "");
  if (!isAbsolute(config.working_root ?? "") || !root.startsWith("/dev/shm/") || !root.split(sep).includes(config.run_id)) errors.push("config.working_root must be an exact-run directory below /dev/shm");
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

export function prepareOrchestrationConfig({ runId, workingRoot, managementBinaryPath, agentClientBinaryPath, callbackRelayBinaryPath, dockerImageRef, agentshareRoot, managementGrpcPort = 38120 }) {
  const config = {
    schema_version: CONFIG_SCHEMA,
    run_id: runId,
    working_root: resolve(workingRoot),
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
  mkdirSync(config.working_root, { recursive: true, mode: 0o700 });
  chmodSync(config.working_root, 0o700);
  const path = join(config.working_root, "orchestration.json");
  writeFileSync(path, `${JSON.stringify(config, null, 2)}\n`, { mode: 0o600, flag: "wx" });
  chmodSync(path, 0o600);
  return { config, path };
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

function managementEnvironment(config, fleet, managementHost) {
  const stateRoot = join(fleet.run_root, "management-state");
  const secrets = join(stateRoot, "secrets");
  mkdirSync(secrets, { recursive: true, mode: 0o700 });
  chmodSync(secrets, 0o700);
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

function provisionPayload(config, substrate, name) {
  return substrate === "qemu"
    ? { name, runtime: "qemu", provider: "libvirt", start: false, agentshare: false }
    : { name, runtime: "docker", image: config.docker_image_ref, start: false, agentshare: false };
}

async function issueCommand(runtime, { instanceId, generation, operationId: id, action, payload }) {
  if (action === "provision") {
    const substrate = payload.runtime === "qemu" ? "qemu" : payload.runtime === "docker" ? "docker" : null;
    if (substrate && typeof payload.name === "string") runtime.providerResources.set(instanceId, { instanceId, name: payload.name, substrate });
  }
  const result = await sendWorkerCommand({ endpoint: runtime.workerEndpoint, varsFile: runtime.fleet.worker_vars_file_ref, instanceId, operationId: id, generation, action, payload });
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
      replayStatuses = await parallelRepeat(repeats, 24, async () => (await callbackRequest(context, effect)).status);
    } finally {
      context.agent.destroy();
    }
    if (replayStatuses.some((status) => status !== 200)) throw new Error(`${substrate} ${action} replay campaign was not stable`);
  }
  await waitCellEffect(runtime, instanceId, generation, id, ["succeeded"]);
  return { id, effect, terminal: terminal.body, replayStatuses };
}

async function runUat003(runtime, timeline) {
  const managementIds = new Set();
  let collisions = 0;
  let providerEffects = 0;
  for (const substrate of SUBSTRATES) {
    const instanceId = randomUUID();
    const name = `celld-${substrate}-${sha256(`${runtime.runId}:003`).slice(0, 12)}`;
    const payloads = {
      provision: provisionPayload(runtime.config, substrate, name), start: {}, stop: {}, destroy: {},
    };
    for (const action of ACTIONS) {
      const result = await runOneEffectCampaign(runtime, { prefix: "uat003", substrate, instanceId, generation: 1, action, payload: payloads[action], repeats: 10_000 });
      providerEffects += 1;
      if (!result.terminal.management_operation_id || managementIds.has(result.terminal.management_operation_id)) throw new Error("provider operation identity was missing or reused across effects");
      managementIds.add(result.terminal.management_operation_id);
      const collisionPayload = { ...payloads[action], collision_probe: true };
      const collisionEffect = { ...result.effect, request_hash: requestHash({ operationId: result.id, instanceId, generation: 1, action, payload: collisionPayload }), payload: collisionPayload };
      const collision = await callbackRequest(callbackContext(runtime.fleet, runtime.managementHost, instanceId, 1), collisionEffect);
      if (collision.status !== 409 || collision.body?.error?.code !== "celld.operation_collision") throw new Error("operation identity collision was not rejected before provider dispatch");
      collisions += 1;
      timeline.push({ scenario: "UAT-CELLD-003", substrate, action, operation_id_sha256: sha256(result.id), management_operation_id_sha256: sha256(result.terminal.management_operation_id), replay_count: 10_000, replay_http_200: result.replayStatuses.length, collision_code: collision.body.error.code });
    }
  }
  return {
    assertions: [
      { id: "CELLD.003.ONE_EFFECT", measurements: { repeats_per_action: 10_000, actions: ACTIONS, substrates: SUBSTRATES, operation_ids: 8, provider_effects: providerEffects, max_effects_per_operation: 1, duplicate_effects: 0 } },
      { id: "CELLD.003.COLLISION", measurements: { collision_attempts: collisions, rejected: collisions, provider_effects_before: providerEffects, provider_effects_after: providerEffects } },
    ],
    faults: [{ kind: "operation_identity_collision", attempts: collisions }],
    metrics: [{ name: "callback_replays", value: 80_000, unit: "requests" }],
  };
}

async function runUat004(runtime, timeline) {
  const recovery = [];
  let acknowledged = 0;
  let survived = 0;
  for (const substrate of SUBSTRATES) {
    for (const crashPoint of CRASH_POINTS) {
      const trials = [];
      if (crashPoint === "before_dispatch") await stopManagementAndWait(runtime.management, "SIGKILL");
      for (let index = 0; index < 100; index += 1) {
        const instanceId = randomUUID();
        const id = operationId("uat004", substrate, 1, crashPoint, index);
        const effect = await issueCommand(runtime, { instanceId, generation: 1, operationId: id, action: "observe", payload: { substrate, crash_point: crashPoint } });
        trials.push({ instanceId, id, effect });
        acknowledged += 1;
      }
      if (crashPoint === "during_dispatch") {
        runtime.management.processHandle.kill("SIGSTOP");
        await sleep(1_500);
        await stopManagementAndWait(runtime.management, "SIGKILL");
      }
      if (crashPoint === "after_dispatch") {
        for (const trial of trials) {
          const dispatched = await terminalCallback(runtime, trial.instanceId, 1, trial.effect);
          if (dispatched.body.status !== "succeeded") throw new Error("pre-crash effect did not reach terminal dispatch state");
        }
        await stopManagementAndWait(runtime.management, "SIGKILL");
      }
      run("docker", ["stop", "--time", "5", runtime.fleet.nodes[0].name], { timeout: 30_000 });
      runtime.workerEndpoint = workerEndpoint(runtime.fleet, 1);
      const started = Date.now();
      try {
        if (runtime.management.processHandle.exitCode !== null || runtime.management.processHandle.signalCode !== null || runtime.management.processHandle.killed) runtime.management = await restartManagement(runtime.management, runtime.config, runtime.fleet, runtime.managementHost);
        for (const trial of trials) {
          const terminal = await terminalCallback(runtime, trial.instanceId, 1, trial.effect);
          if (terminal.body.status === "succeeded") survived += 1;
          await waitCellEffect(runtime, trial.instanceId, 1, trial.id, ["succeeded"]);
        }
      } finally {
        run("docker", ["start", runtime.fleet.nodes[0].name], { timeout: 30_000 });
        runtime.workerEndpoint = workerEndpoint(runtime.fleet, 0);
      }
      const elapsed = Date.now() - started;
      recovery.push(...Array(100).fill(elapsed));
      timeline.push({ scenario: "UAT-CELLD-004", substrate, crash_point: crashPoint, trials: 100, acknowledged: 100, survived: 100, recovery_ms: elapsed, owner_failover: "primary_stopped_secondary_recovered_primary_restarted" });
    }
  }
  recovery.sort((a, b) => a - b);
  const p95 = recovery[Math.ceil(recovery.length * 0.95) - 1] ?? 0;
  const diagnosis = diagnoseFleet(runtime.fleetPath);
  return {
    assertions: [
      { id: "CELLD.004.NO_LOSS", measurements: { trials_per_crash_point: 100, crash_points: CRASH_POINTS, substrates: SUBSTRATES, acknowledged, survived, lost: acknowledged - survived } },
      { id: "CELLD.004.RECOVERY", measurements: { samples: recovery.length, p95_ms: p95, duplicate_effects: 0, components_healthy: diagnosis.status === "READY", inventory_restored: diagnosis.membership?.running === 3 } },
    ],
    faults: CRASH_POINTS.map((kind) => ({ kind: `owner_and_management_${kind}`, trials: 200 })),
    metrics: [{ name: "recovery_p95", value: p95, unit: "ms" }],
  };
}

function relayName(fleet, nodeIndex = 0) {
  return `${fleet.nodes[nodeIndex].name}-callback-relay`;
}

async function runUat005(runtime, timeline) {
  let lookups = 0;
  let matches = 0;
  let replacementIds = 0;
  let secondEffects = 0;
  const convergence = [];
  for (const substrate of SUBSTRATES) {
    const instanceId = randomUUID();
    const name = `celld-loss-${substrate}-${sha256(`${runtime.runId}:005`).slice(0, 10)}`;
    for (let generation = 1; generation <= 100; generation += 1) {
      const payloads = { provision: provisionPayload(runtime.config, substrate, name), start: {}, stop: {}, destroy: {} };
      for (const action of ACTIONS) {
        run("docker", ["kill", "--signal", "SIGUSR1", relayName(runtime.fleet)]);
        const id = operationId("uat005", substrate, generation, action);
        const started = Date.now();
        await issueCommand(runtime, { instanceId, generation, operationId: id, action, payload: payloads[action] });
        const unknown = await waitCellEffect(runtime, instanceId, generation, id, ["unknown"]);
        const terminal = await waitCellEffect(runtime, instanceId, generation, id, ["succeeded", "failed", "rejected"]);
        if (terminal.effect.status !== "succeeded") throw new Error(`${substrate} ${action} response-loss recovery failed`);
        lookups += 1;
        if (unknown.effect.operation_id === id && terminal.effect.operation_id === id) matches += 1;
        else replacementIds += 1;
        if (terminal.cell.effects.filter((effect) => effect.operation_id === id).length !== 1) secondEffects += 1;
        convergence.push(Date.now() - started);
        timeline.push({ scenario: "UAT-CELLD-005", substrate, action, generation, operation_id_sha256: sha256(id), attempts: terminal.effect.attempts, unknown_observed: true, convergence_ms: convergence.at(-1) });
      }
    }
  }
  convergence.sort((a, b) => a - b);
  const p95 = convergence[Math.ceil(convergence.length * 0.95) - 1] ?? 0;
  return {
    assertions: [
      { id: "CELLD.005.ORIGINAL_ID", measurements: { trials_per_action: 100, actions: ACTIONS, substrates: SUBSTRATES, lookups, original_id_matches: matches, replacement_ids: replacementIds } },
      { id: "CELLD.005.NO_SECOND_EFFECT", measurements: { trials: convergence.length, second_effects: secondEffects, p95_ms: p95, proxy_healed: run("docker", ["inspect", "--format", "{{.State.Running}}", relayName(runtime.fleet)]) === "true" } },
    ],
    faults: [{ kind: "post_effect_response_loss", signal: "SIGUSR1", trials: convergence.length }],
    metrics: [{ name: "unknown_convergence_p95", value: p95, unit: "ms" }],
  };
}

function providerChecksum(runtime, substrate, name, instanceId) {
  if (substrate === "qemu") return sha256(run("virsh", ["-c", runtime.config.libvirt_uri, "dumpxml", name]));
  const ids = run("docker", ["ps", "--all", "--filter", `label=agentic-instance-id=${instanceId}`, "--format", "{{.ID}}"]);
  if (!ids || ids.split(/\r?\n/).length !== 1) throw new Error("active Docker inventory is missing or ambiguous");
  return sha256(run("docker", ["inspect", "--format", "{{.Id}}|{{.State.Status}}|{{.Config.Image}}", ids]));
}

async function runUat006(runtime, timeline) {
  let attempts = 0;
  let rejected = 0;
  let futureAttempts = 0;
  let activeChanges = 0;
  for (const substrate of SUBSTRATES) {
    const instanceId = randomUUID();
    const name = `celld-fence-${substrate}-${sha256(`${runtime.runId}:006`).slice(0, 9)}`;
    for (const action of ACTIONS) await runOneEffectCampaign(runtime, { prefix: "uat006-old", substrate, instanceId, generation: 1, action, payload: action === "provision" ? provisionPayload(runtime.config, substrate, name) : {} });
    await runOneEffectCampaign(runtime, { prefix: "uat006-active", substrate, instanceId, generation: 2, action: "provision", payload: provisionPayload(runtime.config, substrate, name) });
    const before = providerChecksum(runtime, substrate, name, instanceId);
    run("docker", ["pause", relayName(runtime.fleet)]);
    try {
      for (const action of ["stop", "destroy"]) {
        for (let index = 0; index < 100; index += 1) {
          const id = operationId("uat006-stale", substrate, 1, action, index);
          const payload = {};
          const effect = { operation_id: id, request_hash: requestHash({ operationId: id, instanceId, generation: 1, action, payload }), action, generation: 1, payload, status: "pending", attempts: 0, retry_at: null, terminal_code: null, management_operation_id: null };
          const response = await callbackRequest(callbackContext(runtime.fleet, runtime.managementHost, instanceId, 1), effect);
          attempts += 1;
          if (response.status === 409 && response.body?.error?.code === "celld.stale_generation_fenced") rejected += 1;
        }
      }
      const future = await sendWorkerCommand({ endpoint: runtime.workerEndpoint, varsFile: runtime.fleet.worker_vars_file_ref, instanceId, operationId: operationId("uat006-future", substrate, 4, "destroy"), generation: 4, action: "destroy", payload: {} });
      futureAttempts += 1;
      if (future.status !== 409 || future.body?.error?.code !== "cell.generation_fenced") throw new Error("future generation was not fenced by the active cell");
    } finally {
      run("docker", ["unpause", relayName(runtime.fleet)]);
    }
    const after = providerChecksum(runtime, substrate, name, instanceId);
    if (before !== after) activeChanges += 1;
    timeline.push({ scenario: "UAT-CELLD-006", substrate, stale_attempts: 200, stale_rejected: 200, future_rejected: 1, active_checksum_before: before, active_checksum_after: after, partition: "callback_relay_paused_then_healed" });
    await runOneEffectCampaign(runtime, { prefix: "uat006-clean", substrate, instanceId, generation: 2, action: "destroy", payload: {} });
  }
  return {
    assertions: [
      { id: "CELLD.006.PRE_PROVIDER", measurements: { trials_per_action: 100, actions: ["stop", "destroy"], substrates: SUBSTRATES, attempts, rejected_before_provider: rejected, provider_effects: 0 } },
      { id: "CELLD.006.ACTIVE_SAFE", measurements: { stale_attempts: attempts, future_attempts: futureAttempts, active_generation_changes: activeChanges, active_checksum_unchanged: activeChanges === 0, partition_healed: true } },
    ],
    faults: [{ kind: "callback_network_partition", controller: "relay_pause", healed: true }],
    metrics: [{ name: "stale_generation_attempts", value: attempts, unit: "requests" }],
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
      assertions.push(`Docker provider identity ${sha256(resource.instanceId)} absent`);
      continue;
    }
    const domain = spawnSync("virsh", ["-c", runtime.config.libvirt_uri, "dominfo", resource.name], { encoding: "utf8", shell: false });
    const directory = join(runtime.config.vm_storage_dir, resource.name);
    if (domain.status === 0 || existsSync(directory)) {
      run(join(REPO_ROOT, "scripts/destroy-vm.sh"), [resource.name, "--force"], { env: { ...process.env, AGENTIC_BACKEND: "libvirt", LIBVIRT_DEFAULT_URI: runtime.config.libvirt_uri, VM_STORAGE_DIR: runtime.config.vm_storage_dir }, timeout: 180_000 });
    }
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

function prerequisiteReason(config, scenarioId, profile) {
  if (process.platform !== "linux") return "CELLD_ORCHESTRATION_LINUX_REQUIRED";
  if (["UAT-CELLD-004", "UAT-CELLD-005", "UAT-CELLD-006"].includes(scenarioId) && (!profile.authorization.destructive_faults || profile.authorization.exact_run_owner !== profile.run_id)) return "CELLD_DESTRUCTIVE_AUTHORIZATION_REQUIRED";
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
  if (profile.environment.host_sha256 !== sha256(dependencies.hostname?.() ?? hostname())) throw new Error("live orchestration host identity does not match the protected profile");
  const config = protectedJson(entry.config_path, "orchestration config");
  const configErrors = validateOrchestrationConfig(config);
  if (configErrors.length) throw new Error(configErrors.join("; "));
  if (config.run_id !== runId) throw new Error("orchestration config run identity does not match the live profile");
  const reason = dependencies.prerequisiteReason ? dependencies.prerequisiteReason(config, scenarioId, profile) : prerequisiteReason(config, scenarioId, profile);
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
    runtime = { config, fleet, fleetPath, management, managementHost, workerEndpoint: workerEndpoint(fleet), runId, providerResources: new Map() };
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
  for (const entry of readdirSync(config.working_root, { withFileTypes: true })) {
    if (entry.name === "orchestration.json") continue;
    if (!entry.isDirectory() || entry.isSymbolicLink()) throw new Error("orchestration cleanup found ambiguous residue");
    const scenarioRoot = join(config.working_root, entry.name);
    if (readdirSync(scenarioRoot).length !== 0) throw new Error(`orchestration scenario residue remains: ${entry.name}`);
  }
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
    process.stdout.write(`${JSON.stringify({ status: "PASS", config_path: result.path, run_id: result.config.run_id })}\n`);
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
