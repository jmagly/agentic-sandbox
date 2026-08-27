#!/usr/bin/env node

import { createHash, randomBytes } from "node:crypto";
import {
  chmodSync,
  closeSync,
  constants,
  existsSync,
  fstatSync,
  fsyncSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { hostname } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

import { fixtureEnvironment, validateFixtureConfig } from "./celld-seaweedfs-fixture.mjs";
import { isPrivateIpv4, openStorageGatewayAccess, startLoopbackForwarder } from "./celld-storage-gateway-access.mjs";
import { loadReviewedRolloutCandidates } from "./celld-rollout-candidate.mjs";
import { S3V1Client, STORAGE_PROFILE_SCHEMA } from "./celld-storage-qualifier.mjs";
import { probeWorkerAuthentication } from "./celld-worker-client.mjs";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
export const FLEET_SCHEMA = "agentic-sandbox.celld-fleet-fixture/v1";
export const INVENTORY_SCHEMA = "agentic-sandbox.celld-fleet-inventory/v1";
export const FLEET_OWNER = Object.freeze({
  repository: "roctinam/agentic-sandbox",
  workflow: "celld-qualification",
});
const RUN_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const SAFE_NAME = /^[a-z0-9][a-z0-9_.-]{0,127}$/;
const SHA256 = /^sha256:[0-9a-f]{64}$/;
const HEX_SHA256 = /^[0-9a-f]{64}$/;
const CONTAINER_ID = /^[0-9a-f]{64}$/;
const EXPECTED_IMAGE = Object.freeze({
  product: "Celld",
  version: "0.2.1",
  commit: "ae8fac053d79f971bfcb996054bb43eb2f9b05da",
  image: "ghcr.io/denoland/celld",
  index_digest: "sha256:7a4380721b6400073f2a26afe70a828410169f658d31b5ef61383e648ca0c530",
  manifest_digest: "sha256:8634eac20f69ffe99103d403b985c0afd43fd970badadd01435f297ba0df797a",
});
const IMAGE_FIELDS = Object.freeze(["product", "version", "commit", "image", "index_digest", "manifest_digest"]);
export const FLEET_IMAGE_CHANNELS = Object.freeze(["approved", "reviewed-candidate"]);
const EXPECTED_WORKER_DIGEST = "sha256:9057aa0debb402c9f8177d1df0c8c06de487c266029f0c5282a8e1cd1076322b";
const CALLBACK_CLIENT_CN = "agentic-celld-worker-callback";
const CALLBACK_RELAY_PORT = 8125;
const MANAGEMENT_TLS_PORT = 8122;
const CREDENTIAL_LAUNCHER_CONTAINER_PATH = "/usr/local/bin/agentic-celld-credential-launcher";
const DEFAULT_CREDENTIAL_LAUNCHER_PATH = join(
  REPO_ROOT,
  "tools/celld-callback-relay/target/x86_64-unknown-linux-musl/release/agentic-celld-credential-launcher",
);
const RESOURCE_LABELS = Object.freeze({
  repository: "dev.agentic-sandbox.repository",
  workflow: "dev.agentic-sandbox.workflow",
  run: "dev.agentic-sandbox.run",
  scope: "dev.agentic-sandbox.scope",
});
const FLEET_DIAGNOSIS_SCHEMA = "agentic-sandbox.celld-fleet-diagnosis/v1";
const STARTUP_NOT_READY = "CELLD_FLEET_STARTUP_NOT_READY";
const DEFAULT_STARTUP_READINESS = Object.freeze({
  maxAttempts: 5,
  deadlineMs: 5_000,
  backoffMs: 250,
});
const PINNED_CONDITIONAL_WRITE_LINE = "ok bucket conditional write (create, reject-create, update, reject-stale)";
const PINNED_SIGNED_DIRECT_PROTOCOL = "2";
const SIGNED_DIRECT_PEER_PREFIX = /^ok peer ([A-Za-z0-9][A-Za-z0-9._-]{0,127}) (?:at ([A-Za-z0-9][A-Za-z0-9._-]{0,127}:\d{1,5})|([A-Za-z0-9][A-Za-z0-9._-]{0,127}:\d{1,5})) \(signed direct probe\)(?:\s+(.+))?$/;
const SIGNED_DIRECT_FIELD = /^([a-z][a-z0-9_]{0,63})=([A-Za-z0-9._:+,/-]{1,256})$/;
const SIGNED_DIRECT_FIELD_VALIDATORS = Object.freeze({
  protocol: new RegExp(`^${PINNED_SIGNED_DIRECT_PROTOCOL}$`),
  resident_cells: /^(?:\d+|unknown)$/,
  websockets: /^(?:\d+|unknown)$/,
  rss_bytes: /^(?:\d+|unknown)$/,
  in_use_bytes: /^(?:\d+|unknown)$/,
  cpu_percent: /^(?:\d+(?:\.\d+)?|unknown)$/,
  fds: /^(?:\d+|unknown)\/(?:\d+|unknown)$/,
  pressured: /^(?:true|false|unknown)$/,
  shed_cells: /^(?:\d+|unknown)$/,
  restoring: /^(?:\d+|unknown)$/,
  load_age_ms: /^(?:\d+|unknown)$/,
});
const MAX_DIAGNOSIS_STDOUT_BYTES = 64 * 1024;

export class CleanupResidueError extends Error {
  constructor(message) {
    super(message);
    this.name = "CleanupResidueError";
    this.exitCode = 4;
  }
}

export class FleetStartupReadinessError extends Error {
  constructor(evidence) {
    super("Celld fleet startup did not reach bounded exact readiness");
    this.name = "FleetStartupReadinessError";
    this.exitCode = 3;
    this.evidence = evidence;
  }
}

class FleetControllerSubprocessError extends Error {
  constructor(program, args, result) {
    super(`${basename(program)} controller subprocess failed`);
    this.name = "FleetControllerSubprocessError";
    this.program = basename(program);
    this.operation = classifyControllerOperation(this.program, args);
    this.exitStatus = Number.isInteger(result.status) ? result.status : null;
    this.signal = typeof result.signal === "string" ? result.signal : null;
    this.errorCode = typeof result.error?.code === "string" ? result.error.code : null;
    this.timedOut = this.errorCode === "ETIMEDOUT";
    const stdout = String(result.stdout ?? "");
    this.stdoutSha256 = sha256(stdout);
    this.stderrSha256 = sha256(String(result.stderr ?? result.error?.message ?? ""));
    Object.defineProperty(this, "controllerStdout", {
      value: this.operation === "docker_exec_diagnose" && Buffer.byteLength(stdout, "utf8") <= MAX_DIAGNOSIS_STDOUT_BYTES
        ? stdout
        : null,
      enumerable: false,
      writable: false,
    });
  }
}

function classifyControllerOperation(program, args) {
  if (program !== "docker" || !Array.isArray(args) || typeof args[0] !== "string") return "other";
  if (args[0] === "exec" && args[2] === CREDENTIAL_LAUNCHER_CONTAINER_PATH && args[3] === "diagnose") return "docker_exec_diagnose";
  if (args[0] === "network" && args[1] === "inspect") return "docker_network_inspect";
  if (args[0] === "inspect") return "docker_inspect";
  if (args[0] === "run" && args.includes("deploy")) return "docker_run_celld_deploy";
  if (args[0] === "run") return "docker_run";
  if (args[0] === "rm") return "docker_remove";
  if (args[0] === "pull") return "docker_pull";
  if (args[0] === "image" && args[1] === "inspect") return "docker_image_inspect";
  if (args[0] === "create") return "docker_create";
  if (args[0] === "start") return "docker_start";
  if (args[0] === "stop") return "docker_stop";
  if (args[0] === "port") return "docker_port";
  return "docker_other";
}

class FleetDiagnosisDeadlineError extends Error {
  constructor(evidenceSha256) {
    super("fleet diagnosis total deadline exceeded");
    this.name = "FleetDiagnosisDeadlineError";
    this.evidenceSha256 = evidenceSha256;
  }
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function diagnosisEvidenceSha256(kind, value, expectedNodeIdsSha256) {
  return sha256(`agentic-sandbox.celld-fleet-diagnosis-attempt/v1\0${kind}\0${expectedNodeIdsSha256}\0${String(value)}`);
}

function synchronousWait(milliseconds) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, milliseconds);
}

function startupReadinessPolicy(value = {}) {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("fleet startup readiness policy must be an object");
  const allowed = new Set(["maxAttempts", "deadlineMs", "backoffMs", "clock", "wait"]);
  if (Object.keys(value).some((key) => !allowed.has(key))) throw new Error("fleet startup readiness policy contains an unsupported field");
  const policy = {
    maxAttempts: value.maxAttempts ?? DEFAULT_STARTUP_READINESS.maxAttempts,
    deadlineMs: value.deadlineMs ?? DEFAULT_STARTUP_READINESS.deadlineMs,
    backoffMs: value.backoffMs ?? DEFAULT_STARTUP_READINESS.backoffMs,
    clock: value.clock ?? Date.now,
    wait: value.wait ?? synchronousWait,
  };
  if (!Number.isSafeInteger(policy.maxAttempts) || policy.maxAttempts < 1 || policy.maxAttempts > 20
      || !Number.isSafeInteger(policy.deadlineMs) || policy.deadlineMs < 1 || policy.deadlineMs > 300_000
      || !Number.isSafeInteger(policy.backoffMs) || policy.backoffMs < 0 || policy.backoffMs > 60_000
      || typeof policy.clock !== "function" || typeof policy.wait !== "function") {
    throw new Error("fleet startup readiness policy is outside its fixed bounds");
  }
  const startedAtMs = policy.clock();
  if (!Number.isFinite(startedAtMs)) throw new Error("fleet startup readiness clock is invalid");
  return { ...policy, startedAtMs, deadlineAtMs: startedAtMs + policy.deadlineMs };
}

function lateStartupDiagnosisEvidence(result, policy) {
  const evidenceSha256 = result?.membership?.probe_sha256
    ?? result?.failure?.evidence_sha256
    ?? diagnosisEvidenceSha256("deadline", "expired", sha256((result?.nodes ?? []).map((node) => node.node_id).join("\n")));
  return {
    ...result,
    status: "NOT_READY",
    reason_code: STARTUP_NOT_READY,
    retryable: false,
    membership: { ...result.membership, probe: "failed" },
    failure: {
      attempts: result.membership.attempts,
      max_attempts: policy.maxAttempts,
      deadline_ms: policy.deadlineMs,
      backoff_ms: policy.backoffMs,
      expected_node_ids_sha256: sha256(result.nodes.map((node) => node.node_id).join("\n")),
      reason_code: "CELLD_DIAGNOSIS_DEADLINE_EXCEEDED",
      evidence_sha256: evidenceSha256,
    },
  };
}

function privateWrite(path, value, { exclusive = true } = {}) {
  writeFileSync(path, value, { mode: 0o600, flag: exclusive ? "wx" : "w" });
  chmodSync(path, 0o600);
}

function atomicJson(path, value, fsOperations = {}) {
  const filesystem = {
    writeFileSync: fsOperations.writeFileSync ?? writeFileSync,
    openSync: fsOperations.openSync ?? openSync,
    fsyncSync: fsOperations.fsyncSync ?? fsyncSync,
    closeSync: fsOperations.closeSync ?? closeSync,
    renameSync: fsOperations.renameSync ?? renameSync,
    constants: fsOperations.constants ?? constants,
  };
  const temporary = `${path}.new-${process.pid}-${randomBytes(8).toString("hex")}`;
  let fileDescriptor = null;
  let directoryDescriptor = null;
  let renamed = false;
  try {
    fileDescriptor = filesystem.openSync(
      temporary,
      filesystem.constants.O_WRONLY
        | filesystem.constants.O_CREAT
        | filesystem.constants.O_EXCL
        | (filesystem.constants.O_NOFOLLOW ?? 0),
      0o600,
    );
    filesystem.writeFileSync(fileDescriptor, `${JSON.stringify(value, null, 2)}\n`, "utf8");
    filesystem.fsyncSync(fileDescriptor);
    filesystem.closeSync(fileDescriptor);
    fileDescriptor = null;
    filesystem.renameSync(temporary, path);
    renamed = true;
    directoryDescriptor = filesystem.openSync(
      dirname(path),
      filesystem.constants.O_RDONLY
        | (filesystem.constants.O_DIRECTORY ?? 0)
        | (filesystem.constants.O_NOFOLLOW ?? 0),
    );
    filesystem.fsyncSync(directoryDescriptor);
  } finally {
    if (fileDescriptor !== null) filesystem.closeSync(fileDescriptor);
    if (directoryDescriptor !== null) filesystem.closeSync(directoryDescriptor);
    if (!renamed && existsSync(temporary)) rmSync(temporary, { force: false });
  }
}

function sameRunInventoryTemporary(candidate, inventory, config) {
  if (validateFleetInventory(candidate, config).length !== 0) return false;
  if (JSON.stringify(candidate) === JSON.stringify(inventory)) return true;
  if (candidate.actions.length !== inventory.actions.length + 1) return false;
  const pending = candidate.actions.at(-1);
  if (pending?.kind !== "celld_diagnose" || pending.status !== "planned") return false;
  const rebased = { ...candidate, updated_at: inventory.updated_at, actions: inventory.actions };
  return JSON.stringify(rebased) === JSON.stringify(inventory);
}

function openInventoryTemporary(path, inventory, config) {
  let descriptor = null;
  try {
    descriptor = openSync(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
    const metadata = fstatSync(descriptor);
    const pathnameMetadata = lstatSync(path);
    const uid = typeof process.getuid === "function" ? process.getuid() : metadata.uid;
    const gid = typeof process.getgid === "function" ? process.getgid() : metadata.gid;
    if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.nlink !== 1
        || metadata.uid !== uid || metadata.gid !== gid || (metadata.mode & 0o777) !== 0o600
        || metadata.size < 2 || metadata.size > 16 * 1024 * 1024
        || pathnameMetadata.dev !== metadata.dev || pathnameMetadata.ino !== metadata.ino) {
      closeSync(descriptor);
      return null;
    }
    const candidate = JSON.parse(readFileSync(descriptor, "utf8"));
    if (!sameRunInventoryTemporary(candidate, inventory, config)) {
      closeSync(descriptor);
      return null;
    }
    return { descriptor, metadata };
  } catch {
    if (descriptor !== null) closeSync(descriptor);
    return null;
  }
}

function fsyncParentDirectory(path) {
  const descriptor = openSync(
    dirname(path),
    constants.O_RDONLY | (constants.O_DIRECTORY ?? 0) | (constants.O_NOFOLLOW ?? 0),
  );
  try { fsyncSync(descriptor); } finally { closeSync(descriptor); }
}

function reclaimInventoryTemporary(config, inventory) {
  const authoritative = inventoryPath(config);
  const parent = dirname(authoritative);
  const prefix = `${basename(authoritative)}.new-`;
  const names = readdirSync(parent).filter((name) => name.startsWith(prefix));
  if (names.length === 0) return [];
  const residue = names.map((name) => join(parent, name));
  if (names.length !== 1) return residue;
  const match = /^fleet-inventory\.json\.new-([1-9][0-9]*)-([0-9a-f]{16})$/.exec(names[0]);
  if (!match || existsSync(`/proc/${match[1]}`)) return residue;
  const path = residue[0];
  const opened = openInventoryTemporary(path, inventory, config);
  if (!opened) return residue;
  let quarantine = `${path}.reclaim-${randomBytes(8).toString("hex")}`;
  while (existsSync(quarantine)) quarantine = `${path}.reclaim-${randomBytes(8).toString("hex")}`;
  try {
    renameSync(path, quarantine);
    const moved = lstatSync(quarantine);
    const anchored = fstatSync(opened.descriptor);
    if (moved.dev !== anchored.dev || moved.ino !== anchored.ino) {
      if (!existsSync(path)) renameSync(quarantine, path);
      return residue;
    }
    const finalMetadata = lstatSync(quarantine);
    if (finalMetadata.dev !== anchored.dev || finalMetadata.ino !== anchored.ino) {
      if (!existsSync(path)) renameSync(quarantine, path);
      return residue;
    }
    rmSync(quarantine, { force: false });
    fsyncParentDirectory(authoritative);
    return [];
  } catch {
    if (existsSync(quarantine) && !existsSync(path)) {
      try { renameSync(quarantine, path); } catch { /* preserve both paths as reported residue */ }
    }
    return residue;
  } finally {
    closeSync(opened.descriptor);
  }
}

function protectedJson(path, description) {
  if (!isAbsolute(path) || !existsSync(path)) throw new Error(`${description} is missing`);
  const metadata = lstatSync(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) {
    throw new Error(`${description} must be a protected regular non-symlink file`);
  }
  return JSON.parse(readFileSync(path, "utf8"));
}

function imageCore(value) {
  return Object.fromEntries(IMAGE_FIELDS.map((key) => [key, value?.[key]]));
}

function exactImagePin(actual, expected) {
  const expectedPin = { ...expected, image_ref: `${expected.image}@${expected.manifest_digest}` };
  return actual && typeof actual === "object" && !Array.isArray(actual)
    && Object.keys(actual).length === Object.keys(expectedPin).length
    && Object.entries(expectedPin).every(([key, value]) => actual[key] === value);
}

export function loadFleetImage(channel = "approved") {
  if (!FLEET_IMAGE_CHANNELS.includes(channel)) throw new Error("Celld fleet image channel is invalid");
  if (channel === "reviewed-candidate") {
    const candidates = loadReviewedRolloutCandidates();
    if (candidates.length !== 1 || candidates[0].qualification_status !== "reviewed_unqualified") {
      throw new Error("reviewed Celld candidate inventory is invalid");
    }
    return imageCore(candidates[0]);
  }
  const path = join(REPO_ROOT, "deploy/celld/qualification/celld-images.json");
  const value = JSON.parse(readFileSync(path, "utf8"));
  if (value.schema_version !== "agentic-sandbox.celld-images/v1" || value.platform !== "linux/amd64") {
    throw new Error("Celld image inventory is invalid");
  }
  for (const [key, expected] of Object.entries(EXPECTED_IMAGE)) {
    if (value.celld?.[key] !== expected) throw new Error(`reviewed Celld ${key} pin changed`);
  }
  if (value.celld.image !== "ghcr.io/denoland/celld" || !SHA256.test(value.celld.manifest_digest)) {
    throw new Error("Celld image reference is invalid");
  }
  return imageCore(value.celld);
}

function loadWorkerDigest() {
  const value = JSON.parse(readFileSync(join(REPO_ROOT, "runtimes/celld/instance-cell/bundle.json"), "utf8"));
  if (value.digest !== EXPECTED_WORKER_DIGEST) throw new Error("reviewed reference Worker digest changed");
  return value.digest;
}

function exactLabels(runId) {
  return {
    [RESOURCE_LABELS.repository]: FLEET_OWNER.repository,
    [RESOURCE_LABELS.workflow]: FLEET_OWNER.workflow,
    [RESOURCE_LABELS.run]: runId,
    [RESOURCE_LABELS.scope]: "celld-qualification",
  };
}

function labelsToArgs(labels) {
  return Object.entries(labels).flatMap(([key, value]) => ["--label", `${key}=${value}`]);
}

function credentialLauncher(path) {
  const resolved = resolve(path ?? "");
  const metadata = lstatSync(resolved);
  if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o111) === 0) {
    throw new Error("the fixed credential launcher executable is missing or unsafe");
  }
  return { path: resolved, sha256: sha256(readFileSync(resolved)) };
}

function credentialLauncherArgs(launcher) {
  return [
    "--mount", `type=bind,src=${launcher.path},dst=${CREDENTIAL_LAUNCHER_CONTAINER_PATH},readonly`,
    "--entrypoint", CREDENTIAL_LAUNCHER_CONTAINER_PATH,
  ];
}

function qualificationProjectRoot(config, projectPath, deploymentKind) {
  const project = resolve(projectPath ?? "");
  const reference = join(REPO_ROOT, "runtimes/celld/instance-cell");
  if (deploymentKind === "approved-reference") {
    if (project !== reference) throw new Error("approved Worker deployment must use the exact reviewed project root");
  } else if (deploymentKind === "qualification-candidate") {
    const relativeProject = relative(config.run_root, project);
    if (!relativeProject || relativeProject.startsWith(`..${sep}`) || relativeProject === ".." || isAbsolute(relativeProject)) {
      throw new Error("qualification Worker project must remain inside the exact run root");
    }
  } else {
    throw new Error("qualification Worker deployment kind is invalid");
  }
  const metadata = lstatSync(project);
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) throw new Error("qualification Worker project root is unsafe");
  return project;
}

function projectFiles(root, deploymentKind) {
  if (deploymentKind === "approved-reference") return ["worker.mjs", "wrangler.json"];
  const files = [];
  const visit = (directory, prefix = "") => {
    for (const entry of readdirSync(directory, { withFileTypes: true }).sort((left, right) => left.name.localeCompare(right.name))) {
      const relativePath = prefix ? `${prefix}/${entry.name}` : entry.name;
      const path = join(directory, entry.name);
      if (entry.isSymbolicLink()) throw new Error("qualification Worker project cannot contain symlinks");
      if (entry.isDirectory()) visit(path, relativePath);
      else if (entry.isFile()) files.push(relativePath);
      else throw new Error("qualification Worker project contains an unsupported filesystem entry");
      if (files.length > 128) throw new Error("qualification Worker project exceeds the evidence file bound");
    }
  };
  visit(root);
  return files;
}

export function workerDeploymentProjectDigest(projectPath, { deploymentKind = "qualification-candidate" } = {}) {
  const root = resolve(projectPath ?? "");
  const hash = createHash("sha256");
  let totalBytes = 0;
  const files = projectFiles(root, deploymentKind);
  if (!files.includes("worker.mjs") || !files.includes("wrangler.json")) throw new Error("qualification Worker project is incomplete");
  for (const relativePath of files) {
    const path = join(root, relativePath);
    const metadata = lstatSync(path);
    if (!metadata.isFile() || metadata.isSymbolicLink()) throw new Error("qualification Worker source file is unsafe");
    totalBytes += metadata.size;
    if (totalBytes > 16 * 1024 * 1024) throw new Error("qualification Worker project exceeds the evidence byte bound");
    const bytes = readFileSync(path);
    hash.update(`${relativePath}\0${bytes.length}\0`);
    hash.update(bytes);
  }
  return `sha256:${hash.digest("hex")}`;
}

function assertProtectedCredentialFile(path) {
  const metadata = lstatSync(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0 || metadata.size === 0 || metadata.size > 16 * 1024) {
    throw new Error("bucket credential must be a protected bounded regular file");
  }
}

function defaultRunner(program, args, options = {}) {
  const result = spawnSync(program, args, { encoding: "utf8", shell: false, ...options });
  if (result.error || result.status !== 0) {
    throw new FleetControllerSubprocessError(program, args, result);
  }
  return result.stdout.trim();
}

function checkedRoot(path, runId) {
  const root = resolve(path ?? "");
  if (!isAbsolute(path ?? "") || !root.split("/").includes(runId)) {
    throw new Error("fleet run root must be absolute and contain the exact run ID");
  }
  const metadata = lstatSync(root);
  if (!metadata.isDirectory() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) {
    throw new Error("fleet run root must be a protected regular directory");
  }
  return root;
}

export function validateFleetConfig(config) {
  const errors = [];
  if (!config || typeof config !== "object" || Array.isArray(config)) return ["config must be an object"];
  const allowed = new Set(["schema_version", "run_id", "run_root", "scope", "host_sha256", "owner", "storage_config_path", "worker_vars_file_ref", "callback", "network", "pins", "nodes", "resources", "instrumentation", "operator_commands"]);
  for (const key of Object.keys(config)) if (!allowed.has(key)) errors.push(`config.${key} is not allowed`);
  if (config.schema_version !== FLEET_SCHEMA) errors.push(`config.schema_version must be ${FLEET_SCHEMA}`);
  if (!RUN_ID.test(config.run_id ?? "")) errors.push("config.run_id is invalid");
  if (!isAbsolute(config.run_root ?? "") || !config.run_root?.split("/").includes(config.run_id)) errors.push("config.run_root must contain run_id");
  if (config.scope !== "single-host multi-node") errors.push("config.scope must be single-host multi-node");
  if (!/^[0-9a-f]{64}$/.test(config.host_sha256 ?? "")) errors.push("config.host_sha256 is invalid");
  if (config.owner?.repository !== FLEET_OWNER.repository || config.owner?.workflow !== FLEET_OWNER.workflow || config.owner?.run_id !== config.run_id) errors.push("config.owner is invalid");
  if (config.storage_config_path !== join(config.run_root ?? "", "fixture.json")) errors.push("config.storage_config_path must be the exact run storage config");
  if (config.worker_vars_file_ref !== join(config.run_root ?? "", "fleet/worker-vars")) errors.push("config.worker_vars_file_ref must be the fixed protected file");
  const callback = config.callback;
  const callbackRoot = join(config.run_root ?? "", "fleet");
  const tlsRoot = join(config.run_root ?? "", "tls");
  const managementTlsRoot = join(config.run_root ?? "", "management-tls");
  if (callback?.worker_url !== `http://127.0.0.1:${CALLBACK_RELAY_PORT}/` || callback?.relay_listener !== `127.0.0.1:${CALLBACK_RELAY_PORT}` || callback?.management_server_name !== "management.internal" || callback?.management_tls_port !== MANAGEMENT_TLS_PORT || callback?.client_cn !== CALLBACK_CLIENT_CN) errors.push("config.callback transport identity is invalid");
  const callbackPaths = {
    ca_file_ref: join(tlsRoot, "ca.crt"),
    management_server_cert_file_ref: join(managementTlsRoot, "management-server.crt"),
    management_server_key_file_ref: join(managementTlsRoot, "management-server.key"),
    relay_client_cert_file_ref: join(managementTlsRoot, "callback-client.crt"),
    relay_client_key_file_ref: join(managementTlsRoot, "callback-client.key"),
    management_auth_key_file_ref: join(callbackRoot, "management-auth-key"),
    effect_ledger_file_ref: join(callbackRoot, "effect-ledger.sqlite"),
  };
  for (const [key, expected] of Object.entries(callbackPaths)) if (callback?.[key] !== expected) errors.push(`config.callback.${key} must be the fixed run path`);
  if (!SAFE_NAME.test(config.network?.name ?? "") || config.network?.scope !== "storage-private" || config.network?.internal_listener !== "0.0.0.0:8081" || config.network?.public_listener !== "0.0.0.0:8080" || config.network?.public_publish !== "127.0.0.1::8080") errors.push("config.network is invalid");
  if (!FLEET_IMAGE_CHANNELS.includes(config.pins?.celld_channel)) {
    errors.push("config.pins.celld_channel is invalid");
  } else {
    try {
      if (!exactImagePin(config.pins?.celld, loadFleetImage(config.pins.celld_channel))) {
        errors.push("config.pins.celld does not match its exact image channel");
      }
    } catch (error) {
      errors.push(`config.pins.celld cannot be validated: ${error.message}`);
    }
  }
  if (config.pins?.worker_digest !== EXPECTED_WORKER_DIGEST) errors.push("config.pins.worker_digest is invalid");
  if (!Array.isArray(config.nodes) || config.nodes.length !== 3) errors.push("config.nodes must contain exactly three nodes");
  const roles = (config.nodes ?? []).map((node) => node.role);
  if (roles.filter((role) => role === "reserve").length !== 1 || roles.filter((role) => role === "active").length !== 2) errors.push("config.nodes must declare two active nodes and one reserve");
  const names = new Set();
  for (const [index, node] of (config.nodes ?? []).entries()) {
    if (!SAFE_NAME.test(node?.name ?? "") || names.has(node?.name)) errors.push(`config.nodes[${index}].name is invalid or duplicated`);
    names.add(node?.name);
    if (node?.node_id !== `${config.run_id}-node-${index + 1}` || node?.advertise !== `${node?.name}:8081` || node?.state_dir !== join(config.run_root ?? "", `fleet/node-${index + 1}`)) errors.push(`config.nodes[${index}] identity is invalid`);
  }
  if (config.resources?.cpu_per_node !== 1 || config.resources?.memory_per_node_mb !== 2048 || config.resources?.pids_per_node !== 256 || config.resources?.max_resident_cells !== 1000 || config.resources?.max_rss_mb !== 1536) errors.push("config.resources is invalid");
  if (config.instrumentation?.management !== "required" || config.instrumentation?.qemu !== "required" || config.instrumentation?.docker !== "required") errors.push("config.instrumentation boundaries are incomplete");
  if (JSON.stringify(config.operator_commands) !== JSON.stringify(["prepare", "deploy", "start", "start-relays", "diagnose", "probe-worker", "cleanup", "janitor-preview", "janitor-reap"])) errors.push("config.operator_commands is invalid");
  return errors;
}

export function validateFleetInventory(inventory, config) {
  const errors = [];
  if (inventory?.schema_version !== INVENTORY_SCHEMA) errors.push(`inventory.schema_version must be ${INVENTORY_SCHEMA}`);
  if (inventory?.run_id !== config.run_id || inventory?.owner?.repository !== FLEET_OWNER.repository || inventory?.owner?.workflow !== FLEET_OWNER.workflow || inventory?.owner?.run_id !== config.run_id) errors.push("inventory owner does not match config");
  if (!Array.isArray(inventory?.resources) || !Array.isArray(inventory?.actions)) errors.push("inventory resources/actions must be arrays");
  if (inventory?.credential_launcher_sha256 !== undefined && !HEX_SHA256.test(inventory.credential_launcher_sha256)) errors.push("inventory credential launcher digest is invalid");
  if (inventory?.deployment_version_id !== undefined && !/^[0-9a-f]{16}$/.test(inventory.deployment_version_id)) errors.push("inventory deployment version ID is invalid");
  if (inventory?.active_worker_deployment !== undefined) {
    const deployment = inventory.active_worker_deployment;
    const allowed = new Set(["deployment_kind", "project_sha256", "worker_digest", "version_id", "output_sha256"]);
    if (!deployment || typeof deployment !== "object" || Array.isArray(deployment)
        || Object.keys(deployment).some((key) => !allowed.has(key))
        || !["approved-reference", "qualification-candidate"].includes(deployment.deployment_kind)
        || !SHA256.test(deployment.project_sha256 ?? "")
        || !SHA256.test(deployment.worker_digest ?? "")
        || !/^[0-9a-f]{16}$/.test(deployment.version_id ?? "")
        || !HEX_SHA256.test(deployment.output_sha256 ?? "")) errors.push("inventory active Worker deployment is invalid");
  }
  const keys = new Set();
  for (const resource of inventory?.resources ?? []) {
    const key = `${resource.type}:${resource.id}`;
    if (keys.has(key)) errors.push(`inventory resource is duplicated: ${key}`);
    keys.add(key);
    if (!new Set(["directory", "protected_file", "docker_container"]).has(resource.type) || !SAFE_NAME.test(resource.id.replaceAll("/", "-").replace(/^-+/, "")) || !["planned", "created", "started", "removed"].includes(resource.status)) errors.push(`inventory resource is invalid: ${key}`);
  }
  const expectedNodeIdsSha256 = sha256((config.nodes ?? []).map((node) => node.node_id).join("\n"));
  for (const [index, action] of (inventory?.actions ?? []).entries()) {
    if (action?.kind !== "celld_diagnose") continue;
    const context = `inventory.actions[${index}]`;
    const allowed = new Set([
      "kind", "target", "attempt", "max_attempts", "deadline_ms", "backoff_ms", "expected_node_ids_sha256",
      "planned_at", "status", "evidence_sha256", "completed_at", "failed_at", "reason_code",
    ]);
    if (!action || typeof action !== "object" || Array.isArray(action)
        || Object.keys(action).some((key) => !allowed.has(key))
        || JSON.stringify(action).length > 2_048
        || action.target !== config.nodes?.[0]?.name
        || !Number.isSafeInteger(action.attempt) || action.attempt < 1
        || !Number.isSafeInteger(action.max_attempts) || action.max_attempts < action.attempt || action.max_attempts > 20
        || !Number.isSafeInteger(action.deadline_ms) || action.deadline_ms < 1 || action.deadline_ms > 300_000
        || !Number.isSafeInteger(action.backoff_ms) || action.backoff_ms < 0 || action.backoff_ms > 60_000
        || action.expected_node_ids_sha256 !== expectedNodeIdsSha256
        || !Number.isFinite(Date.parse(action.planned_at))
        || !["planned", "completed", "failed"].includes(action.status)) {
      errors.push(`${context} is invalid`);
      continue;
    }
    if (action.status === "planned") {
      if (["evidence_sha256", "completed_at", "failed_at", "reason_code"].some((key) => action[key] !== undefined)) errors.push(`${context} planned outcome is invalid`);
    } else if (!HEX_SHA256.test(action.evidence_sha256 ?? "")) {
      errors.push(`${context} terminal evidence digest is invalid`);
    } else if (action.status === "completed") {
      if (!Number.isFinite(Date.parse(action.completed_at)) || action.failed_at !== undefined || action.reason_code !== undefined) errors.push(`${context} completed outcome is invalid`);
    } else if (!Number.isFinite(Date.parse(action.failed_at))
        || !/^CELLD_DIAGNOSIS_[A-Z0-9_]+$/.test(action.reason_code ?? "")
        || action.completed_at !== undefined) {
      errors.push(`${context} failed outcome is invalid`);
    }
  }
  return errors;
}

export function prepareFleet({ storageConfigPath, outputPath, now = new Date(), celldChannel = "approved", afterCreate = () => {} }) {
  if (typeof afterCreate !== "function") throw new Error("fleet creation checkpoint must be a function");
  const storage = protectedJson(resolve(storageConfigPath), "storage fixture config");
  const storageErrors = validateFixtureConfig(storage);
  if (storageErrors.length) throw new Error(storageErrors.join("; "));
  if (storage.fixture_profile !== "titan-single-host-storage" || !storage.promoting) throw new Error("fleet requires the exact promoting Titan storage profile");
  const runRoot = checkedRoot(storage.run_root, storage.run_id);
  const expectedOutput = join(runRoot, "fleet.json");
  if (resolve(outputPath ?? expectedOutput) !== expectedOutput) throw new Error("fleet config output must remain at the fixed run-root path");
  if (existsSync(expectedOutput) || existsSync(join(runRoot, "fleet-inventory.json"))) throw new Error("fleet fixture already exists for this run");
  const image = loadFleetImage(celldChannel);
  const nodePrefix = `${storage.project}-celld`;
  const config = {
    schema_version: FLEET_SCHEMA,
    run_id: storage.run_id,
    run_root: runRoot,
    scope: "single-host multi-node",
    host_sha256: sha256(hostname()),
    owner: { ...FLEET_OWNER, run_id: storage.run_id },
    storage_config_path: resolve(storageConfigPath),
    worker_vars_file_ref: join(runRoot, "fleet/worker-vars"),
    callback: {
      worker_url: `http://127.0.0.1:${CALLBACK_RELAY_PORT}/`,
      relay_listener: `127.0.0.1:${CALLBACK_RELAY_PORT}`,
      management_server_name: "management.internal",
      management_tls_port: MANAGEMENT_TLS_PORT,
      client_cn: CALLBACK_CLIENT_CN,
      ca_file_ref: join(runRoot, "tls/ca.crt"),
      management_server_cert_file_ref: join(runRoot, "management-tls/management-server.crt"),
      management_server_key_file_ref: join(runRoot, "management-tls/management-server.key"),
      relay_client_cert_file_ref: join(runRoot, "management-tls/callback-client.crt"),
      relay_client_key_file_ref: join(runRoot, "management-tls/callback-client.key"),
      management_auth_key_file_ref: join(runRoot, "fleet/management-auth-key"),
      effect_ledger_file_ref: join(runRoot, "fleet/effect-ledger.sqlite"),
    },
    network: {
      name: `${storage.project}_storage-private`,
      scope: "storage-private",
      internal_listener: "0.0.0.0:8081",
      public_listener: "0.0.0.0:8080",
      public_publish: "127.0.0.1::8080",
    },
    pins: {
      celld_channel: celldChannel,
      celld: { ...image, image_ref: `${image.image}@${image.manifest_digest}` },
      worker_digest: loadWorkerDigest(),
    },
    nodes: Array.from({ length: 3 }, (_value, index) => ({
      name: `${nodePrefix}-node-${index + 1}`,
      node_id: `${storage.run_id}-node-${index + 1}`,
      role: index === 2 ? "reserve" : "active",
      advertise: `${nodePrefix}-node-${index + 1}:8081`,
      state_dir: join(runRoot, `fleet/node-${index + 1}`),
    })),
    resources: { cpu_per_node: 1, memory_per_node_mb: 2048, pids_per_node: 256, max_resident_cells: 1000, max_rss_mb: 1536 },
    instrumentation: { management: "required", qemu: "required", docker: "required" },
    operator_commands: ["prepare", "deploy", "start", "start-relays", "diagnose", "probe-worker", "cleanup", "janitor-preview", "janitor-reap"],
  };
  const errors = validateFleetConfig(config);
  if (errors.length) throw new Error(errors.join("; "));
  const inventory = {
    schema_version: INVENTORY_SCHEMA,
    run_id: config.run_id,
    scope: config.scope,
    owner: { ...config.owner },
    created_at: now.toISOString(),
    updated_at: now.toISOString(),
    state: "prepared",
    resources: [],
    actions: [],
  };
  privateWrite(expectedOutput, `${JSON.stringify(config, null, 2)}\n`);
  privateWrite(join(runRoot, "fleet-inventory.json"), `${JSON.stringify(inventory, null, 2)}\n`);
  const fleetRoot = join(runRoot, "fleet");
  planResource(config, inventory, { type: "directory", id: fleetRoot, status: "planned" }, now);
  mkdirSync(fleetRoot, { mode: 0o700 });
  chmodSync(fleetRoot, 0o700);
  markResource(config, inventory, "directory", fleetRoot, "created", now);
  afterCreate({ type: "directory", id: fleetRoot });
  planResource(config, inventory, { type: "protected_file", id: config.worker_vars_file_ref, status: "planned" }, now);
  const authKeyId = `run-${sha256(config.run_id).slice(0, 20)}`;
  const authKey = randomBytes(32).toString("base64url");
  privateWrite(config.worker_vars_file_ref, `CELL_AUTH_KEY_ID=${authKeyId}\nCELL_AUTH_KEY=${authKey}\nMANAGEMENT_URL=${config.callback.worker_url}\n`);
  markResource(config, inventory, "protected_file", config.worker_vars_file_ref, "created", now);
  afterCreate({ type: "protected_file", id: config.worker_vars_file_ref });
  planResource(config, inventory, { type: "protected_file", id: config.callback.management_auth_key_file_ref, status: "planned" }, now);
  privateWrite(config.callback.management_auth_key_file_ref, authKey);
  markResource(config, inventory, "protected_file", config.callback.management_auth_key_file_ref, "created", now);
  afterCreate({ type: "protected_file", id: config.callback.management_auth_key_file_ref });
  for (const path of [config.callback.effect_ledger_file_ref, `${config.callback.effect_ledger_file_ref}-shm`, `${config.callback.effect_ledger_file_ref}-wal`]) {
    planResource(config, inventory, { type: "protected_file", id: path, status: "planned" }, now);
  }
  for (const node of config.nodes) {
    planResource(config, inventory, { type: "directory", id: node.state_dir, status: "planned" }, now);
    mkdirSync(node.state_dir, { mode: 0o700 });
    chmodSync(node.state_dir, 0o700);
    markResource(config, inventory, "directory", node.state_dir, "created", now);
    afterCreate({ type: "directory", id: node.state_dir });
  }
  inventory.state = "ready_to_start";
  persistInventory(config, inventory, now);
  return config;
}

function inventoryPath(config) {
  return join(config.run_root, "fleet-inventory.json");
}

function persistInventory(config, inventory, now = new Date(), fsOperations) {
  inventory.updated_at = now.toISOString();
  atomicJson(inventoryPath(config), inventory, fsOperations);
}

function planResource(config, inventory, resource, now = new Date()) {
  if (inventory.resources.some((candidate) => candidate.type === resource.type && candidate.id === resource.id)) return;
  inventory.resources.push({ ...resource, planned_at: now.toISOString(), updated_at: now.toISOString() });
  persistInventory(config, inventory, now);
}

function markResource(config, inventory, type, id, status, now = new Date()) {
  const resource = inventory.resources.find((candidate) => candidate.type === type && candidate.id === id);
  if (!resource) throw new Error(`resource was not inventoried before mutation: ${type}:${id}`);
  resource.status = status;
  resource.updated_at = now.toISOString();
  persistInventory(config, inventory, now);
}

function planAction(config, inventory, action, now = new Date(), fsOperations) {
  inventory.actions.push({ ...action, planned_at: now.toISOString(), status: "planned" });
  persistInventory(config, inventory, now, fsOperations);
  return inventory.actions.length - 1;
}

function completeAction(config, inventory, index, now = new Date()) {
  inventory.actions[index].status = "completed";
  inventory.actions[index].completed_at = now.toISOString();
  persistInventory(config, inventory, now);
}

function finishDiagnosisAction(config, inventory, index, { status, evidenceSha256, reasonCode }, now = new Date(), fsOperations) {
  const action = inventory.actions[index];
  if (action?.kind !== "celld_diagnose" || action.status !== "planned" || !HEX_SHA256.test(evidenceSha256 ?? "")
      || !["completed", "failed"].includes(status)) {
    throw new Error("fleet diagnosis action terminal is invalid");
  }
  action.status = status;
  action.evidence_sha256 = evidenceSha256;
  if (status === "completed") action.completed_at = now.toISOString();
  else {
    action.failed_at = now.toISOString();
    action.reason_code = reasonCode;
  }
  persistInventory(config, inventory, now, fsOperations);
}

function loadFixture(configPath, { requireWorkerVars = true } = {}) {
  const config = protectedJson(resolve(configPath), "fleet config");
  const errors = validateFleetConfig(config);
  if (errors.length) throw new Error(errors.join("; "));
  checkedRoot(config.run_root, config.run_id);
  const storage = protectedJson(config.storage_config_path, "storage fixture config");
  const storageErrors = validateFixtureConfig(storage);
  if (storageErrors.length || storage.run_id !== config.run_id || storage.run_root !== config.run_root) throw new Error(`storage fixture mismatch: ${storageErrors.join("; ")}`);
  const inventory = protectedJson(inventoryPath(config), "fleet inventory");
  const inventoryErrors = validateFleetInventory(inventory, config);
  if (inventoryErrors.length) throw new Error(inventoryErrors.join("; "));
  const workerVarsResource = inventory.resources.find((resource) => resource.type === "protected_file" && resource.id === config.worker_vars_file_ref);
  if (requireWorkerVars && !workerVarsResource) throw new Error("Worker vars file is absent from fleet inventory");
  if (workerVarsResource?.status === "removed") {
    if (existsSync(config.worker_vars_file_ref)) throw new Error("removed Worker vars file still exists");
  } else if (workerVarsResource && existsSync(config.worker_vars_file_ref)) {
    const metadata = lstatSync(config.worker_vars_file_ref);
    if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) throw new Error("Worker vars file must be a protected regular non-symlink file");
  } else if (workerVarsResource && workerVarsResource.status !== "planned") {
    throw new Error("created Worker vars file is missing");
  }
  return { config, storage, inventory };
}

function assertWorkerVarsReady(config, inventory) {
  const resource = inventory.resources.find((candidate) => candidate.type === "protected_file" && candidate.id === config.worker_vars_file_ref);
  if (resource?.status !== "created" || !existsSync(config.worker_vars_file_ref)) {
    throw new Error("Worker vars file is not ready; exact cleanup is required before mutation");
  }
}

function inspectContainer(runner, name) {
  try {
    return JSON.parse(runner("docker", ["inspect", name]));
  } catch (error) {
    if (error instanceof FleetDiagnosisDeadlineError) throw error;
    return null;
  }
}

function assertOwnedContainer(document, config, name) {
  const labels = document?.[0]?.Config?.Labels ?? {};
  for (const [key, expected] of Object.entries(exactLabels(config.run_id))) {
    if (labels[key] !== expected) throw new Error(`refusing unowned container ${name}`);
  }
}

function assertExactFleetNodeDocument(document, config, node) {
  if (!document || document.length !== 1) throw new Error(`fleet node ${node.name} is unavailable`);
  assertOwnedContainer(document, config, node.name);
  if (document[0]?.Config?.Image !== config.pins.celld.image_ref) throw new Error(`fleet node ${node.name} image identity is invalid`);
}

function assertStorageNetwork(runner, config, storage) {
  const network = JSON.parse(runner("docker", ["network", "inspect", config.network.name]));
  const labels = network?.[0]?.Labels ?? {};
  if (network?.length !== 1
      || network[0].Name !== config.network.name
      || network[0].Driver !== "bridge"
      || network[0].Scope !== "local"
      || network[0].Internal !== true
      || labels["com.docker.compose.project"] !== storage.project
      || labels["com.docker.compose.network"] !== "storage-private"
      || labels["dev.agentic-sandbox.run"] !== storage.run_id
      || labels["dev.agentic-sandbox.scope"] !== "celld-qualification") {
    throw new Error("storage-private network identity is invalid");
  }
  return network[0];
}

function storageNetworkGateway(network) {
  const gateway = network?.IPAM?.Config?.[0]?.Gateway;
  const octets = typeof gateway === "string" ? gateway.split(".").map(Number) : [];
  const validIpv4 = octets.length === 4 && octets.every((value) => Number.isInteger(value) && value >= 0 && value <= 255);
  const privateIpv4 = validIpv4 && (octets[0] === 10 || (octets[0] === 172 && octets[1] >= 16 && octets[1] <= 31) || (octets[0] === 192 && octets[1] === 168));
  if (!privateIpv4) {
    throw new Error("storage-private network gateway is not an RFC1918 IPv4 address");
  }
  return gateway;
}

function inspectFleetNodePrivateTarget(config, storage, node, document, runner) {
  assertExactFleetNodeDocument(document, config, node);
  if (document[0]?.State?.Running !== true) throw new Error(`fleet node ${node.name} is not running`);
  const network = assertStorageNetwork(runner, config, storage);
  const container = document[0];
  const containerId = container.Id;
  if (!CONTAINER_ID.test(containerId ?? "")) throw new Error(`fleet node ${node.name} container identity is invalid`);
  const networks = container.NetworkSettings?.Networks ?? {};
  const attachment = networks[config.network.name];
  if (Object.keys(networks).length !== 1 || !attachment || attachment.NetworkID !== network.Id) {
    throw new Error(`fleet node ${node.name} network attachment is invalid`);
  }
  const address = String(attachment.IPAddress ?? "");
  if (!isPrivateIpv4(address)) throw new Error(`fleet node ${node.name} private address is unavailable`);
  const member = network.Containers?.[containerId];
  if (member?.Name !== node.name || !String(member?.IPv4Address ?? "").startsWith(`${address}/`)) {
    throw new Error(`fleet node ${node.name} network membership is invalid`);
  }
  return { host: address, port: 8080 };
}

function discoverPublishedWorkerEndpoint(runner, nodeName) {
  try {
    const port = runner("docker", ["port", nodeName, "8080/tcp"]);
    const match = /^127\.0\.0\.1:(\d+)$/.exec(port.trim());
    if (!match) throw new Error(`node ${nodeName} public listener is not loopback-only`);
    return `http://127.0.0.1:${match[1]}`;
  } catch (error) {
    if (retryablePortDiscoveryError(error)) return null;
    throw error;
  }
}

export async function openFleetWorkerAccess(configPath, {
  runner = defaultRunner,
  nodeIndex = 0,
  forwarderFactory = startLoopbackForwarder,
} = {}) {
  const { config, storage } = loadFixture(configPath);
  if (!Number.isSafeInteger(nodeIndex) || nodeIndex < 0 || nodeIndex >= config.nodes.length) throw new Error("fleet Worker node selection is invalid");
  const node = config.nodes[nodeIndex];
  const document = inspectContainer(runner, node.name);
  const target = inspectFleetNodePrivateTarget(config, storage, node, document, runner);
  const forwarder = await forwarderFactory(target, { scheme: "http" });
  return {
    endpoint: forwarder.endpoint,
    node: node.name,
    close: () => forwarder.close(),
  };
}

export async function ensureFleetBucket(config, storage, runner, {
  gatewayAccessFactory = openStorageGatewayAccess,
  clientFactory = (profile) => new S3V1Client(profile),
} = {}) {
  const gatewayAccess = await gatewayAccessFactory(storage, { services: ["s3gateway1"], runner });
  const [endpoint] = gatewayAccess.endpoints;
  const profile = {
    schema_version: STORAGE_PROFILE_SCHEMA,
    profile_id: storage.project,
    run_id: storage.run_id,
    dialect: "s3-v1",
    scope: "live_candidate",
    endpoint,
    region: storage.region,
    addressing_mode: "path",
    bucket: storage.bucket,
    run_prefix: storage.run_prefix,
    identity_file_ref: storage.admin_identity_file_ref,
    ca_file_ref: storage.ca_file_ref,
    backend: {
      product: storage.backend.product,
      version: storage.backend.version,
      artifact_sha256: storage.backend.artifact_sha256,
      configuration_sha256: storage.backend.configuration_sha256,
      gateway_endpoints: [endpoint],
      topology: storage.backend.topology,
    },
    limits: storage.limits,
  };
  let client = null;
  let result = null;
  let operationError = null;
  try {
    client = clientFactory(profile);
    const create = await client.createBucket();
    if (create.status >= 200 && create.status < 300) result = { created: true };
    if (create.status === 409) {
      const existing = await client.listPrefix();
      if (existing.status === 200) result = { created: false };
    }
    if (!result) throw new Error(`fleet bucket creation returned ${create.status}`);
  } catch (error) {
    operationError = error;
  } finally {
    const cleanupErrors = [];
    try { client?.close(); } catch (error) { cleanupErrors.push(error); }
    try { await gatewayAccess.close(); } catch (error) { cleanupErrors.push(error); }
    if (cleanupErrors.length) {
      throw new AggregateError([...(operationError ? [operationError] : []), ...cleanupErrors], "fleet bucket gateway cleanup failed");
    }
  }
  if (operationError) throw operationError;
  return result;
}

function nodeCreateArgs(config, storage, node, launcher) {
  const uid = typeof process.getuid === "function" ? process.getuid() : 1000;
  const gid = typeof process.getgid === "function" ? process.getgid() : 1000;
  const endpoint = "https://s3gateway1:8334";
  const args = [
    "create", "--name", node.name,
    ...labelsToArgs(exactLabels(config.run_id)),
    "--network", config.network.name,
    "--user", `${uid}:${gid}`,
    "--read-only", "--security-opt", "no-new-privileges:true", "--cap-drop", "ALL",
    "--pids-limit", String(config.resources.pids_per_node),
    "--cpus", String(config.resources.cpu_per_node),
    "--memory", `${config.resources.memory_per_node_mb}m`,
    "--tmpfs", "/tmp:rw,noexec,nosuid,nodev,size=64m,mode=1777",
    "--mount", `type=bind,src=${node.state_dir},dst=/var/lib/celld`,
    "--mount", `type=bind,src=${storage.identity_file_ref},dst=/run/identity/credentials,readonly`,
    // Only the public CA certificate crosses into Celld. The fixture CA key and
    // S3 gateway private key remain controller/gateway-only.
    "--mount", `type=bind,src=${storage.ca_file_ref},dst=/run/tls/ca.crt,readonly`,
    "--mount", `type=bind,src=${config.worker_vars_file_ref},dst=/run/worker/vars,readonly`,
    "--publish", config.network.public_publish,
    "--env", "AWS_CA_BUNDLE=/run/tls/ca.crt",
    "--env", "SSL_CERT_FILE=/run/tls/ca.crt",
    "--env", `AWS_REGION=${storage.region}`,
    "--env", `CELLD_NODE=${node.node_id}`,
    "--env", "CELLD_WATCH=/var/lib/celld",
    "--env", "CELLD_STORAGE_PROBE=1",
    "--env", "CELLD_VARS_FILE=/run/worker/vars",
    "--env", `CELLD_ADDR=${config.network.public_listener}`,
    "--env", `CELLD_INTERNAL_ADDR=${config.network.internal_listener}`,
    "--env", `CELLD_ADVERTISE=${node.advertise}`,
    "--env", `CELLD_MAX_RESIDENT_CELLS=${config.resources.max_resident_cells}`,
    "--env", `CELLD_MAX_RSS_MB=${config.resources.max_rss_mb}`,
    ...credentialLauncherArgs(launcher),
    config.pins.celld.image_ref,
    "--bucket", `s3://${storage.bucket}/${storage.run_prefix}/fleet`,
    "--endpoint", endpoint,
    "--region", storage.region,
    "--listen", config.network.public_listener,
    "--internal-listen", config.network.internal_listener,
    "--advertise", node.advertise,
  ];
  return args;
}

export async function deployFleetWorker(configPath, {
  runner = defaultRunner,
  now = () => new Date(),
  esbuildPath = join(REPO_ROOT, "runtimes/celld/instance-cell/node_modules/@esbuild/linux-x64/bin/esbuild"),
  credentialLauncherPath = DEFAULT_CREDENTIAL_LAUNCHER_PATH,
  ensureBucket = ensureFleetBucket,
} = {}) {
  const { config, storage, inventory } = loadFixture(configPath);
  assertWorkerVarsReady(config, inventory);
  assertProtectedCredentialFile(storage.identity_file_ref);
  assertStorageNetwork(runner, config, storage);
  const workerRoot = join(REPO_ROOT, "runtimes/celld/instance-cell");
  const workerDigest = `sha256:${sha256(readFileSync(join(workerRoot, "worker.mjs")))}`;
  if (workerDigest !== loadWorkerDigest() || workerDigest !== config.pins.worker_digest) {
    throw new Error("reference Worker bytes do not match the reviewed digest");
  }
  const esbuild = resolve(esbuildPath);
  const esbuildMetadata = lstatSync(esbuild);
  if (!esbuildMetadata.isFile() || esbuildMetadata.isSymbolicLink() || (esbuildMetadata.mode & 0o111) === 0) {
    throw new Error("the fixed esbuild executable is missing or unsafe");
  }
  const launcher = credentialLauncher(credentialLauncherPath);
  if (inventory.credential_launcher_sha256 && inventory.credential_launcher_sha256 !== launcher.sha256) {
    throw new Error("credential launcher digest changed within the fleet run");
  }
  const deployer = `${storage.project}-celld-worker-deploy`;
  if (!SAFE_NAME.test(deployer)) throw new Error("Worker deployer name is invalid");
  const existing = inspectContainer(runner, deployer);
  if (existing) assertOwnedContainer(existing, config, deployer);
  const priorDeploy = [...inventory.actions].reverse().find((candidate) => candidate.kind === "celld_deploy");
  if (priorDeploy?.status === "planned") {
    throw new Error("Worker deployment outcome is unknown; exact cleanup is required before a new run");
  }
  if (priorDeploy?.status === "completed") {
    if (existing) throw new Error("completed Worker deployment left an owned deployer; exact cleanup is required");
    if (inventory.worker_digest !== workerDigest || !/^[0-9a-f]{64}$/.test(inventory.deployment_sha256 ?? "") || !/^[0-9a-f]{16}$/.test(inventory.deployment_version_id ?? "")) {
      throw new Error("completed Worker deployment inventory is inconsistent");
    }
    return {
      schema_version: "agentic-sandbox.celld-worker-deployment/v1",
      run_id: config.run_id,
      scope: config.scope,
      worker_digest: workerDigest,
      celld_manifest_digest: config.pins.celld.manifest_digest,
      credential_launcher_sha256: launcher.sha256,
      deployment_sha256: inventory.deployment_sha256,
      version_id: inventory.deployment_version_id,
      status: "DEPLOYED",
    };
  }
  const bucketAction = planAction(config, inventory, { kind: "s3_create_bucket", target_sha256: sha256(storage.bucket) }, now());
  const bucket = await ensureBucket(config, storage, runner);
  if (!bucket || typeof bucket.created !== "boolean") throw new Error("fleet bucket initializer returned invalid evidence");
  completeAction(config, inventory, bucketAction, now());
  inventory.bucket_ready_sha256 = sha256(storage.bucket);
  persistInventory(config, inventory, now());
  if (existing) {
    const staleAction = planAction(config, inventory, { kind: "docker_remove_stale", target: deployer }, now());
    runner("docker", ["rm", "--force", "--volumes", deployer], { timeout: 120_000 });
    completeAction(config, inventory, staleAction, now());
  }
  const priorResource = inventory.resources.find((resource) => resource.type === "docker_container" && resource.id === deployer);
  if (priorResource) markResource(config, inventory, "docker_container", deployer, "planned", now());
  else planResource(config, inventory, { type: "docker_container", id: deployer, status: "planned" }, now());
  const action = planAction(config, inventory, { kind: "celld_deploy", target: deployer, worker_digest: workerDigest, credential_launcher_sha256: launcher.sha256 }, now());
  const uid = typeof process.getuid === "function" ? process.getuid() : 1000;
  const gid = typeof process.getgid === "function" ? process.getgid() : 1000;
  const output = runner("docker", [
    "run", "--rm", "--name", deployer,
    ...labelsToArgs(exactLabels(config.run_id)),
    "--network", config.network.name,
    "--user", `${uid}:${gid}`,
    "--read-only", "--security-opt", "no-new-privileges:true", "--cap-drop", "ALL",
    "--pids-limit", "128", "--cpus", "1", "--memory", "1024m",
    "--tmpfs", "/tmp:rw,noexec,nosuid,nodev,size=128m,mode=1777",
    "--workdir", "/workspace",
    "--mount", `type=bind,src=${workerRoot},dst=/workspace,readonly`,
    "--mount", `type=bind,src=${esbuild},dst=/usr/local/bin/esbuild,readonly`,
    "--mount", `type=bind,src=${storage.identity_file_ref},dst=/run/identity/credentials,readonly`,
    "--mount", `type=bind,src=${storage.ca_file_ref},dst=/run/tls/ca.crt,readonly`,
    "--env", "AWS_CA_BUNDLE=/run/tls/ca.crt",
    "--env", "SSL_CERT_FILE=/run/tls/ca.crt",
    "--env", `AWS_REGION=${storage.region}`,
    "--env", "CELLD_ESBUILD=/usr/local/bin/esbuild",
    ...credentialLauncherArgs(launcher),
    config.pins.celld.image_ref,
    "deploy", "/workspace",
    "--bucket", `s3://${storage.bucket}/${storage.run_prefix}/fleet`,
    "--endpoint", "https://s3gateway1:8334",
    "--region", storage.region,
  ], { timeout: 300_000 });
  const versionId = /Current Version ID: ([0-9a-f]{16})/.exec(output)?.[1];
  if (!versionId) throw new Error("Celld deployment did not return an immutable version ID");
  completeAction(config, inventory, action, now());
  markResource(config, inventory, "docker_container", deployer, "removed", now());
  inventory.deployment_sha256 = sha256(output);
  inventory.deployment_version_id = versionId;
  inventory.worker_digest = workerDigest;
  inventory.credential_launcher_sha256 = launcher.sha256;
  persistInventory(config, inventory, now());
  return {
    schema_version: "agentic-sandbox.celld-worker-deployment/v1",
    run_id: config.run_id,
    scope: config.scope,
    worker_digest: workerDigest,
    celld_manifest_digest: config.pins.celld.manifest_digest,
    credential_launcher_sha256: launcher.sha256,
    deployment_sha256: inventory.deployment_sha256,
    version_id: versionId,
    status: "DEPLOYED",
  };
}

export function deployFleetQualificationWorker(configPath, {
  projectPath,
  projectDigest,
  deploymentKind,
  runner = defaultRunner,
  now = () => new Date(),
  esbuildPath = join(REPO_ROOT, "runtimes/celld/instance-cell/node_modules/@esbuild/linux-x64/bin/esbuild"),
  credentialLauncherPath = DEFAULT_CREDENTIAL_LAUNCHER_PATH,
} = {}) {
  const { config, storage, inventory } = loadFixture(configPath);
  assertWorkerVarsReady(config, inventory);
  assertProtectedCredentialFile(storage.identity_file_ref);
  assertStorageNetwork(runner, config, storage);
  const project = qualificationProjectRoot(config, projectPath, deploymentKind);
  const observedProjectDigest = workerDeploymentProjectDigest(project, { deploymentKind });
  if (!SHA256.test(projectDigest ?? "") || projectDigest !== observedProjectDigest) {
    throw new Error("qualification Worker project digest does not match its exact source tree");
  }
  const workerDigest = `sha256:${sha256(readFileSync(join(project, "worker.mjs")))}`;
  if (deploymentKind === "approved-reference" && workerDigest !== config.pins.worker_digest) {
    throw new Error("approved Worker bytes do not match the reviewed fleet pin");
  }
  const esbuild = resolve(esbuildPath);
  const esbuildMetadata = lstatSync(esbuild);
  if (!esbuildMetadata.isFile() || esbuildMetadata.isSymbolicLink() || (esbuildMetadata.mode & 0o111) === 0) {
    throw new Error("the fixed esbuild executable is missing or unsafe");
  }
  const launcher = credentialLauncher(credentialLauncherPath);
  if (inventory.credential_launcher_sha256 && inventory.credential_launcher_sha256 !== launcher.sha256) {
    throw new Error("credential launcher digest changed within the fleet run");
  }
  const deployer = `${storage.project}-celld-worker-qualify`;
  if (!SAFE_NAME.test(deployer)) throw new Error("qualification Worker deployer name is invalid");
  const existing = inspectContainer(runner, deployer);
  if (existing) {
    assertOwnedContainer(existing, config, deployer);
    throw new Error("qualification Worker deployer residue requires exact fleet cleanup");
  }
  const priorDeploy = [...inventory.actions].reverse().find((candidate) => candidate.kind === "celld_qualification_deploy");
  if (priorDeploy?.status === "planned") throw new Error("qualification Worker deployment outcome is unknown; exact fleet cleanup is required");
  const priorResource = inventory.resources.find((resource) => resource.type === "docker_container" && resource.id === deployer);
  if (priorResource) markResource(config, inventory, "docker_container", deployer, "planned", now());
  else planResource(config, inventory, { type: "docker_container", id: deployer, status: "planned" }, now());
  const action = planAction(config, inventory, {
    kind: "celld_qualification_deploy",
    target: deployer,
    deployment_kind: deploymentKind,
    project_sha256: observedProjectDigest,
    worker_digest: workerDigest,
    credential_launcher_sha256: launcher.sha256,
  }, now());
  const uid = typeof process.getuid === "function" ? process.getuid() : 1000;
  const gid = typeof process.getgid === "function" ? process.getgid() : 1000;
  const output = runner("docker", [
    "run", "--rm", "--name", deployer,
    ...labelsToArgs(exactLabels(config.run_id)),
    "--network", config.network.name,
    "--user", `${uid}:${gid}`,
    "--read-only", "--security-opt", "no-new-privileges:true", "--cap-drop", "ALL",
    "--pids-limit", "128", "--cpus", "1", "--memory", "1024m",
    "--tmpfs", "/tmp:rw,noexec,nosuid,nodev,size=128m,mode=1777",
    "--workdir", "/workspace",
    "--mount", `type=bind,src=${project},dst=/workspace,readonly`,
    "--mount", `type=bind,src=${esbuild},dst=/usr/local/bin/esbuild,readonly`,
    "--mount", `type=bind,src=${storage.identity_file_ref},dst=/run/identity/credentials,readonly`,
    "--mount", `type=bind,src=${storage.ca_file_ref},dst=/run/tls/ca.crt,readonly`,
    "--env", "AWS_CA_BUNDLE=/run/tls/ca.crt",
    "--env", "SSL_CERT_FILE=/run/tls/ca.crt",
    "--env", `AWS_REGION=${storage.region}`,
    "--env", "CELLD_ESBUILD=/usr/local/bin/esbuild",
    ...credentialLauncherArgs(launcher),
    config.pins.celld.image_ref,
    "deploy", "/workspace",
    "--bucket", `s3://${storage.bucket}/${storage.run_prefix}/fleet`,
    "--endpoint", "https://s3gateway1:8334",
    "--region", storage.region,
  ], { timeout: 300_000 });
  const versionId = /Current Version ID: ([0-9a-f]{16})/.exec(output)?.[1];
  if (!versionId) throw new Error("Celld qualification deployment did not return an immutable version ID");
  completeAction(config, inventory, action, now());
  markResource(config, inventory, "docker_container", deployer, "removed", now());
  inventory.credential_launcher_sha256 = launcher.sha256;
  inventory.active_worker_deployment = {
    deployment_kind: deploymentKind,
    project_sha256: observedProjectDigest,
    worker_digest: workerDigest,
    version_id: versionId,
    output_sha256: sha256(output),
  };
  persistInventory(config, inventory, now());
  return {
    schema_version: "agentic-sandbox.celld-worker-qualification-deployment/v1",
    run_id: config.run_id,
    deployment_kind: deploymentKind,
    project_sha256: observedProjectDigest,
    worker_digest: workerDigest,
    version_id: versionId,
    output_sha256: inventory.active_worker_deployment.output_sha256,
    credential_launcher_sha256: launcher.sha256,
    status: "DEPLOYED",
  };
}

export function startFleet(configPath, {
  runner = defaultRunner,
  now = () => new Date(),
  credentialLauncherPath = DEFAULT_CREDENTIAL_LAUNCHER_PATH,
  readinessPolicy,
} = {}) {
  const { config, storage, inventory } = loadFixture(configPath);
  assertWorkerVarsReady(config, inventory);
  assertProtectedCredentialFile(storage.identity_file_ref);
  assertStorageNetwork(runner, config, storage);
  const launcher = credentialLauncher(credentialLauncherPath);
  if (inventory.credential_launcher_sha256 && inventory.credential_launcher_sha256 !== launcher.sha256) {
    throw new Error("credential launcher digest changed within the fleet run");
  }
  inventory.credential_launcher_sha256 = launcher.sha256;
  persistInventory(config, inventory, now());
  const pullAction = planAction(config, inventory, { kind: "docker_pull", target: config.pins.celld.image_ref }, now());
  runner("docker", ["pull", "--quiet", config.pins.celld.image_ref], { timeout: 300_000 });
  completeAction(config, inventory, pullAction, now());
  runner("docker", ["image", "inspect", config.pins.celld.image_ref]);
  for (const node of config.nodes) {
    const existing = inspectContainer(runner, node.name);
    if (existing) {
      assertOwnedContainer(existing, config, node.name);
    } else {
      planResource(config, inventory, { type: "docker_container", id: node.name, status: "planned" }, now());
      const action = planAction(config, inventory, { kind: "docker_create", target: node.name }, now());
      runner("docker", nodeCreateArgs(config, storage, node, launcher), { env: fixtureEnvironment(storage), timeout: 120_000 });
      completeAction(config, inventory, action, now());
      markResource(config, inventory, "docker_container", node.name, "created", now());
    }
    const startAction = planAction(config, inventory, { kind: "docker_start", target: node.name }, now());
    runner("docker", ["start", node.name], { timeout: 120_000 });
    completeAction(config, inventory, startAction, now());
    markResource(config, inventory, "docker_container", node.name, "started", now());
  }
  inventory.state = "started";
  persistInventory(config, inventory, now());
  const policy = startupReadinessPolicy(readinessPolicy);
  let latest = null;
  for (let attempt = 1; attempt <= policy.maxAttempts; attempt += 1) {
    const attemptStartedAtMs = policy.clock();
    if (!Number.isFinite(attemptStartedAtMs)) throw new Error("fleet startup readiness clock is invalid");
    if (attemptStartedAtMs >= policy.deadlineAtMs) {
      const attemptMetadata = {
        number: Math.max(1, attempt - 1),
        maxAttempts: policy.maxAttempts,
        deadlineMs: policy.deadlineMs,
        backoffMs: policy.backoffMs,
      };
      latest = fleetDiagnosisDocument(config, storage, config.nodes.map((node) => ({
        name: node.name,
        node_id: node.node_id,
        role: node.role,
        running: false,
        public_endpoint: null,
      })), {
        attempt: attemptMetadata,
        status: "NOT_READY",
        probeSha256: null,
        retryable: false,
        reasonCode: "CELLD_DIAGNOSIS_DEADLINE_EXCEEDED",
        evidenceSha256: deadlineEvidenceSha256(sha256(config.nodes.map((node) => node.node_id).join("\n")), "before-attempt"),
      });
      const { config: currentConfig, inventory: currentInventory } = loadFixture(configPath);
      currentInventory.state = "not_ready";
      currentInventory.diagnosis_sha256 = sha256(JSON.stringify(latest));
      persistInventory(currentConfig, currentInventory, now());
      throw new FleetStartupReadinessError(latest);
    }
    latest = diagnoseFleet(configPath, {
      runner,
      mutateInventory: true,
      now,
      attempt: {
        number: attempt,
        maxAttempts: policy.maxAttempts,
        deadlineMs: policy.deadlineMs,
        backoffMs: policy.backoffMs,
        timeoutMs: Math.max(1, Math.min(120_000, Math.floor(policy.deadlineAtMs - attemptStartedAtMs))),
        deadlineAtMs: policy.deadlineAtMs,
        clock: policy.clock,
      },
    });
    const observedAtMs = policy.clock();
    if (!Number.isFinite(observedAtMs)) throw new Error("fleet startup readiness clock is invalid");
    if (latest.status === "READY" && observedAtMs <= policy.deadlineAtMs) return latest;
    if (latest.status === "READY") {
      latest = lateStartupDiagnosisEvidence(latest, policy);
      const { config: currentConfig, inventory: currentInventory } = loadFixture(configPath);
      currentInventory.state = "not_ready";
      currentInventory.diagnosis_sha256 = sha256(JSON.stringify(latest));
      persistInventory(currentConfig, currentInventory, now());
    }
    const exhausted = attempt >= policy.maxAttempts
      || observedAtMs >= policy.deadlineAtMs
      || observedAtMs + policy.backoffMs > policy.deadlineAtMs
      || latest.retryable !== true;
    if (exhausted) throw new FleetStartupReadinessError(latest);
    policy.wait(policy.backoffMs);
  }
  throw new FleetStartupReadinessError(latest);
}

export function stopFleetForWorkerDeployment(configPath, { runner = defaultRunner, now = () => new Date() } = {}) {
  const { config, inventory } = loadFixture(configPath);
  for (const node of config.nodes) {
    const document = inspectContainer(runner, node.name);
    if (!document) throw new Error(`Worker deployment stop requires exact node ${node.name}`);
    assertOwnedContainer(document, config, node.name);
    if (document[0]?.State?.Running === true) {
      const action = planAction(config, inventory, { kind: "docker_stop_for_worker_deployment", target: node.name }, now());
      runner("docker", ["stop", "--time", "20", node.name], { timeout: 60_000 });
      completeAction(config, inventory, action, now());
    }
    markResource(config, inventory, "docker_container", node.name, "created", now());
  }
  inventory.state = "stopped_for_worker_deployment";
  persistInventory(config, inventory, now());
  return {
    schema_version: "agentic-sandbox.celld-fleet-worker-stop/v1",
    run_id: config.run_id,
    nodes: config.nodes.map((node) => node.name),
    status: "STOPPED",
  };
}

export function startCallbackRelays(configPath, {
  runner = defaultRunner,
  now = () => new Date(),
  relayBinaryPath = join(REPO_ROOT, "tools/celld-callback-relay/target/x86_64-unknown-linux-musl/release/agentic-celld-callback-relay"),
  enableFaultSignal = false,
} = {}) {
  if (typeof enableFaultSignal !== "boolean") throw new Error("callback relay fault-signal selection must be boolean");
  const { config, storage, inventory } = loadFixture(configPath);
  assertWorkerVarsReady(config, inventory);
  const network = assertStorageNetwork(runner, config, storage);
  const gateway = storageNetworkGateway(network);
  const relayBinary = resolve(relayBinaryPath);
  const relayMetadata = lstatSync(relayBinary);
  if (!relayMetadata.isFile() || relayMetadata.isSymbolicLink() || (relayMetadata.mode & 0o111) === 0) {
    throw new Error("the callback relay executable is missing or unsafe");
  }
  for (const path of [
    config.callback.ca_file_ref,
    config.callback.relay_client_cert_file_ref,
    config.callback.relay_client_key_file_ref,
  ]) {
    const metadata = lstatSync(path);
    if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) {
      throw new Error("callback relay TLS material must be protected regular files");
    }
  }
  const relaySha256 = sha256(readFileSync(relayBinary));
  const uid = typeof process.getuid === "function" ? process.getuid() : 1000;
  const gid = typeof process.getgid === "function" ? process.getgid() : 1000;
  const relays = [];
  for (const node of config.nodes) {
    const nodeDocument = inspectContainer(runner, node.name);
    if (!nodeDocument || nodeDocument[0]?.State?.Running !== true) throw new Error(`callback relay requires running node ${node.name}`);
    assertOwnedContainer(nodeDocument, config, node.name);
    const name = `${node.name}-callback-relay`;
    const existing = inspectContainer(runner, name);
    if (existing) {
      assertOwnedContainer(existing, config, name);
    } else {
      planResource(config, inventory, { type: "docker_container", id: name, status: "planned" }, now());
      const action = planAction(config, inventory, { kind: "callback_relay_create", target: name, node: node.name, binary_sha256: relaySha256 }, now());
      runner("docker", [
        "create", "--name", name,
        ...labelsToArgs(exactLabels(config.run_id)),
        "--network", `container:${node.name}`,
        "--user", `${uid}:${gid}`,
        "--read-only", "--security-opt", "no-new-privileges:true", "--cap-drop", "ALL",
        "--pids-limit", "64", "--cpus", "0.25", "--memory", "64m",
        "--mount", `type=bind,src=${relayBinary},dst=/usr/local/bin/agentic-celld-callback-relay,readonly`,
        "--mount", `type=bind,src=${config.callback.ca_file_ref},dst=/run/tls/ca.crt,readonly`,
        "--mount", `type=bind,src=${config.callback.relay_client_cert_file_ref},dst=/run/tls/client.crt,readonly`,
        "--mount", `type=bind,src=${config.callback.relay_client_key_file_ref},dst=/run/tls/client.key,readonly`,
        "--entrypoint", "/usr/local/bin/agentic-celld-callback-relay",
        config.pins.celld.image_ref,
        "--listen", config.callback.relay_listener,
        "--target", `${gateway}:${config.callback.management_tls_port}`,
        "--server-name", config.callback.management_server_name,
        "--ca", "/run/tls/ca.crt",
        "--cert", "/run/tls/client.crt",
        "--key", "/run/tls/client.key",
        "--fault-signal", enableFaultSignal ? "enabled" : "disabled",
      ], { timeout: 120_000 });
      completeAction(config, inventory, action, now());
      markResource(config, inventory, "docker_container", name, "created", now());
    }
    const startAction = planAction(config, inventory, { kind: "callback_relay_start", target: name, node: node.name }, now());
    runner("docker", ["start", name], { timeout: 120_000 });
    completeAction(config, inventory, startAction, now());
    const running = inspectContainer(runner, name);
    if (!running || running[0]?.State?.Running !== true) throw new Error(`callback relay did not remain running for node ${node.name}`);
    assertOwnedContainer(running, config, name);
    markResource(config, inventory, "docker_container", name, "started", now());
    relays.push({ name, node: node.name, listener: "node-loopback", management_transport: "private-ca-mtls", response_loss_signal: enableFaultSignal ? "SIGUSR1" : null });
  }
  const result = {
    schema_version: "agentic-sandbox.celld-callback-relays/v1",
    run_id: config.run_id,
    scope: config.scope,
    binary_sha256: relaySha256,
    client_cn: config.callback.client_cn,
    relays,
    status: "READY",
  };
  inventory.callback_relays_sha256 = sha256(JSON.stringify(result));
  persistInventory(config, inventory, now());
  return result;
}

function observeFleetDiagnosisNodes(config, storage, runner) {
  return config.nodes.map((node) => {
    const document = inspectContainer(runner, node.name);
    if (!document) return { name: node.name, node_id: node.node_id, role: node.role, running: false, public_endpoint: null };
    assertExactFleetNodeDocument(document, config, node);
    const running = document[0]?.State?.Running === true;
    let publicEndpoint = null;
    if (running) {
      publicEndpoint = discoverPublishedWorkerEndpoint(runner, node.name);
      if (!publicEndpoint) inspectFleetNodePrivateTarget(config, storage, node, document, runner);
    }
    return { name: node.name, node_id: node.node_id, role: node.role, running, public_endpoint: publicEndpoint };
  });
}

function exactProbeReadiness(output, expectedNodeIds) {
  const lines = String(output).split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  const conditionalWrites = lines.filter((line) => line === PINNED_CONDITIONAL_WRITE_LINE).length;
  const peerLines = lines.filter((line) => /^ok peer\b/.test(line));
  const signedPeers = [];
  let invalidSignedProof = false;
  for (const line of peerLines) {
    const nodeId = parseSignedDirectPeerNodeId(line);
    if (!nodeId) invalidSignedProof = true;
    else signedPeers.push(nodeId);
  }
  const expected = new Set(expectedNodeIds);
  const duplicateOrForeign = signedPeers.length !== new Set(signedPeers).size
    || signedPeers.some((nodeId) => !expected.has(nodeId));
  if (conditionalWrites !== 1) return { ready: false, retryable: false, reasonCode: "CELLD_DIAGNOSIS_CONDITIONAL_WRITE_INVALID" };
  if (invalidSignedProof) {
    return { ready: false, retryable: false, reasonCode: "CELLD_DIAGNOSIS_SIGNED_PEER_PROOF_REQUIRED" };
  }
  if (duplicateOrForeign || signedPeers.length > expectedNodeIds.length) {
    return { ready: false, retryable: false, reasonCode: "CELLD_DIAGNOSIS_NODE_IDENTITY_INVALID" };
  }
  const ready = signedPeers.length === expectedNodeIds.length;
  return {
    ready,
    retryable: !ready,
    reasonCode: ready ? "CELLD_DIAGNOSIS_READY" : "CELLD_DIAGNOSIS_LEASE_OR_PEER_INCOMPLETE",
  };
}

function parseSignedDirectPeerNodeId(line) {
  const match = SIGNED_DIRECT_PEER_PREFIX.exec(line);
  if (!match) return null;
  const fields = new Map();
  for (const token of String(match[4] ?? "").split(/\s+/).filter(Boolean)) {
    const field = SIGNED_DIRECT_FIELD.exec(token);
    if (!field || fields.has(field[1])) return null;
    const validator = SIGNED_DIRECT_FIELD_VALIDATORS[field[1]];
    if (validator && !validator.test(field[2])) return null;
    fields.set(field[1], field[2]);
  }
  if (fields.get("protocol") !== PINNED_SIGNED_DIRECT_PROTOCOL) return null;
  return match[1];
}

function isFleetControllerSubprocessError(error) {
  return error instanceof FleetControllerSubprocessError
    || (error?.name === "FleetControllerSubprocessError"
      && typeof error.program === "string"
      && typeof error.operation === "string"
      && (Number.isInteger(error.exitStatus) || error.exitStatus === null)
      && (typeof error.signal === "string" || error.signal === null)
      && typeof error.timedOut === "boolean");
}

function retryableProbeError(error, expectedNodeIds) {
  if (!isFleetControllerSubprocessError(error)
      || error.program !== "docker"
      || error.operation !== "docker_exec_diagnose"
      || error.exitStatus !== 1
      || error.timedOut !== false
      || typeof error.controllerStdout !== "string") return false;
  const probe = exactProbeReadiness(error.controllerStdout, expectedNodeIds);
  return probe.ready === false && probe.retryable === true;
}

function retryablePortDiscoveryError(error) {
  return isFleetControllerSubprocessError(error)
    && error.program === "docker"
    && error.operation === "docker_port"
    && error.exitStatus === 1
    && error.signal === null
    && error.timedOut === false;
}

function diagnosisErrorEvidenceSha256(error, expectedNodeIdsSha256) {
  const classified = isFleetControllerSubprocessError(error) ? {
    kind: "controller_subprocess",
    program: error.program,
    operation: error.operation,
    exit_status: error.exitStatus,
    signal: error.signal,
    error_code: error.errorCode,
    timed_out: error.timedOut,
    stdout_sha256: error.stdoutSha256,
    stderr_sha256: error.stderrSha256,
  } : {
    kind: "unclassified",
    name: String(error?.name ?? "Error").slice(0, 128),
    message_sha256: sha256(String(error?.message ?? "")),
  };
  return diagnosisEvidenceSha256("error", JSON.stringify(classified), expectedNodeIdsSha256);
}

function diagnosisTiming(attempt) {
  const clock = attempt.clock ?? Date.now;
  if (typeof clock !== "function") throw new Error("fleet diagnosis attempt clock is invalid");
  const observedAtMs = clock();
  if (!Number.isFinite(observedAtMs)) throw new Error("fleet diagnosis attempt clock is invalid");
  const deadlineAtMs = attempt.deadlineAtMs ?? observedAtMs + attempt.deadlineMs;
  if (!Number.isFinite(deadlineAtMs)) {
    throw new Error("fleet diagnosis absolute deadline is invalid");
  }
  return { clock, deadlineAtMs };
}

function deadlineEvidenceSha256(expectedNodeIdsSha256, phase = "expired") {
  return diagnosisEvidenceSha256("deadline", phase, expectedNodeIdsSha256);
}

function diagnosisDeadlineRunner(runner, timing, expectedNodeIdsSha256) {
  return (program, args, options = {}) => {
    const invokedAtMs = timing.clock();
    if (!Number.isFinite(invokedAtMs)) throw new Error("fleet diagnosis attempt clock is invalid");
    const remainingMs = Math.floor(timing.deadlineAtMs - invokedAtMs);
    if (remainingMs <= 0) {
      throw new FleetDiagnosisDeadlineError(deadlineEvidenceSha256(expectedNodeIdsSha256, "before-invocation"));
    }
    const requestedTimeout = Number.isSafeInteger(options.timeout) && options.timeout > 0
      ? options.timeout
      : 120_000;
    let result;
    let invocationError = null;
    try {
      result = runner(program, args, { ...options, timeout: Math.min(requestedTimeout, remainingMs, 120_000) });
    } catch (error) {
      invocationError = error;
    }
    const completedAtMs = timing.clock();
    if (!Number.isFinite(completedAtMs)) throw new Error("fleet diagnosis attempt clock is invalid");
    if (completedAtMs > timing.deadlineAtMs || invocationError?.timedOut === true) {
      throw new FleetDiagnosisDeadlineError(deadlineEvidenceSha256(expectedNodeIdsSha256, "during-invocation"));
    }
    if (invocationError) throw invocationError;
    return result;
  };
}

function fleetDiagnosisDocument(config, storage, nodes, {
  attempt,
  status,
  probeSha256,
  retryable = false,
  reasonCode,
  evidenceSha256,
}) {
  const document = {
    schema_version: FLEET_DIAGNOSIS_SCHEMA,
    run_id: config.run_id,
    scope: config.scope,
    host_sha256: config.host_sha256,
    pins: config.pins,
    storage: { product: storage.backend.product, version: storage.backend.version, artifact_sha256: storage.backend.artifact_sha256, topology: storage.backend.topology },
    membership: {
      expected: 3,
      running: nodes.filter((node) => node.running).length,
      reserve: nodes.filter((node) => node.role === "reserve" && node.running).length,
      probe: status === "READY" ? "passed" : "failed",
      probe_sha256: probeSha256,
      attempts: attempt.number,
    },
    listeners: { public: "published on host loopback only", internal: `unpublished on ${config.network.name}` },
    instrumentation: config.instrumentation,
    nodes,
    status,
  };
  if (status !== "READY") {
    document.reason_code = STARTUP_NOT_READY;
    document.retryable = retryable;
    document.failure = {
      attempts: attempt.number,
      max_attempts: attempt.maxAttempts,
      deadline_ms: attempt.deadlineMs,
      backoff_ms: attempt.backoffMs,
      expected_node_ids_sha256: sha256(config.nodes.map((node) => node.node_id).join("\n")),
      reason_code: reasonCode,
      evidence_sha256: evidenceSha256,
    };
  }
  return document;
}

export function diagnoseFleet(configPath, {
  runner = defaultRunner,
  mutateInventory = false,
  now = () => new Date(),
  attempt = { number: 1, maxAttempts: 1, deadlineMs: 120_000, backoffMs: 0, timeoutMs: 120_000 },
  fsOperations,
} = {}) {
  const { config, storage, inventory } = loadFixture(configPath);
  if (!attempt || !Number.isSafeInteger(attempt.number) || attempt.number < 1
      || !Number.isSafeInteger(attempt.maxAttempts) || attempt.number > attempt.maxAttempts
      || !Number.isSafeInteger(attempt.deadlineMs) || attempt.deadlineMs < 1
      || !Number.isSafeInteger(attempt.backoffMs) || attempt.backoffMs < 0
      || !Number.isSafeInteger(attempt.timeoutMs ?? Math.min(120_000, attempt.deadlineMs))
      || (attempt.timeoutMs ?? Math.min(120_000, attempt.deadlineMs)) < 1
      || (attempt.timeoutMs ?? Math.min(120_000, attempt.deadlineMs)) > 120_000
      || (attempt.deadlineAtMs !== undefined && !Number.isFinite(attempt.deadlineAtMs))
      || (attempt.clock !== undefined && typeof attempt.clock !== "function")) {
    throw new Error("fleet diagnosis attempt metadata is invalid");
  }
  const expectedNodeIds = config.nodes.map((node) => node.node_id);
  const expectedNodeIdsSha256 = sha256(expectedNodeIds.join("\n"));
  const timing = diagnosisTiming(attempt);
  const boundedRunner = diagnosisDeadlineRunner(runner, timing, expectedNodeIdsSha256);
  const unknownNodes = () => config.nodes.map((node) => ({
    name: node.name,
    node_id: node.node_id,
    role: node.role,
    running: false,
    public_endpoint: null,
  }));
  const failureDocument = (nodes, reasonCode, evidenceSha256, retryable = false) => fleetDiagnosisDocument(config, storage, nodes, {
    attempt,
    status: "NOT_READY",
    probeSha256: null,
    retryable,
    reasonCode,
    evidenceSha256,
  });
  const persistResult = (result) => {
    if (!mutateInventory) return;
    inventory.state = "not_ready";
    inventory.diagnosis_sha256 = sha256(JSON.stringify(result));
    persistInventory(config, inventory, now(), fsOperations);
  };

  let nodes;
  try {
    nodes = observeFleetDiagnosisNodes(config, storage, boundedRunner);
  } catch (error) {
    if (retryablePortDiscoveryError(error)) {
      const result = failureDocument(
        unknownNodes(),
        "CELLD_DIAGNOSIS_TRANSIENT_PORT_DISCOVERY",
        diagnosisErrorEvidenceSha256(error, expectedNodeIdsSha256),
        true,
      );
      persistResult(result);
      return result;
    }
    if (!(error instanceof FleetDiagnosisDeadlineError)) throw error;
    const result = failureDocument(
      unknownNodes(),
      "CELLD_DIAGNOSIS_DEADLINE_EXCEEDED",
      error.evidenceSha256 ?? deadlineEvidenceSha256(expectedNodeIdsSha256),
    );
    persistResult(result);
    return result;
  }
  const containersRunning = nodes.every((node) => node.running) && nodes.length === 3;
  if (!containersRunning) {
    const evidenceSha256 = diagnosisEvidenceSha256("containers-not-running", nodes.map((node) => `${node.node_id}:${node.running}`).join("\n"), expectedNodeIdsSha256);
    const result = failureDocument(nodes, "CELLD_DIAGNOSIS_CONTAINER_NOT_RUNNING", evidenceSha256);
    persistResult(result);
    return result;
  }
  const primary = config.nodes[0];
  const action = planAction(config, inventory, {
    kind: "celld_diagnose",
    target: primary.name,
    attempt: attempt.number,
    max_attempts: attempt.maxAttempts,
    deadline_ms: attempt.deadlineMs,
    backoff_ms: attempt.backoffMs,
    expected_node_ids_sha256: expectedNodeIdsSha256,
  }, now(), fsOperations);
  let membershipProbe;
  try {
    membershipProbe = boundedRunner("docker", [
      "exec", primary.name, CREDENTIAL_LAUNCHER_CONTAINER_PATH, "diagnose",
      "--bucket", `s3://${storage.bucket}/${storage.run_prefix}/fleet`,
      "--endpoint", "https://s3gateway1:8334",
      "--region", storage.region,
      "--listen", "127.0.0.1:0",
      "--internal-listen", "127.0.0.1:0",
      ...config.nodes.flatMap((node) => ["--peer", node.node_id]),
    ], { timeout: attempt.timeoutMs ?? Math.min(120_000, attempt.deadlineMs) });
  } catch (error) {
    const deadlineExceeded = error instanceof FleetDiagnosisDeadlineError;
    const evidenceSha256 = deadlineExceeded
      ? error.evidenceSha256 ?? deadlineEvidenceSha256(expectedNodeIdsSha256)
      : diagnosisErrorEvidenceSha256(error, expectedNodeIdsSha256);
    const retryable = !deadlineExceeded && retryableProbeError(error, expectedNodeIds);
    const reasonCode = deadlineExceeded
      ? "CELLD_DIAGNOSIS_DEADLINE_EXCEEDED"
      : retryable
        ? "CELLD_DIAGNOSIS_TRANSIENT_LEASE_OR_PEER_READINESS"
        : "CELLD_DIAGNOSIS_NONRETRYABLE_FAILURE";
    finishDiagnosisAction(config, inventory, action, { status: "failed", evidenceSha256, reasonCode }, now(), fsOperations);
    const result = failureDocument(nodes, reasonCode, evidenceSha256, retryable);
    persistResult(result);
    return result;
  }
  const terminalObservedAtMs = timing.clock();
  if (!Number.isFinite(terminalObservedAtMs)) throw new Error("fleet diagnosis attempt clock is invalid");
  if (terminalObservedAtMs > timing.deadlineAtMs) {
    const evidenceSha256 = deadlineEvidenceSha256(expectedNodeIdsSha256, "before-terminal");
    const reasonCode = "CELLD_DIAGNOSIS_DEADLINE_EXCEEDED";
    finishDiagnosisAction(config, inventory, action, { status: "failed", evidenceSha256, reasonCode }, now(), fsOperations);
    const result = failureDocument(nodes, reasonCode, evidenceSha256);
    persistResult(result);
    return result;
  }
  const probe = exactProbeReadiness(membershipProbe, expectedNodeIds);
  const probeSha256 = diagnosisEvidenceSha256("output", membershipProbe, expectedNodeIdsSha256);
  finishDiagnosisAction(config, inventory, action, {
    status: probe.ready ? "completed" : "failed",
    evidenceSha256: probeSha256,
    reasonCode: probe.reasonCode,
  }, now(), fsOperations);
  const result = fleetDiagnosisDocument(config, storage, nodes, {
    attempt,
    status: probe.ready ? "READY" : "NOT_READY",
    probeSha256,
    retryable: probe.retryable,
    reasonCode: probe.reasonCode,
    evidenceSha256: probeSha256,
  });
  if (mutateInventory) {
    inventory.state = probe.ready ? "ready" : "not_ready";
    inventory.diagnosis_sha256 = sha256(JSON.stringify(result));
    persistInventory(config, inventory, now(), fsOperations);
  }
  return result;
}

export async function probeFleetWorker(configPath, {
  runner = defaultRunner,
  fetcher = fetch,
  now = () => new Date(),
  nonceFactory = () => randomBytes(16).toString("hex"),
  endpoint,
} = {}) {
  const { config, inventory } = loadFixture(configPath);
  assertWorkerVarsReady(config, inventory);
  const primary = config.nodes[0];
  const document = inspectContainer(runner, primary.name);
  if (!document) throw new Error("Worker probe requires the primary fleet node");
  assertOwnedContainer(document, config, primary.name);
  if (document[0]?.State?.Running !== true) throw new Error("Worker probe requires a running primary fleet node");
  let targetEndpoint = endpoint;
  if (targetEndpoint !== undefined && !/^http:\/\/127\.0\.0\.1:[1-9][0-9]{0,4}$/.test(targetEndpoint)) throw new Error("Worker probe endpoint must be host-loopback only");
  if (targetEndpoint === undefined) {
    const port = runner("docker", ["port", primary.name, "8080/tcp"]);
    const match = /^127\.0\.0\.1:(\d+)$/.exec(port.trim());
    if (!match) throw new Error("Worker probe endpoint must be host-loopback only");
    targetEndpoint = `http://127.0.0.1:${match[1]}`;
  }
  const instanceId = `qualification-${sha256(config.run_id).slice(0, 24)}`;
  const operationId = `probe-${sha256(`${config.run_id}:${primary.name}`).slice(0, 24)}`;
  const nonce = nonceFactory();
  const action = planAction(config, inventory, { kind: "worker_public_auth_probe", target: primary.name }, now());
  const checks = await probeWorkerAuthentication({ endpoint: targetEndpoint, varsFile: config.worker_vars_file_ref, instanceId, operationId, fetcher, now: now(), nonce });
  completeAction(config, inventory, action, now());
  const result = {
    schema_version: "agentic-sandbox.celld-worker-probe/v1",
    run_id: config.run_id,
    scope: config.scope,
    node: primary.name,
    worker_digest: config.pins.worker_digest,
    checks,
    status: "READY",
  };
  inventory.worker_probe_sha256 = sha256(JSON.stringify(result));
  persistInventory(config, inventory, now());
  return result;
}

export function cleanupFleet(configPath, { runner = defaultRunner, now = () => new Date(), removeState = true } = {}) {
  const { config, inventory } = loadFixture(configPath, { requireWorkerVars: false });
  const residue = reclaimInventoryTemporary(config, inventory);
  const inventoriedContainers = inventory.resources.filter((resource) => resource.type === "docker_container").map((resource) => resource.id);
  const containerNames = [...new Set([...config.nodes.map((node) => node.name), ...inventoriedContainers])].reverse();
  for (const name of containerNames) {
    const document = inspectContainer(runner, name);
    if (document) {
      assertOwnedContainer(document, config, name);
      const action = planAction(config, inventory, { kind: "docker_remove", target: name }, now());
      try {
        runner("docker", ["rm", "--force", "--volumes", name], { timeout: 120_000 });
        completeAction(config, inventory, action, now());
      } catch {
        if (inspectContainer(runner, name)) residue.push(name);
      }
    }
    const resource = inventory.resources.find((candidate) => candidate.type === "docker_container" && candidate.id === name);
    if (inspectContainer(runner, name)) residue.push(name);
    else if (resource) markResource(config, inventory, "docker_container", name, "removed", now());
  }
  const filters = Object.entries(exactLabels(config.run_id)).flatMap(([key, value]) => ["--filter", `label=${key}=${value}`]);
  let remaining;
  try {
    remaining = runner("docker", ["ps", "--all", ...filters, "--format", "{{.Names}}"]).split(/\r?\n/).filter(Boolean);
  } catch (error) {
    throw new CleanupResidueError(`fleet cleanup could not prove container absence: ${sha256(error.message)}`);
  }
  residue.push(...remaining);
  if (removeState) {
    for (const node of config.nodes) {
      if (existsSync(node.state_dir)) {
        try { rmSync(node.state_dir, { recursive: true, force: false }); } catch { residue.push(node.state_dir); }
      }
      const resource = inventory.resources.find((candidate) => candidate.type === "directory" && candidate.id === node.state_dir);
      if (existsSync(node.state_dir)) residue.push(node.state_dir);
      else if (resource) markResource(config, inventory, "directory", node.state_dir, "removed", now());
    }
    for (const resource of inventory.resources.filter((candidate) => candidate.type === "protected_file")) {
      if (existsSync(resource.id)) {
        const metadata = lstatSync(resource.id);
        if (!metadata.isFile() || metadata.isSymbolicLink() || !resource.id.startsWith(`${join(config.run_root, "fleet")}/`)) throw new Error("refusing unsafe protected-file cleanup target");
        try { rmSync(resource.id, { force: false }); } catch { residue.push(resource.id); }
      }
      if (existsSync(resource.id)) residue.push(resource.id);
      else markResource(config, inventory, "protected_file", resource.id, "removed", now());
    }
    const fleetRoot = join(config.run_root, "fleet");
    if (existsSync(fleetRoot)) {
      try { rmdirSync(fleetRoot); } catch { residue.push(fleetRoot); }
    }
    const rootResource = inventory.resources.find((candidate) => candidate.type === "directory" && candidate.id === fleetRoot);
    if (existsSync(fleetRoot)) residue.push(fleetRoot);
    else if (rootResource) markResource(config, inventory, "directory", fleetRoot, "removed", now());
  }
  for (const resource of inventory.resources) {
    if (resource.status !== "removed" && resource.type !== "docker_container") residue.push(resource.id);
  }
  const uniqueResidue = [...new Set(residue)];
  inventory.state = uniqueResidue.length ? "cleanup_residue" : "clean";
  persistInventory(config, inventory, now());
  if (uniqueResidue.length) throw new CleanupResidueError(`fleet cleanup residue: ${uniqueResidue.join(",")}`);
  return { status: "PASS", run_id: config.run_id, scope: config.scope, removed_containers: containerNames.length, residue: [] };
}

export function janitorPreview(root, { minimumAgeSeconds, now = new Date() } = {}) {
  const resolvedRoot = resolve(root ?? "");
  if (!isAbsolute(root ?? "") || !existsSync(resolvedRoot)) throw new Error("janitor root must be an existing absolute directory");
  if (!Number.isSafeInteger(minimumAgeSeconds) || minimumAgeSeconds < 3600) throw new Error("janitor minimum age must be at least one hour");
  const targets = [];
  const retained = [];
  for (const entry of readdirSync(resolvedRoot, { withFileTypes: true })) {
    if (!entry.isDirectory() || entry.isSymbolicLink() || !RUN_ID.test(entry.name)) continue;
    const runRoot = join(resolvedRoot, entry.name);
    const inventoryFile = join(runRoot, "fleet-inventory.json");
    const configFile = join(runRoot, "fleet.json");
    if (!existsSync(inventoryFile) && !existsSync(configFile)) continue;
    try {
      const inventory = protectedJson(inventoryFile, "fleet inventory");
      const ageSeconds = Math.floor((now.getTime() - Date.parse(inventory.created_at)) / 1000);
      const owned = inventory.owner?.repository === FLEET_OWNER.repository && inventory.owner?.workflow === FLEET_OWNER.workflow && inventory.owner?.run_id === entry.name;
      if (!owned || !existsSync(configFile) || ageSeconds < minimumAgeSeconds) retained.push({ run_id: entry.name, reason: !owned ? "ownership_mismatch" : !existsSync(configFile) ? "partial_inventory" : "minimum_age" });
      else targets.push({ run_id: entry.name, config_path: configFile, age_seconds: ageSeconds });
    } catch {
      retained.push({ run_id: entry.name, reason: "ambiguous_inventory" });
    }
  }
  return { schema_version: "agentic-sandbox.celld-fleet-janitor/v1", root: resolvedRoot, minimum_age_seconds: minimumAgeSeconds, targets, retained };
}

export function reapJanitor(preview, { runner = defaultRunner, now = () => new Date() } = {}) {
  const results = [];
  for (const target of preview.targets) results.push(cleanupFleet(target.config_path, { runner, now }));
  return { ...preview, reaped: results.map((result) => result.run_id) };
}

function argument(args, name) {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : undefined;
}

function integerArgument(args, name, fallback) {
  const value = argument(args, name) ?? String(fallback);
  if (!/^\d+$/.test(value)) throw new Error(`${name} must be an integer`);
  return Number(value);
}

async function main(args) {
  const command = args[0];
  if (command === "prepare") {
    const config = prepareFleet({
      storageConfigPath: resolve(argument(args, "--storage-config") ?? ""),
      outputPath: argument(args, "--output"),
      celldChannel: argument(args, "--celld-channel") ?? "approved",
    });
    console.log(JSON.stringify({ status: "PASS", config_path: join(config.run_root, "fleet.json"), run_id: config.run_id, scope: config.scope }));
    return 0;
  }
  if (command === "start") {
    const credentialLauncherPath = argument(args, "--credential-launcher");
    if (!credentialLauncherPath) throw new Error("--credential-launcher is required");
    const result = startFleet(resolve(argument(args, "--config") ?? ""), {
      credentialLauncherPath: resolve(credentialLauncherPath),
    });
    console.log(JSON.stringify(result));
    return result.status === "READY" ? 0 : 3;
  }
  if (command === "start-relays") {
    const faultSignal = argument(args, "--fault-signal") ?? "disabled";
    if (!["enabled", "disabled"].includes(faultSignal)) throw new Error("--fault-signal must be enabled or disabled");
    const result = startCallbackRelays(resolve(argument(args, "--config") ?? ""), {
      relayBinaryPath: resolve(argument(args, "--relay-binary") ?? ""),
      enableFaultSignal: faultSignal === "enabled",
    });
    console.log(JSON.stringify(result));
    return result.status === "READY" ? 0 : 3;
  }
  if (command === "deploy") {
    const credentialLauncherPath = argument(args, "--credential-launcher");
    if (!credentialLauncherPath) throw new Error("--credential-launcher is required");
    console.log(JSON.stringify(await deployFleetWorker(resolve(argument(args, "--config") ?? ""), {
      credentialLauncherPath: resolve(credentialLauncherPath),
    })));
    return 0;
  }
  if (command === "diagnose") {
    const result = diagnoseFleet(resolve(argument(args, "--config") ?? ""));
    console.log(JSON.stringify(result));
    return result.status === "READY" ? 0 : 3;
  }
  if (command === "probe-worker") {
    console.log(JSON.stringify(await probeFleetWorker(resolve(argument(args, "--config") ?? ""))));
    return 0;
  }
  if (command === "cleanup") {
    console.log(JSON.stringify(cleanupFleet(resolve(argument(args, "--config") ?? ""))));
    return 0;
  }
  if (command === "janitor-preview" || command === "janitor-reap") {
    const preview = janitorPreview(resolve(argument(args, "--root") ?? ""), { minimumAgeSeconds: integerArgument(args, "--minimum-age-seconds", 21600) });
    const result = command === "janitor-reap" ? reapJanitor(preview) : preview;
    console.log(JSON.stringify(result));
    return 0;
  }
  throw new Error("usage: celld-fleet-fixture.mjs <prepare|deploy|start|start-relays|diagnose|probe-worker|cleanup|janitor-preview|janitor-reap> [options]");
}

if (process.argv[1] && SCRIPT_PATH === resolve(process.argv[1])) {
  main(process.argv.slice(2)).then((code) => { process.exitCode = code; }).catch((error) => {
    if (error instanceof FleetStartupReadinessError) {
      console.log(JSON.stringify(error.evidence));
      process.exitCode = error.exitCode;
      return;
    }
    console.error(JSON.stringify({ status: "ERROR", reason_code: error instanceof CleanupResidueError ? "CELLD_FLEET_CLEANUP_RESIDUE" : "CELLD_FLEET_FIXTURE_ERROR", error_sha256: sha256(error.message) }));
    process.exitCode = error.exitCode ?? 3;
  });
}
