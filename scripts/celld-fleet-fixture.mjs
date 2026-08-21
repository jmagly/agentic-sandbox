#!/usr/bin/env node

import { createHash, createHmac, randomBytes } from "node:crypto";
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { hostname } from "node:os";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

import { fixtureEnvironment, validateFixtureConfig } from "./celld-seaweedfs-fixture.mjs";
import { S3V1Client, STORAGE_PROFILE_SCHEMA } from "./celld-storage-qualifier.mjs";

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
const EXPECTED_IMAGE = Object.freeze({
  version: "0.2.1",
  commit: "ae8fac053d79f971bfcb996054bb43eb2f9b05da",
  index_digest: "sha256:7a4380721b6400073f2a26afe70a828410169f658d31b5ef61383e648ca0c530",
  manifest_digest: "sha256:8634eac20f69ffe99103d403b985c0afd43fd970badadd01435f297ba0df797a",
});
const EXPECTED_WORKER_DIGEST = "sha256:97ba7bb98beb18d007e471d8bd731006d29f5c35c3c7829ee27c71ba0d487716";
const RESOURCE_LABELS = Object.freeze({
  repository: "dev.agentic-sandbox.repository",
  workflow: "dev.agentic-sandbox.workflow",
  run: "dev.agentic-sandbox.run",
  scope: "dev.agentic-sandbox.scope",
});

export class CleanupResidueError extends Error {
  constructor(message) {
    super(message);
    this.name = "CleanupResidueError";
    this.exitCode = 4;
  }
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function privateWrite(path, value, { exclusive = true } = {}) {
  writeFileSync(path, value, { mode: 0o600, flag: exclusive ? "wx" : "w" });
  chmodSync(path, 0o600);
}

function atomicJson(path, value) {
  const temporary = `${path}.new`;
  privateWrite(temporary, `${JSON.stringify(value, null, 2)}\n`, { exclusive: false });
  renameSync(temporary, path);
  chmodSync(path, 0o600);
}

function protectedJson(path, description) {
  if (!isAbsolute(path) || !existsSync(path)) throw new Error(`${description} is missing`);
  const metadata = lstatSync(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) {
    throw new Error(`${description} must be a protected regular non-symlink file`);
  }
  return JSON.parse(readFileSync(path, "utf8"));
}

function loadImages() {
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
  return value.celld;
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

function defaultRunner(program, args, options = {}) {
  const result = spawnSync(program, args, { encoding: "utf8", shell: false, ...options });
  if (result.error || result.status !== 0) {
    throw new Error(`${basename(program)} failed: ${(result.error?.message ?? result.stderr ?? "").trim()}`);
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
  const allowed = new Set(["schema_version", "run_id", "run_root", "scope", "host_sha256", "owner", "storage_config_path", "worker_vars_file_ref", "network", "pins", "nodes", "resources", "instrumentation", "operator_commands"]);
  for (const key of Object.keys(config)) if (!allowed.has(key)) errors.push(`config.${key} is not allowed`);
  if (config.schema_version !== FLEET_SCHEMA) errors.push(`config.schema_version must be ${FLEET_SCHEMA}`);
  if (!RUN_ID.test(config.run_id ?? "")) errors.push("config.run_id is invalid");
  if (!isAbsolute(config.run_root ?? "") || !config.run_root?.split("/").includes(config.run_id)) errors.push("config.run_root must contain run_id");
  if (config.scope !== "single-host multi-node") errors.push("config.scope must be single-host multi-node");
  if (!/^[0-9a-f]{64}$/.test(config.host_sha256 ?? "")) errors.push("config.host_sha256 is invalid");
  if (config.owner?.repository !== FLEET_OWNER.repository || config.owner?.workflow !== FLEET_OWNER.workflow || config.owner?.run_id !== config.run_id) errors.push("config.owner is invalid");
  if (config.storage_config_path !== join(config.run_root ?? "", "fixture.json")) errors.push("config.storage_config_path must be the exact run storage config");
  if (config.worker_vars_file_ref !== join(config.run_root ?? "", "fleet/worker-vars")) errors.push("config.worker_vars_file_ref must be the fixed protected file");
  if (!SAFE_NAME.test(config.network?.name ?? "") || config.network?.scope !== "storage-private" || config.network?.internal_listener !== "0.0.0.0:8081" || config.network?.public_listener !== "0.0.0.0:8080" || config.network?.public_publish !== "127.0.0.1::8080") errors.push("config.network is invalid");
  for (const [key, expected] of Object.entries(EXPECTED_IMAGE)) if (config.pins?.celld?.[key] !== expected) errors.push(`config.pins.celld.${key} is invalid`);
  if (config.pins?.celld?.image_ref !== `ghcr.io/denoland/celld@${EXPECTED_IMAGE.manifest_digest}`) errors.push("config.pins.celld.image_ref is invalid");
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
  if (JSON.stringify(config.operator_commands) !== JSON.stringify(["prepare", "deploy", "start", "diagnose", "probe-worker", "cleanup", "janitor-preview", "janitor-reap"])) errors.push("config.operator_commands is invalid");
  return errors;
}

export function validateFleetInventory(inventory, config) {
  const errors = [];
  if (inventory?.schema_version !== INVENTORY_SCHEMA) errors.push(`inventory.schema_version must be ${INVENTORY_SCHEMA}`);
  if (inventory?.run_id !== config.run_id || inventory?.owner?.repository !== FLEET_OWNER.repository || inventory?.owner?.workflow !== FLEET_OWNER.workflow || inventory?.owner?.run_id !== config.run_id) errors.push("inventory owner does not match config");
  if (!Array.isArray(inventory?.resources) || !Array.isArray(inventory?.actions)) errors.push("inventory resources/actions must be arrays");
  const keys = new Set();
  for (const resource of inventory?.resources ?? []) {
    const key = `${resource.type}:${resource.id}`;
    if (keys.has(key)) errors.push(`inventory resource is duplicated: ${key}`);
    keys.add(key);
    if (!new Set(["directory", "protected_file", "docker_container"]).has(resource.type) || !SAFE_NAME.test(resource.id.replaceAll("/", "-").replace(/^-+/, "")) || !["planned", "created", "started", "removed"].includes(resource.status)) errors.push(`inventory resource is invalid: ${key}`);
  }
  return errors;
}

export function prepareFleet({ storageConfigPath, outputPath, now = new Date() }) {
  const storage = protectedJson(resolve(storageConfigPath), "storage fixture config");
  const storageErrors = validateFixtureConfig(storage);
  if (storageErrors.length) throw new Error(storageErrors.join("; "));
  if (storage.fixture_profile !== "titan-single-host-storage" || !storage.promoting) throw new Error("fleet requires the exact promoting Titan storage profile");
  const runRoot = checkedRoot(storage.run_root, storage.run_id);
  const expectedOutput = join(runRoot, "fleet.json");
  if (resolve(outputPath ?? expectedOutput) !== expectedOutput) throw new Error("fleet config output must remain at the fixed run-root path");
  if (existsSync(expectedOutput) || existsSync(join(runRoot, "fleet-inventory.json"))) throw new Error("fleet fixture already exists for this run");
  const image = loadImages();
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
    network: {
      name: `${storage.project}_storage-private`,
      scope: "storage-private",
      internal_listener: "0.0.0.0:8081",
      public_listener: "0.0.0.0:8080",
      public_publish: "127.0.0.1::8080",
    },
    pins: {
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
    operator_commands: ["prepare", "deploy", "start", "diagnose", "probe-worker", "cleanup", "janitor-preview", "janitor-reap"],
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
  planResource(config, inventory, { type: "protected_file", id: config.worker_vars_file_ref, status: "planned" }, now);
  const authKeyId = `run-${sha256(config.run_id).slice(0, 20)}`;
  const authKey = randomBytes(32).toString("base64url");
  privateWrite(config.worker_vars_file_ref, `CELL_AUTH_KEY_ID=${authKeyId}\nCELL_AUTH_KEY=${authKey}\nMANAGEMENT_URL=https://management.internal/\n`);
  markResource(config, inventory, "protected_file", config.worker_vars_file_ref, "created", now);
  for (const node of config.nodes) {
    planResource(config, inventory, { type: "directory", id: node.state_dir, status: "planned" }, now);
    mkdirSync(node.state_dir, { mode: 0o700 });
    chmodSync(node.state_dir, 0o700);
    markResource(config, inventory, "directory", node.state_dir, "created", now);
  }
  inventory.state = "ready_to_start";
  persistInventory(config, inventory, now);
  return config;
}

function inventoryPath(config) {
  return join(config.run_root, "fleet-inventory.json");
}

function persistInventory(config, inventory, now = new Date()) {
  inventory.updated_at = now.toISOString();
  atomicJson(inventoryPath(config), inventory);
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

function planAction(config, inventory, action, now = new Date()) {
  inventory.actions.push({ ...action, planned_at: now.toISOString(), status: "planned" });
  persistInventory(config, inventory, now);
  return inventory.actions.length - 1;
}

function completeAction(config, inventory, index, now = new Date()) {
  inventory.actions[index].status = "completed";
  inventory.actions[index].completed_at = now.toISOString();
  persistInventory(config, inventory, now);
}

function loadFixture(configPath) {
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
  if (!workerVarsResource) throw new Error("Worker vars file is absent from fleet inventory");
  if (workerVarsResource.status === "removed") {
    if (existsSync(config.worker_vars_file_ref)) throw new Error("removed Worker vars file still exists");
  } else if (existsSync(config.worker_vars_file_ref)) {
    const metadata = lstatSync(config.worker_vars_file_ref);
    if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) throw new Error("Worker vars file must be a protected regular non-symlink file");
  } else if (workerVarsResource.status !== "planned") {
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

function readWorkerVars(config) {
  const values = new Map();
  for (const line of readFileSync(config.worker_vars_file_ref, "utf8").split(/\r?\n/)) {
    if (line === "") continue;
    const match = /^([A-Z][A-Z0-9_]*)=(.*)$/.exec(line);
    if (!match || values.has(match[1])) throw new Error("Worker vars file is malformed");
    values.set(match[1], match[2]);
  }
  if (values.size !== 3 || !/^run-[a-f0-9]{20}$/.test(values.get("CELL_AUTH_KEY_ID") ?? "") || Buffer.byteLength(values.get("CELL_AUTH_KEY") ?? "") < 32 || values.get("MANAGEMENT_URL") !== "https://management.internal/") {
    throw new Error("Worker vars file does not contain the exact required values");
  }
  return { keyId: values.get("CELL_AUTH_KEY_ID"), key: values.get("CELL_AUTH_KEY") };
}

async function boundedJson(response) {
  const declared = response.headers.get("content-length");
  if (declared !== null && (!/^\d+$/.test(declared) || Number(declared) > 4096)) throw new Error("Worker probe response exceeds the evidence bound");
  const bytes = Buffer.from(await response.arrayBuffer());
  if (bytes.length > 4096) throw new Error("Worker probe response exceeds the evidence bound");
  try { return JSON.parse(bytes.toString("utf8")); }
  catch { throw new Error("Worker probe response is not JSON"); }
}

async function workerProbeRequest(fetcher, endpoint, path, headers) {
  const response = await fetcher(new URL(path, endpoint), { method: "GET", headers, redirect: "error", signal: AbortSignal.timeout(10_000) });
  return { status: response.status, body: await boundedJson(response) };
}

function inspectContainer(runner, name) {
  try {
    return JSON.parse(runner("docker", ["inspect", name]));
  } catch {
    return null;
  }
}

function assertOwnedContainer(document, config, name) {
  const labels = document?.[0]?.Config?.Labels ?? {};
  for (const [key, expected] of Object.entries(exactLabels(config.run_id))) {
    if (labels[key] !== expected) throw new Error(`refusing unowned container ${name}`);
  }
}

function assertStorageNetwork(runner, config, storage) {
  const network = JSON.parse(runner("docker", ["network", "inspect", config.network.name]));
  const labels = network?.[0]?.Labels ?? {};
  if (labels["com.docker.compose.project"] !== storage.project || labels["dev.agentic-sandbox.scope"] !== "celld-qualification") {
    throw new Error("storage-private network identity is invalid");
  }
}

async function ensureFleetBucket(config, storage, runner) {
  const port = runner("docker", ["compose", "-f", storage.compose_file, "-p", storage.project, "port", "s3gateway1", "8334"], { env: fixtureEnvironment(storage) });
  const match = /(?:127\.0\.0\.1|0\.0\.0\.0|\[::\]):(\d+)$/.exec(port.trim());
  if (!match) throw new Error("could not resolve the fleet S3 gateway TLS port");
  const endpoint = `https://127.0.0.1:${match[1]}`;
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
  const client = new S3V1Client(profile);
  try {
    const create = await client.createBucket();
    if (create.status >= 200 && create.status < 300) return { created: true };
    if (create.status === 409) {
      const existing = await client.listPrefix();
      if (existing.status === 200) return { created: false };
    }
    throw new Error(`fleet bucket creation returned ${create.status}`);
  } finally {
    client.close();
  }
}

function nodeCreateArgs(config, storage, node) {
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
    "--env", "AWS_SHARED_CREDENTIALS_FILE=/run/identity/credentials",
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
  ensureBucket = ensureFleetBucket,
} = {}) {
  const { config, storage, inventory } = loadFixture(configPath);
  assertWorkerVarsReady(config, inventory);
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
    if (inventory.worker_digest !== workerDigest || !/^[0-9a-f]{64}$/.test(inventory.deployment_sha256 ?? "")) {
      throw new Error("completed Worker deployment inventory is inconsistent");
    }
    return {
      schema_version: "agentic-sandbox.celld-worker-deployment/v1",
      run_id: config.run_id,
      scope: config.scope,
      worker_digest: workerDigest,
      celld_manifest_digest: config.pins.celld.manifest_digest,
      deployment_sha256: inventory.deployment_sha256,
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
  const action = planAction(config, inventory, { kind: "celld_deploy", target: deployer, worker_digest: workerDigest }, now());
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
    "--env", "AWS_SHARED_CREDENTIALS_FILE=/run/identity/credentials",
    "--env", "AWS_CA_BUNDLE=/run/tls/ca.crt",
    "--env", "SSL_CERT_FILE=/run/tls/ca.crt",
    "--env", `AWS_REGION=${storage.region}`,
    "--env", "CELLD_ESBUILD=/usr/local/bin/esbuild",
    config.pins.celld.image_ref,
    "deploy", "/workspace",
    "--bucket", `s3://${storage.bucket}/${storage.run_prefix}/fleet`,
    "--endpoint", "https://s3gateway1:8334",
    "--region", storage.region,
  ], { timeout: 300_000 });
  completeAction(config, inventory, action, now());
  markResource(config, inventory, "docker_container", deployer, "removed", now());
  inventory.deployment_sha256 = sha256(output);
  inventory.worker_digest = workerDigest;
  persistInventory(config, inventory, now());
  return {
    schema_version: "agentic-sandbox.celld-worker-deployment/v1",
    run_id: config.run_id,
    scope: config.scope,
    worker_digest: workerDigest,
    celld_manifest_digest: config.pins.celld.manifest_digest,
    deployment_sha256: inventory.deployment_sha256,
    status: "DEPLOYED",
  };
}

export function startFleet(configPath, { runner = defaultRunner, now = () => new Date() } = {}) {
  const { config, storage, inventory } = loadFixture(configPath);
  assertWorkerVarsReady(config, inventory);
  assertStorageNetwork(runner, config, storage);
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
      runner("docker", nodeCreateArgs(config, storage, node), { env: fixtureEnvironment(storage), timeout: 120_000 });
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
  return diagnoseFleet(configPath, { runner, mutateInventory: true, now });
}

export function diagnoseFleet(configPath, { runner = defaultRunner, mutateInventory = false, now = () => new Date() } = {}) {
  const { config, storage, inventory } = loadFixture(configPath);
  const nodes = config.nodes.map((node) => {
    const document = inspectContainer(runner, node.name);
    if (!document) return { name: node.name, node_id: node.node_id, role: node.role, running: false, public_endpoint: null };
    assertOwnedContainer(document, config, node.name);
    const running = document[0]?.State?.Running === true;
    let publicEndpoint = null;
    if (running) {
      const port = runner("docker", ["port", node.name, "8080/tcp"]);
      const match = /127\.0\.0\.1:(\d+)$/.exec(port);
      if (!match) throw new Error(`node ${node.name} public listener is not loopback-only`);
      publicEndpoint = `http://127.0.0.1:${match[1]}`;
    }
    return { name: node.name, node_id: node.node_id, role: node.role, running, public_endpoint: publicEndpoint };
  });
  let membershipProbe = null;
  const containersRunning = nodes.every((node) => node.running) && nodes.length === 3;
  if (containersRunning) {
    const primary = config.nodes[0];
    const action = planAction(config, inventory, { kind: "celld_diagnose", target: primary.name }, now());
    membershipProbe = runner("docker", [
      "exec", primary.name, "/usr/local/bin/celld", "diagnose",
      "--bucket", `s3://${storage.bucket}/${storage.run_prefix}/fleet`,
      "--endpoint", "https://s3gateway1:8334",
      "--region", storage.region,
      "--listen", "127.0.0.1:0",
      "--internal-listen", "127.0.0.1:0",
      ...config.nodes.flatMap((node) => ["--peer", node.advertise]),
    ], { timeout: 120_000 });
    completeAction(config, inventory, action, now());
  }
  const healthy = containersRunning && typeof membershipProbe === "string";
  const result = {
    schema_version: "agentic-sandbox.celld-fleet-diagnosis/v1",
    run_id: config.run_id,
    scope: config.scope,
    host_sha256: config.host_sha256,
    pins: config.pins,
    storage: { product: storage.backend.product, version: storage.backend.version, artifact_sha256: storage.backend.artifact_sha256, topology: storage.backend.topology },
    membership: { expected: 3, running: nodes.filter((node) => node.running).length, reserve: nodes.filter((node) => node.role === "reserve" && node.running).length, probe: healthy ? "passed" : "not_run", probe_sha256: membershipProbe === null ? null : sha256(membershipProbe) },
    listeners: { public: "published on host loopback only", internal: `unpublished on ${config.network.name}` },
    instrumentation: config.instrumentation,
    nodes,
    status: healthy ? "READY" : "NOT_READY",
  };
  if (mutateInventory) {
    inventory.state = healthy ? "ready" : "not_ready";
    inventory.diagnosis_sha256 = sha256(JSON.stringify(result));
    persistInventory(config, inventory, now());
  }
  return result;
}

export async function probeFleetWorker(configPath, {
  runner = defaultRunner,
  fetcher = fetch,
  now = () => new Date(),
  nonceFactory = () => randomBytes(16).toString("hex"),
} = {}) {
  const { config, inventory } = loadFixture(configPath);
  assertWorkerVarsReady(config, inventory);
  const primary = config.nodes[0];
  const document = inspectContainer(runner, primary.name);
  if (!document) throw new Error("Worker probe requires the primary fleet node");
  assertOwnedContainer(document, config, primary.name);
  if (document[0]?.State?.Running !== true) throw new Error("Worker probe requires a running primary fleet node");
  const port = runner("docker", ["port", primary.name, "8080/tcp"]);
  const match = /^127\.0\.0\.1:(\d+)$/.exec(port.trim());
  if (!match) throw new Error("Worker probe endpoint must be host-loopback only");
  const endpoint = `http://127.0.0.1:${match[1]}`;
  const instanceId = `qualification-${sha256(config.run_id).slice(0, 24)}`;
  const path = `/instance-cells/${instanceId}`;
  const operationId = `probe-${sha256(`${config.run_id}:${primary.name}`).slice(0, 24)}`;
  const generation = 1;
  const timestamp = now().toISOString();
  const nonce = nonceFactory();
  if (!/^[a-f0-9]{32}$/.test(nonce)) throw new Error("Worker probe nonce factory returned an invalid nonce");
  const { keyId, key } = readWorkerVars(config);
  const digest = sha256("");
  const canonical = ["GET", path, operationId, String(generation), timestamp, nonce, digest].join("\n");
  const signature = createHmac("sha256", key).update(canonical).digest("hex");
  const headers = {
    "x-agentic-key-id": keyId,
    "x-agentic-timestamp": timestamp,
    "x-agentic-nonce": nonce,
    "x-agentic-generation": String(generation),
    "x-agentic-operation-id": operationId,
    "x-agentic-body-sha256": digest,
    "x-agentic-signature": signature,
  };
  const action = planAction(config, inventory, { kind: "worker_public_auth_probe", target: primary.name }, now());
  const forged = await workerProbeRequest(fetcher, endpoint, path, { ...headers, "x-agentic-signature": "0".repeat(64) });
  const valid = await workerProbeRequest(fetcher, endpoint, path, headers);
  const replay = await workerProbeRequest(fetcher, endpoint, path, headers);
  if (forged.status !== 401 || forged.body?.error?.code !== "cell.signature_invalid" || valid.status !== 404 || valid.body?.error?.code !== "cell.missing" || replay.status !== 409 || replay.body?.error?.code !== "cell.signature_replayed") {
    throw new Error("deployed Worker auth probe did not return the exact expected outcomes");
  }
  completeAction(config, inventory, action, now());
  const result = {
    schema_version: "agentic-sandbox.celld-worker-probe/v1",
    run_id: config.run_id,
    scope: config.scope,
    node: primary.name,
    worker_digest: config.pins.worker_digest,
    checks: {
      signed_missing_cell: { status: valid.status, code: valid.body.error.code },
      forged_signature: { status: forged.status, code: forged.body.error.code },
      nonce_replay: { status: replay.status, code: replay.body.error.code },
    },
    status: "READY",
  };
  inventory.worker_probe_sha256 = sha256(JSON.stringify(result));
  persistInventory(config, inventory, now());
  return result;
}

export function cleanupFleet(configPath, { runner = defaultRunner, now = () => new Date(), removeState = true } = {}) {
  const { config, inventory } = loadFixture(configPath);
  const residue = [];
  const inventoriedContainers = inventory.resources.filter((resource) => resource.type === "docker_container").map((resource) => resource.id);
  const containerNames = [...new Set([...config.nodes.map((node) => node.name), ...inventoriedContainers])].reverse();
  for (const name of containerNames) {
    const document = inspectContainer(runner, name);
    if (document) {
      assertOwnedContainer(document, config, name);
      const action = planAction(config, inventory, { kind: "docker_remove", target: name }, now());
      runner("docker", ["rm", "--force", "--volumes", name], { timeout: 120_000 });
      completeAction(config, inventory, action, now());
    }
    const resource = inventory.resources.find((candidate) => candidate.type === "docker_container" && candidate.id === name);
    if (resource) markResource(config, inventory, "docker_container", name, "removed", now());
  }
  const filters = Object.entries(exactLabels(config.run_id)).flatMap(([key, value]) => ["--filter", `label=${key}=${value}`]);
  const remaining = runner("docker", ["ps", "--all", ...filters, "--format", "{{.Names}}"]).split(/\r?\n/).filter(Boolean);
  residue.push(...remaining);
  if (removeState) {
    for (const node of config.nodes) {
      if (existsSync(node.state_dir)) rmSync(node.state_dir, { recursive: true, force: false });
      const resource = inventory.resources.find((candidate) => candidate.type === "directory" && candidate.id === node.state_dir);
      if (resource) markResource(config, inventory, "directory", node.state_dir, "removed", now());
    }
    const workerVars = inventory.resources.find((resource) => resource.type === "protected_file" && resource.id === config.worker_vars_file_ref);
    if (existsSync(config.worker_vars_file_ref)) {
      const metadata = lstatSync(config.worker_vars_file_ref);
      if (!metadata.isFile() || metadata.isSymbolicLink()) throw new Error("refusing unsafe Worker vars cleanup target");
      rmSync(config.worker_vars_file_ref, { force: false });
    }
    if (workerVars) markResource(config, inventory, "protected_file", config.worker_vars_file_ref, "removed", now());
    const fleetRoot = join(config.run_root, "fleet");
    if (existsSync(fleetRoot)) rmdirSync(fleetRoot);
    const rootResource = inventory.resources.find((candidate) => candidate.type === "directory" && candidate.id === fleetRoot);
    if (rootResource) markResource(config, inventory, "directory", fleetRoot, "removed", now());
  }
  for (const resource of inventory.resources) {
    if (resource.status !== "removed" && resource.type !== "docker_container") residue.push(resource.id);
  }
  inventory.state = residue.length ? "cleanup_residue" : "clean";
  persistInventory(config, inventory, now());
  if (residue.length) throw new CleanupResidueError(`fleet cleanup residue: ${residue.join(",")}`);
  return { status: "PASS", run_id: config.run_id, scope: config.scope, removed_containers: config.nodes.length, residue: [] };
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
    const config = prepareFleet({ storageConfigPath: resolve(argument(args, "--storage-config") ?? ""), outputPath: argument(args, "--output") });
    console.log(JSON.stringify({ status: "PASS", config_path: join(config.run_root, "fleet.json"), run_id: config.run_id, scope: config.scope }));
    return 0;
  }
  if (command === "start") {
    const result = startFleet(resolve(argument(args, "--config") ?? ""));
    console.log(JSON.stringify(result));
    return result.status === "READY" ? 0 : 3;
  }
  if (command === "deploy") {
    console.log(JSON.stringify(await deployFleetWorker(resolve(argument(args, "--config") ?? ""))));
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
  throw new Error("usage: celld-fleet-fixture.mjs <prepare|deploy|start|diagnose|probe-worker|cleanup|janitor-preview|janitor-reap> [options]");
}

if (process.argv[1] && SCRIPT_PATH === resolve(process.argv[1])) {
  main(process.argv.slice(2)).then((code) => { process.exitCode = code; }).catch((error) => {
    console.error(JSON.stringify({ status: "ERROR", reason_code: error instanceof CleanupResidueError ? "CELLD_FLEET_CLEANUP_RESIDUE" : "CELLD_FLEET_FIXTURE_ERROR", error_sha256: sha256(error.message) }));
    process.exitCode = error.exitCode ?? 3;
  });
}
