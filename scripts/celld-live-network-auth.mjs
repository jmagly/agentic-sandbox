#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash, createHmac, randomBytes, randomUUID } from "node:crypto";
import { chmodSync, existsSync, lstatSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { request as httpsRequest } from "node:https";
import { connect, createServer as createNetServer, isIP } from "node:net";
import { hostname } from "node:os";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";
import { connect as tlsConnect } from "node:tls";
import { fileURLToPath } from "node:url";

import { cleanupFleet, deployFleetWorker, diagnoseFleet, prepareFleet, startCallbackRelays, startFleet } from "./celld-fleet-fixture.mjs";
import { cleanupFixture, prepareFixture, startFixture } from "./celld-seaweedfs-fixture.mjs";
import { annotateDriverError, driverErrorDocument, driverOperationError, emitDriverError as emitLiveDriverError, withDriverOperation } from "./celld-live-driver-error.mjs";
import { openStorageGatewayAccess } from "./celld-storage-gateway-access.mjs";
import { S3V1Client, STORAGE_PROFILE_SCHEMA } from "./celld-storage-qualifier.mjs";
import { launchManagement, stopManagementAndWait, storageGateway, validateOrchestrationConfig, waitManagement } from "./celld-live-orchestration.mjs";
import { validateLiveProfile } from "./celld-uat-live-protocol.mjs";
export { driverErrorDocument };

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
const DRIVER_ID = "celld-live-network-auth";
const DRIVER_VERSION = "celld-live-network-auth/v1";
const OBSERVATION_SCHEMA = "agentic-sandbox.celld-live-observation/v1";
const NETWORK_AUTH_INVENTORY_SCHEMA = "agentic-sandbox.celld-network-auth-inventory/v1";
const NETWORK_AUTH_OWNER = Object.freeze({ repository: "roctinam/agentic-sandbox", workflow: "celld-qualification.yml" });
const PROBE_CONCURRENCY = 32;
const MTLS_PROXY_PORT = 8443;
const SCENARIOS = new Set(["UAT-CELLD-010", "UAT-CELLD-012"]);
const NODE_PROBE_IMAGE = "docker.io/library/node:20@sha256:8f693eaa7e0a8e71560c9a82b55fd54c2ae920a2ba5d2cde28bac7d1c01c9ba5";
const DENIAL_CLASSES = ["forged_body", "forged_mac", "stale_timestamp", "nonce_replay", "wrong_key", "zero_generation", "wrong_generation", "public_route", "cross_fleet_request"];
const PARTITION_BOUNDARIES = new Map([
  ["management_to_celld", "celld_management"],
  ["celld_to_management", "celld_management"],
  ["celld_to_store", "celld_store"],
  ["node_to_peer", "node_peer"],
]);
const RUN_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const CONTAINER_NAME = /^celld-[a-z0-9-]{1,80}$/;
const SHA256 = /^[0-9a-f]{64}$/;

function sha256(value) { return createHash("sha256").update(value).digest("hex"); }

function emitDriverError(error) {
  emitLiveDriverError("CELLD_NETWORK_AUTH_DRIVER_ERROR", error);
}

function validTimestamp(value) {
  return typeof value === "string" && Number.isFinite(Date.parse(value));
}

function validIpAddress(value) {
  return typeof value === "string" && value.length <= 45 && isIP(value) !== 0;
}

export async function mapBounded(items, limit, mapper, statistics = {}) {
  if (!Array.isArray(items) || !Number.isSafeInteger(limit) || limit < 1 || limit > PROBE_CONCURRENCY || typeof mapper !== "function") {
    throw new Error(`bounded probe pool requires an array, mapper, and concurrency from 1 through ${PROBE_CONCURRENCY}`);
  }
  const results = new Array(items.length);
  let cursor = 0;
  let inFlight = 0;
  let firstError = null;
  statistics.max_in_flight = Number.isSafeInteger(statistics.max_in_flight) ? statistics.max_in_flight : 0;
  async function worker() {
    while (!firstError) {
      const index = cursor;
      cursor += 1;
      if (index >= items.length) return;
      inFlight += 1;
      statistics.max_in_flight = Math.max(statistics.max_in_flight, inFlight);
      try {
        results[index] = await mapper(items[index], index);
      } catch (error) {
        firstError ??= error;
      } finally {
        inFlight -= 1;
      }
    }
  }
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, () => worker()));
  if (firstError) throw firstError;
  return results;
}

export function createNetworkAuthInventory({ runId, runRoot, host = hostname(), now = new Date() }) {
  const root = resolve(runRoot ?? "");
  if (!RUN_ID.test(runId ?? "") || !isAbsolute(runRoot ?? "") || !root.startsWith("/dev/shm/") || !root.split("/").includes(runId)) {
    throw new Error("network inventory requires an exact-run /dev/shm root");
  }
  const timestamp = now.toISOString();
  const inventory = {
    schema_version: NETWORK_AUTH_INVENTORY_SCHEMA,
    run_id: runId,
    run_root: root,
    owner: { ...NETWORK_AUTH_OWNER, run_id: runId },
    host_sha256: sha256(host),
    created_at: timestamp,
    updated_at: timestamp,
    state: "prepared",
    namespaces: [],
    guards: [],
    proxies: [],
    faults: [],
  };
  const errors = validateNetworkAuthInventory(inventory, { runId, runRoot: root, hostSha256: inventory.host_sha256 });
  if (errors.length) throw new Error(errors.join("; "));
  return inventory;
}

export function registerNetworkNamespace(inventory, { container, pid, inode, runLabel }, now = new Date()) {
  if (inventory.state !== "prepared" && inventory.state !== "active") throw new Error("network namespace registration requires a prepared inventory");
  if (!CONTAINER_NAME.test(container ?? "") || !Number.isSafeInteger(pid) || pid < 1 || !Number.isSafeInteger(inode) || inode < 1 || runLabel !== inventory.run_id) {
    throw new Error("network namespace identity is not exact-run owned");
  }
  if (inventory.namespaces.some((entry) => entry.container === container || entry.inode === inode)) throw new Error("network namespace identity is duplicated");
  inventory.namespaces.push({ container, pid, inode, run_label: runLabel });
  inventory.updated_at = now.toISOString();
  return inventory.namespaces.at(-1);
}

export function observeFleetNetworkNamespaces(inventory, fleet, { runner = run, namespaceInode = (pid) => lstatSync(`/proc/${pid}/ns/net`).ino, now = new Date() } = {}) {
  if (fleet?.run_id !== inventory.run_id || !Array.isArray(fleet?.nodes) || typeof fleet?.network?.name !== "string") throw new Error("fleet namespace observation is not run bound");
  const observed = [];
  for (const node of fleet.nodes) {
    let document;
    try { document = JSON.parse(runner("docker", ["container", "inspect", node.name], { timeout: 30_000 })); } catch { throw new Error("fleet namespace inspection is not bounded JSON"); }
    const value = document?.[0];
    const pid = value?.State?.Pid;
    const labels = value?.Config?.Labels;
    const address = value?.NetworkSettings?.Networks?.[fleet.network.name]?.IPAddress;
    if (value?.State?.Running !== true || labels?.["dev.agentic-sandbox.run"] !== inventory.run_id || labels?.["dev.agentic-sandbox.scope"] !== "celld-qualification" || !validIpAddress(address)) {
      throw new Error("fleet namespace inspection is not exact-run owned");
    }
    const inode = namespaceInode(pid);
    registerNetworkNamespace(inventory, { container: node.name, pid, inode, runLabel: inventory.run_id }, now);
    observed.push({ container: node.name, pid, inode, address });
  }
  return observed;
}

export function planDirectionalPartition(inventory, { direction, sourceContainer, sourceNamespaceInode, destinationAddress, destinationPort, faultId = randomBytes(16).toString("hex") }, now = new Date()) {
  const boundary = PARTITION_BOUNDARIES.get(direction);
  const namespace = inventory.namespaces.find((entry) => entry.container === sourceContainer && entry.inode === sourceNamespaceInode && entry.run_label === inventory.run_id);
  if (!boundary || !namespace || !validIpAddress(destinationAddress) || !Number.isSafeInteger(destinationPort) || destinationPort < 1 || destinationPort > 65535 || !/^[0-9a-f]{32}$/.test(faultId)) {
    throw new Error("directional partition target is not exact-run inventory bound");
  }
  if (inventory.faults.some((entry) => entry.id === faultId)) throw new Error("directional partition fault identity is duplicated");
  const timestamp = now.toISOString();
  const fault = {
    id: faultId,
    boundary,
    direction,
    source_container: sourceContainer,
    source_namespace_inode: sourceNamespaceInode,
    destination_address: destinationAddress,
    destination_port: destinationPort,
    nft_family: "inet",
    nft_table: `as_celld_${faultId.slice(0, 16)}`,
    nft_chain: `p_${faultId.slice(0, 16)}`,
    nft_comment: `agentic-sandbox:celld-network:${inventory.run_id}:${faultId}`,
    status: "planned",
    planned_at: timestamp,
    updated_at: timestamp,
  };
  inventory.faults.push(fault);
  inventory.state = "active";
  inventory.updated_at = timestamp;
  return fault;
}

export function planListenerGuard(inventory, { sourceContainer, sourceNamespaceInode, sameFleetAddresses }, now = new Date()) {
  const namespace = inventory.namespaces.find((entry) => entry.container === sourceContainer && entry.inode === sourceNamespaceInode && entry.run_label === inventory.run_id);
  const addresses = Array.isArray(sameFleetAddresses) ? [...sameFleetAddresses].sort() : [];
  if (!namespace || addresses.length !== inventory.namespaces.length || new Set(addresses).size !== addresses.length || addresses.some((address) => !validIpAddress(address) || !/^(?:10\.|172\.|192\.168\.)/.test(address)) || inventory.guards.some((guard) => guard.source_container === sourceContainer)) {
    throw new Error("listener guard target is not exact-fleet inventory bound");
  }
  const id = sha256(`${inventory.run_id}\n${sourceContainer}\nlistener-guard`).slice(0, 32);
  const timestamp = now.toISOString();
  const guard = {
    id,
    source_container: sourceContainer,
    source_namespace_inode: sourceNamespaceInode,
    protected_port: 8081,
    same_fleet_addresses: addresses,
    nft_family: "inet",
    nft_table: `as_celld_g_${id.slice(0, 12)}`,
    nft_chain: `in_${id.slice(0, 12)}`,
    nft_comment: `agentic-sandbox:celld-listener:${inventory.run_id}:${id}`,
    status: "planned",
    planned_at: timestamp,
    updated_at: timestamp,
  };
  inventory.guards.push(guard);
  inventory.state = "active";
  inventory.updated_at = timestamp;
  return guard;
}

export function planMtlsProxy(inventory, { nodeContainer, listenAddress, binarySha256, imageRef }, now = new Date()) {
  const namespace = inventory.namespaces.find((entry) => entry.container === nodeContainer && entry.run_label === inventory.run_id);
  const name = `${nodeContainer}-mtls-proxy`;
  if (!namespace || !CONTAINER_NAME.test(name) || !validIpAddress(listenAddress) || !SHA256.test(binarySha256 ?? "") || !/^sha256:[0-9a-f]{64}$/.test(imageRef ?? "") || inventory.proxies.some((entry) => entry.name === name || entry.node_container === nodeContainer)) {
    throw new Error("mTLS proxy target is not exact-run inventory bound");
  }
  const timestamp = now.toISOString();
  const tlsRoot = join(inventory.run_root, "network-tls");
  const proxy = {
    name,
    node_container: nodeContainer,
    listen_address: listenAddress,
    listen_port: MTLS_PROXY_PORT,
    target_address: "127.0.0.1",
    target_port: 8081,
    binary_sha256: binarySha256,
    image_ref: imageRef,
    ca_file_ref: join(inventory.run_root, "tls/ca.crt"),
    server_cert_file_ref: join(tlsRoot, `${nodeContainer}-server.crt`),
    server_key_file_ref: join(tlsRoot, `${nodeContainer}-server.key`),
    management_client_cert_file_ref: join(tlsRoot, "management-client.crt"),
    management_client_identity_file_ref: join(tlsRoot, "management-client.pem"),
    status: "planned",
    planned_at: timestamp,
    updated_at: timestamp,
  };
  inventory.proxies.push(proxy);
  inventory.state = "active";
  inventory.updated_at = timestamp;
  return proxy;
}

export function validateNetworkAuthInventory(inventory, { runId, runRoot, hostSha256 } = {}) {
  const errors = [];
  if (!inventory || typeof inventory !== "object" || Array.isArray(inventory)) return ["network inventory must be an object"];
  const allowed = new Set(["schema_version", "run_id", "run_root", "owner", "host_sha256", "created_at", "updated_at", "state", "namespaces", "guards", "proxies", "faults"]);
  for (const key of Object.keys(inventory)) if (!allowed.has(key)) errors.push(`inventory.${key} is not allowed`);
  if (inventory.schema_version !== NETWORK_AUTH_INVENTORY_SCHEMA) errors.push("network inventory schema is invalid");
  if (!RUN_ID.test(inventory.run_id ?? "") || (runId && inventory.run_id !== runId)) errors.push("network inventory run ID is invalid");
  if (!isAbsolute(inventory.run_root ?? "") || !resolve(inventory.run_root).startsWith("/dev/shm/") || !resolve(inventory.run_root).split("/").includes(inventory.run_id) || (runRoot && resolve(inventory.run_root) !== resolve(runRoot))) errors.push("network inventory root is invalid");
  if (inventory.owner?.repository !== NETWORK_AUTH_OWNER.repository || inventory.owner?.workflow !== NETWORK_AUTH_OWNER.workflow || inventory.owner?.run_id !== inventory.run_id) errors.push("network inventory owner is invalid");
  if (!SHA256.test(inventory.host_sha256 ?? "") || (hostSha256 && inventory.host_sha256 !== hostSha256)) errors.push("network inventory host is invalid");
  if (!validTimestamp(inventory.created_at) || !validTimestamp(inventory.updated_at)) errors.push("network inventory timestamps are invalid");
  if (!["prepared", "active", "cleanup_residue", "clean"].includes(inventory.state)) errors.push("network inventory state is invalid");
  if (!Array.isArray(inventory.namespaces) || !Array.isArray(inventory.guards) || !Array.isArray(inventory.proxies) || !Array.isArray(inventory.faults)) return [...errors, "network inventory namespaces/guards/proxies/faults must be arrays"];
  const containers = new Set(), inodes = new Set();
  for (const [index, namespace] of inventory.namespaces.entries()) {
    if (!CONTAINER_NAME.test(namespace?.container ?? "") || !Number.isSafeInteger(namespace?.pid) || namespace.pid < 1 || !Number.isSafeInteger(namespace?.inode) || namespace.inode < 1 || namespace?.run_label !== inventory.run_id || containers.has(namespace.container) || inodes.has(namespace.inode)) errors.push(`network inventory namespace is invalid at index ${index}`);
    containers.add(namespace?.container); inodes.add(namespace?.inode);
  }
  const guardIds = new Set(), guardContainers = new Set();
  for (const [index, guard] of inventory.guards.entries()) {
    const namespace = inventory.namespaces.find((entry) => entry.container === guard?.source_container && entry.inode === guard?.source_namespace_inode && entry.run_label === inventory.run_id);
    const expectedId = sha256(`${inventory.run_id}\n${guard?.source_container}\nlistener-guard`).slice(0, 32);
    const addresses = guard?.same_fleet_addresses;
    if (!namespace || guard?.id !== expectedId || guardIds.has(guard?.id) || guardContainers.has(guard?.source_container) || guard?.protected_port !== 8081 || !Array.isArray(addresses) || addresses.length !== inventory.namespaces.length || new Set(addresses).size !== addresses.length || JSON.stringify(addresses) !== JSON.stringify([...addresses].sort()) || addresses.some((address) => !validIpAddress(address) || !/^(?:10\.|172\.|192\.168\.)/.test(address)) || guard?.nft_family !== "inet" || guard?.nft_table !== `as_celld_g_${expectedId.slice(0, 12)}` || guard?.nft_chain !== `in_${expectedId.slice(0, 12)}` || guard?.nft_comment !== `agentic-sandbox:celld-listener:${inventory.run_id}:${expectedId}` || !["planned", "applied", "removed"].includes(guard?.status) || !validTimestamp(guard?.planned_at) || !validTimestamp(guard?.updated_at) || (guard?.status === "applied" && !validTimestamp(guard?.applied_at)) || (guard?.applied_at !== undefined && !validTimestamp(guard.applied_at)) || (guard?.status === "removed" && !validTimestamp(guard?.removed_at))) errors.push(`network inventory listener guard is invalid at index ${index}`);
    guardIds.add(guard?.id); guardContainers.add(guard?.source_container);
  }
  const proxyNames = new Set(), proxyNodes = new Set();
  for (const [index, proxy] of inventory.proxies.entries()) {
    const namespace = inventory.namespaces.find((entry) => entry.container === proxy?.node_container && entry.run_label === inventory.run_id);
    const tlsRoot = join(inventory.run_root, "network-tls");
    if (!namespace || proxy?.name !== `${proxy.node_container}-mtls-proxy` || !CONTAINER_NAME.test(proxy.name) || proxyNames.has(proxy.name) || proxyNodes.has(proxy.node_container) || !validIpAddress(proxy?.listen_address) || proxy?.listen_port !== MTLS_PROXY_PORT || proxy?.target_address !== "127.0.0.1" || proxy?.target_port !== 8081 || !SHA256.test(proxy?.binary_sha256 ?? "") || !/^sha256:[0-9a-f]{64}$/.test(proxy?.image_ref ?? "") || proxy?.ca_file_ref !== join(inventory.run_root, "tls/ca.crt") || proxy?.server_cert_file_ref !== join(tlsRoot, `${proxy.node_container}-server.crt`) || proxy?.server_key_file_ref !== join(tlsRoot, `${proxy.node_container}-server.key`) || proxy?.management_client_cert_file_ref !== join(tlsRoot, "management-client.crt") || proxy?.management_client_identity_file_ref !== join(tlsRoot, "management-client.pem") || !["planned", "created", "started", "removed"].includes(proxy?.status) || !validTimestamp(proxy?.planned_at) || !validTimestamp(proxy?.updated_at) || (proxy?.status === "created" && !validTimestamp(proxy?.created_at)) || (proxy?.status === "started" && (!validTimestamp(proxy?.created_at) || !validTimestamp(proxy?.started_at))) || (proxy?.created_at !== undefined && !validTimestamp(proxy.created_at)) || (proxy?.started_at !== undefined && !validTimestamp(proxy.started_at)) || (proxy?.status === "removed" && !validTimestamp(proxy?.removed_at))) errors.push(`network inventory proxy is invalid at index ${index}`);
    proxyNames.add(proxy?.name); proxyNodes.add(proxy?.node_container);
  }
  const faultIds = new Set();
  for (const [index, fault] of inventory.faults.entries()) {
    const namespace = inventory.namespaces.find((entry) => entry.container === fault?.source_container && entry.inode === fault?.source_namespace_inode);
    if (!/^[0-9a-f]{32}$/.test(fault?.id ?? "") || faultIds.has(fault?.id) || PARTITION_BOUNDARIES.get(fault?.direction) !== fault?.boundary || !namespace || !validIpAddress(fault?.destination_address) || !Number.isSafeInteger(fault?.destination_port) || fault.destination_port < 1 || fault.destination_port > 65535 || fault?.nft_family !== "inet" || fault?.nft_table !== `as_celld_${fault.id.slice(0, 16)}` || fault?.nft_chain !== `p_${fault.id.slice(0, 16)}` || fault?.nft_comment !== `agentic-sandbox:celld-network:${inventory.run_id}:${fault.id}` || !["planned", "applied", "healed"].includes(fault?.status) || !validTimestamp(fault?.planned_at) || !validTimestamp(fault?.updated_at) || (fault?.status === "applied" && !validTimestamp(fault?.applied_at)) || (fault?.applied_at !== undefined && !validTimestamp(fault.applied_at)) || (fault?.status === "healed" && !validTimestamp(fault?.healed_at))) errors.push(`network inventory fault is invalid at index ${index}`);
    faultIds.add(fault?.id);
  }
  return errors;
}

export function persistNetworkAuthInventory(inventory) {
  const errors = validateNetworkAuthInventory(inventory);
  if (errors.length) throw new Error(errors.join("; "));
  const path = join(inventory.run_root, "network-auth-inventory.json");
  const temporary = `${path}.tmp-${process.pid}-${randomBytes(8).toString("hex")}`;
  try {
    writeFileSync(temporary, `${JSON.stringify(inventory, null, 2)}\n`, { mode: 0o600, flag: "wx" });
    chmodSync(temporary, 0o600);
    renameSync(temporary, path);
    chmodSync(path, 0o600);
  } finally {
    rmSync(temporary, { force: true });
  }
  return path;
}

function exactLabels(runId) {
  return { "dev.agentic-sandbox.run": runId, "dev.agentic-sandbox.scope": "celld-qualification" };
}

function labelsToArgs(labels) {
  return Object.entries(labels).flatMap(([key, value]) => ["--label", `${key}=${value}`]);
}

export function mtlsProxyCreateArgs(inventory, proxy, { binaryPath, uid = typeof process.getuid === "function" ? process.getuid() : 1000, gid = typeof process.getgid === "function" ? process.getgid() : 1000 } = {}) {
  const errors = validateNetworkAuthInventory(inventory);
  if (errors.length || !inventory.proxies.includes(proxy) || proxy.status !== "planned" || !isAbsolute(binaryPath ?? "") || basename(binaryPath) !== "agentic-celld-mtls-proxy" || !Number.isSafeInteger(uid) || uid < 0 || !Number.isSafeInteger(gid) || gid < 0) {
    throw new Error("mTLS proxy creation target is not an exact planned resource");
  }
  return [
    "create", "--name", proxy.name,
    ...labelsToArgs(exactLabels(inventory.run_id)),
    "--network", `container:${proxy.node_container}`,
    "--user", `${uid}:${gid}`,
    "--read-only", "--security-opt", "no-new-privileges:true", "--cap-drop", "ALL",
    "--pids-limit", "64", "--cpus", "0.25", "--memory", "64m",
    "--mount", `type=bind,src=${binaryPath},dst=/usr/local/bin/agentic-celld-mtls-proxy,readonly`,
    "--mount", `type=bind,src=${proxy.ca_file_ref},dst=/run/tls/ca.crt,readonly`,
    "--mount", `type=bind,src=${proxy.server_cert_file_ref},dst=/run/tls/server.crt,readonly`,
    "--mount", `type=bind,src=${proxy.server_key_file_ref},dst=/run/tls/server.key,readonly`,
    "--mount", `type=bind,src=${proxy.management_client_cert_file_ref},dst=/run/tls/management-client.crt,readonly`,
    "--entrypoint", "/usr/local/bin/agentic-celld-mtls-proxy",
    proxy.image_ref,
    "--listen", `0.0.0.0:${proxy.listen_port}`,
    "--target", `${proxy.target_address}:${proxy.target_port}`,
    "--ca", "/run/tls/ca.crt",
    "--cert", "/run/tls/server.crt",
    "--key", "/run/tls/server.key",
    "--client-cert", "/run/tls/management-client.crt",
  ];
}

function writeProtectedNew(path, value) {
  writeFileSync(path, value, { mode: 0o600, flag: "wx" });
  chmodSync(path, 0o600);
}

function verifyMtlsCertificateSources(inventory) {
  for (const path of [join(inventory.run_root, "tls/ca.crt"), join(inventory.run_root, "tls/ca.key"), join(inventory.run_root, "tls/ca.srl")]) {
    const metadata = lstatSync(path);
    if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) throw new Error("network mTLS CA source is not a protected regular file");
  }
}

export function mtlsNegativeIdentityFiles(inventory) {
  const errors = validateNetworkAuthInventory(inventory);
  if (errors.length) throw new Error(errors.join("; "));
  const tlsRoot = join(inventory.run_root, "network-tls");
  return Object.fromEntries([
    ["wrong_cn", "agentic-celld-wrong-cn", 2],
    ["cross_fleet_certificate", "agentic-celld-cross-fleet", 2],
    ["expired_certificate", "agentic-celld-expired", 0],
  ].map(([role, cn, days]) => [role, {
    role,
    cn,
    days,
    cert_file_ref: join(tlsRoot, `${role}.crt`),
    key_file_ref: join(tlsRoot, `${role}.key`),
    identity_file_ref: join(tlsRoot, `${role}.pem`),
  }]));
}

export function prepareMtlsProxyCertificates(inventory, {
  runner = run,
  persist = persistNetworkAuthInventory,
  rootAvailable = (path) => !existsSync(path),
  createDirectory = (path) => { mkdirSync(path, { mode: 0o700 }); chmodSync(path, 0o700); },
  writeProtected = writeProtectedNew,
  readProtected = readFileSync,
  protect = (path) => chmodSync(path, 0o600),
  verifySources = verifyMtlsCertificateSources,
} = {}) {
  const errors = validateNetworkAuthInventory(inventory);
  if (errors.length || inventory.proxies.length === 0 || inventory.proxies.some((proxy) => proxy.status !== "planned")) throw new Error("network mTLS certificate plan is not an exact prepared inventory");
  const tlsRoot = join(inventory.run_root, "network-tls");
  if (!rootAvailable(tlsRoot)) throw new Error("network mTLS certificate root already exists");
  verifySources(inventory);
  persist(inventory);
  createDirectory(tlsRoot);
  const caCert = join(inventory.run_root, "tls/ca.crt");
  const caKey = join(inventory.run_root, "tls/ca.key");
  const caSerial = join(inventory.run_root, "tls/ca.srl");
  const clientKey = join(tlsRoot, "management-client.key");
  const clientCsr = join(tlsRoot, "management-client.csr");
  const clientCert = join(tlsRoot, "management-client.crt");
  const clientIdentity = join(tlsRoot, "management-client.pem");
  const clientExtensions = join(tlsRoot, "management-client-ext.cnf");
  writeProtected(clientExtensions, "extendedKeyUsage=clientAuth\n");
  runner("openssl", ["genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", clientKey]);
  runner("openssl", ["req", "-new", "-key", clientKey, "-subj", "/CN=agentic-celld-management", "-out", clientCsr]);
  runner("openssl", ["x509", "-req", "-in", clientCsr, "-CA", caCert, "-CAkey", caKey, "-CAserial", caSerial, "-days", "2", "-sha256", "-extfile", clientExtensions, "-out", clientCert]);
  for (const path of [clientKey, clientCsr, clientCert]) protect(path);
  runner("openssl", ["verify", "-purpose", "sslclient", "-CAfile", caCert, clientCert]);
  runner("openssl", ["x509", "-in", clientCert, "-noout", "-checkend", "3600"]);
  writeProtected(clientIdentity, Buffer.concat([Buffer.from(readProtected(clientCert)), Buffer.from(readProtected(clientKey))]));

  const negativeIdentities = mtlsNegativeIdentityFiles(inventory);
  for (const identity of Object.values(negativeIdentities)) {
    const csr = join(tlsRoot, `${identity.role}.csr`);
    const extensions = join(tlsRoot, `${identity.role}-ext.cnf`);
    writeProtected(extensions, "extendedKeyUsage=clientAuth\n");
    runner("openssl", ["genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", identity.key_file_ref]);
    runner("openssl", ["req", "-new", "-key", identity.key_file_ref, "-subj", `/CN=${identity.cn}`, "-out", csr]);
    runner("openssl", ["x509", "-req", "-in", csr, "-CA", caCert, "-CAkey", caKey, "-CAserial", caSerial, "-days", String(identity.days), "-sha256", "-extfile", extensions, "-out", identity.cert_file_ref]);
    for (const path of [identity.key_file_ref, csr, identity.cert_file_ref]) protect(path);
    if (identity.days > 0) {
      runner("openssl", ["verify", "-purpose", "sslclient", "-CAfile", caCert, identity.cert_file_ref]);
      runner("openssl", ["x509", "-in", identity.cert_file_ref, "-noout", "-checkend", "3600"]);
    }
    writeProtected(identity.identity_file_ref, Buffer.concat([Buffer.from(readProtected(identity.cert_file_ref)), Buffer.from(readProtected(identity.key_file_ref))]));
  }

  const servers = [];
  for (const proxy of inventory.proxies) {
    const serverCsr = join(tlsRoot, `${proxy.node_container}-server.csr`);
    const serverExtensions = join(tlsRoot, `${proxy.node_container}-server-ext.cnf`);
    writeProtected(serverExtensions, `subjectAltName=IP:${proxy.listen_address}\nextendedKeyUsage=serverAuth\n`);
    runner("openssl", ["genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", proxy.server_key_file_ref]);
    runner("openssl", ["req", "-new", "-key", proxy.server_key_file_ref, "-subj", "/CN=celld.internal", "-out", serverCsr]);
    runner("openssl", ["x509", "-req", "-in", serverCsr, "-CA", caCert, "-CAkey", caKey, "-CAserial", caSerial, "-days", "2", "-sha256", "-extfile", serverExtensions, "-out", proxy.server_cert_file_ref]);
    for (const path of [proxy.server_key_file_ref, serverCsr, proxy.server_cert_file_ref]) protect(path);
    runner("openssl", ["verify", "-purpose", "sslserver", "-CAfile", caCert, proxy.server_cert_file_ref]);
    runner("openssl", ["x509", "-in", proxy.server_cert_file_ref, "-noout", "-checkend", "3600"]);
    servers.push({ node_container: proxy.node_container, address: proxy.listen_address, cert_file_ref: proxy.server_cert_file_ref });
  }
  return { tls_root: tlsRoot, management_client_identity_file_ref: clientIdentity, negative_client_identities: negativeIdentities, servers };
}

function verifyMtlsProxyMaterial(proxy, binaryPath) {
  const binary = lstatSync(binaryPath);
  if (!binary.isFile() || binary.isSymbolicLink() || (binary.mode & 0o111) === 0 || sha256(readFileSync(binaryPath)) !== proxy.binary_sha256) {
    throw new Error("mTLS proxy executable does not match the planned digest");
  }
  for (const path of [proxy.ca_file_ref, proxy.server_cert_file_ref, proxy.server_key_file_ref, proxy.management_client_cert_file_ref]) {
    const metadata = lstatSync(path);
    if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) throw new Error("mTLS proxy material is not a protected regular file");
  }
}

function assertOwnedMtlsProxy(document, inventory, proxy) {
  const value = document?.[0];
  const labels = value?.Config?.Labels;
  if (labels?.["dev.agentic-sandbox.run"] !== inventory.run_id || labels?.["dev.agentic-sandbox.scope"] !== "celld-qualification" || value?.Name !== `/${proxy.name}` || value?.HostConfig?.NetworkMode !== `container:${proxy.node_container}` || value?.Config?.Image !== proxy.image_ref) {
    throw new Error("refusing a substituted mTLS proxy container");
  }
  return value;
}

export function startMtlsProxy(inventory, proxy, {
  binaryPath,
  executor = rawCommand,
  persist = persistNetworkAuthInventory,
  verifyMaterial = verifyMtlsProxyMaterial,
  uid,
  gid,
  now = () => new Date(),
} = {}) {
  const args = mtlsProxyCreateArgs(inventory, proxy, { binaryPath, uid, gid });
  verifyMaterial(proxy, binaryPath);
  persist(inventory);
  const daemon = executor("docker", ["info", "--format", "{{.ServerVersion}}"], { timeout: 30_000 });
  if (daemon.status !== 0) throw new Error("Docker is unavailable before mTLS proxy mutation");
  const existing = executor("docker", ["container", "inspect", proxy.name], { timeout: 30_000 });
  if (existing.status === 0) throw new Error("refusing to replace an existing mTLS proxy container");
  const created = executor("docker", args, { timeout: 120_000 });
  if (created.status !== 0) throw new Error(`mTLS proxy creation failed: ${sha256(created.stderr ?? "")}`);
  const createdAt = now().toISOString();
  proxy.status = "created";
  proxy.created_at = createdAt;
  proxy.updated_at = createdAt;
  inventory.updated_at = createdAt;
  persist(inventory);
  const started = executor("docker", ["start", proxy.name], { timeout: 120_000 });
  if (started.status !== 0) throw new Error(`mTLS proxy start failed: ${sha256(started.stderr ?? "")}`);
  const inspected = executor("docker", ["container", "inspect", proxy.name], { timeout: 30_000 });
  if (inspected.status !== 0) throw new Error("mTLS proxy disappeared after start");
  let document;
  try { document = JSON.parse(inspected.stdout); } catch { throw new Error("mTLS proxy inspection is not bounded JSON"); }
  const observed = assertOwnedMtlsProxy(document, inventory, proxy);
  if (observed.State?.Running !== true) throw new Error("mTLS proxy did not remain running");
  const startedAt = now().toISOString();
  proxy.status = "started";
  proxy.started_at = startedAt;
  proxy.updated_at = startedAt;
  inventory.updated_at = startedAt;
  persist(inventory);
  return proxy;
}

function probeMtlsProxy(proxy) {
  return new Promise((resolvePromise) => {
    const identity = readFileSync(proxy.management_client_identity_file_ref);
    const socket = tlsConnect({
      host: proxy.listen_address,
      port: proxy.listen_port,
      ca: readFileSync(proxy.ca_file_ref),
      cert: identity,
      key: identity,
      rejectUnauthorized: true,
    });
    let settled = false;
    const finish = (ready) => {
      if (settled) return;
      settled = true;
      socket.destroy();
      resolvePromise(ready);
    };
    socket.setTimeout(2_000, () => finish(false));
    socket.once("secureConnect", () => finish(socket.authorized));
    socket.once("error", () => finish(false));
  });
}

export async function waitMtlsProxies(inventory, { probe = probeMtlsProxy, attempts = 480, intervalMs = 250, delay = (milliseconds) => new Promise((resolvePromise) => setTimeout(resolvePromise, milliseconds)) } = {}) {
  const errors = validateNetworkAuthInventory(inventory);
  if (errors.length || inventory.proxies.length === 0 || inventory.proxies.some((proxy) => proxy.status !== "started") || !Number.isSafeInteger(attempts) || attempts < 1 || attempts > 480 || !Number.isSafeInteger(intervalMs) || intervalMs < 0 || intervalMs > 1_000) {
    throw new Error("mTLS proxy readiness requires a complete exact-run started inventory");
  }
  for (const proxy of inventory.proxies) {
    let ready = false;
    for (let attempt = 0; attempt < attempts; attempt += 1) {
      if (await probe(proxy)) { ready = true; break; }
      if (attempt + 1 < attempts) await delay(intervalMs);
    }
    if (!ready) throw new Error(`mTLS proxy readiness failed: ${sha256(proxy.name)}`);
  }
  return { status: "READY", proxies: inventory.proxies.length };
}

export function cleanupMtlsProxies(inventory, { executor = rawCommand, persist = persistNetworkAuthInventory, now = () => new Date() } = {}) {
  const errors = validateNetworkAuthInventory(inventory);
  if (errors.length) throw new Error(errors.join("; "));
  const targets = [...inventory.proxies].reverse().filter((proxy) => proxy.status !== "removed");
  if (targets.length === 0) return [];
  const daemon = executor("docker", ["info", "--format", "{{.ServerVersion}}"], { timeout: 30_000 });
  if (daemon.status !== 0) throw new Error("Docker is unavailable before mTLS proxy cleanup");
  const removed = [];
  const failures = [];
  for (const proxy of targets) {
    try {
      const inspected = executor("docker", ["container", "inspect", proxy.name], { timeout: 30_000 });
      if (inspected.status === 0) {
        let document;
        try { document = JSON.parse(inspected.stdout); } catch { throw new Error("mTLS proxy cleanup inspection is not bounded JSON"); }
        assertOwnedMtlsProxy(document, inventory, proxy);
        const deletion = executor("docker", ["rm", "--force", "--volumes", proxy.name], { timeout: 120_000 });
        if (deletion.status !== 0) throw new Error(`mTLS proxy cleanup failed: ${sha256(deletion.stderr ?? "")}`);
        const remaining = executor("docker", ["container", "inspect", proxy.name], { timeout: 30_000 });
        if (remaining.status === 0) throw new Error("mTLS proxy remained after exact deletion");
      }
      const timestamp = now().toISOString();
      proxy.status = "removed";
      proxy.removed_at = timestamp;
      proxy.updated_at = timestamp;
      inventory.updated_at = timestamp;
      inventory.state = inventory.proxies.every((entry) => entry.status === "removed") && inventory.guards.every((entry) => entry.status === "removed") && inventory.faults.every((entry) => entry.status === "healed") ? "clean" : "active";
      persist(inventory);
      removed.push(proxy.name);
    } catch (error) {
      failures.push(error);
    }
  }
  if (failures.length) throw new AggregateError(failures, "exact-run mTLS proxy cleanup left residue");
  return removed;
}

function rawCommand(program, args, options = {}) {
  const result = spawnSync(program, args, { encoding: "utf8", shell: false, ...options });
  if (result.error) throw result.error;
  return { status: result.status, signal: result.signal, stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
}

function exactNamespace(inventory, fault, { dockerRunner = run, namespaceInode = (pid) => lstatSync(`/proc/${pid}/ns/net`).ino } = {}) {
  const namespace = inventory.namespaces.find((entry) => entry.container === fault.source_container && entry.inode === fault.source_namespace_inode && entry.run_label === inventory.run_id);
  if (!namespace) throw new Error("partition namespace is absent from the exact-run inventory");
  const fields = dockerRunner("docker", ["inspect", "--format", '{{.State.Pid}}|{{index .Config.Labels "dev.agentic-sandbox.run"}}|{{index .Config.Labels "dev.agentic-sandbox.scope"}}', namespace.container], { timeout: 30_000 }).trim().split("|");
  const observedPid = Number(fields[0]);
  if (!Number.isSafeInteger(observedPid) || observedPid !== namespace.pid || fields[1] !== inventory.run_id || fields[2] !== "celld-qualification" || namespaceInode(observedPid) !== namespace.inode) {
    throw new Error("partition namespace no longer matches the exact-run container identity");
  }
  return namespace;
}

export function listenerGuardCommands(inventory, guard) {
  const errors = validateNetworkAuthInventory(inventory);
  if (errors.length || !inventory.guards.includes(guard) || guard.status === "removed") throw new Error("listener guard command target is not exact-run active inventory");
  const namespace = inventory.namespaces.find((entry) => entry.container === guard.source_container && entry.inode === guard.source_namespace_inode);
  if (!namespace) throw new Error("listener guard namespace is not inventory bound");
  const prefix = ["--target", String(namespace.pid), "--net", "--", "nft"];
  return {
    inspect: [...prefix, "list", "table", guard.nft_family, guard.nft_table],
    apply: [
      [...prefix, "add", "table", guard.nft_family, guard.nft_table],
      [...prefix, "add", "chain", guard.nft_family, guard.nft_table, guard.nft_chain, "{", "type", "filter", "hook", "input", "priority", "-200", ";", "policy", "accept", ";", "}"],
      [...prefix, "add", "rule", guard.nft_family, guard.nft_table, guard.nft_chain, "iifname", "lo", "tcp", "dport", String(guard.protected_port), "counter", "accept", "comment", guard.nft_comment],
      ...guard.same_fleet_addresses.map((address) => [...prefix, "add", "rule", guard.nft_family, guard.nft_table, guard.nft_chain, "ip", "saddr", address, "tcp", "dport", String(guard.protected_port), "counter", "accept", "comment", guard.nft_comment]),
      [...prefix, "add", "rule", guard.nft_family, guard.nft_table, guard.nft_chain, "tcp", "dport", String(guard.protected_port), "counter", "drop", "comment", guard.nft_comment],
    ],
    remove: [...prefix, "delete", "table", guard.nft_family, guard.nft_table],
    verify_absent: [...prefix, "list", "tables"],
  };
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

function listenerGuardApplyStep(index, peerCount) {
  if (index === 0) return "add-table";
  if (index === 1) return "add-chain";
  if (index === 2) return "add-loopback-accept";
  if (index < peerCount + 3) return `add-peer-accept-${index - 3}`;
  if (index === peerCount + 3) return "add-bypass-drop";
  return `apply-${index}`;
}

function executeListenerGuardCommand(executor, operation, args) {
  try {
    return executor("sudo", ["-n", "nsenter", ...args], { timeout: 30_000 });
  } catch (error) {
    throw annotateDriverError(error, {
      operation,
      errorCode: "CELLD_LISTENER_GUARD_COMMAND_UNAVAILABLE",
    });
  }
}

export function applyListenerGuard(inventory, guard, { executor = rawCommand, persist = persistNetworkAuthInventory, dockerRunner = run, namespaceInode, now = new Date() } = {}) {
  if (guard.status !== "planned") throw new Error("only a planned listener guard can be applied");
  try {
    exactNamespace(inventory, guard, { dockerRunner, namespaceInode });
  } catch (error) {
    throw annotateDriverError(error, {
      operation: "network-auth.apply-listener-guard.exact-namespace",
      errorCode: "CELLD_LISTENER_GUARD_NAMESPACE_INVALID",
    });
  }
  try {
    persist(inventory);
  } catch (error) {
    throw annotateDriverError(error, {
      operation: "network-auth.apply-listener-guard.persist-planned",
      errorCode: "CELLD_LISTENER_GUARD_PERSIST_FAILED",
    });
  }
  let commands;
  try {
    commands = listenerGuardCommands(inventory, guard);
  } catch (error) {
    throw annotateDriverError(error, {
      operation: "network-auth.apply-listener-guard.plan-commands",
      errorCode: "CELLD_LISTENER_GUARD_COMMAND_PLAN_INVALID",
    });
  }
  const existing = executeListenerGuardCommand(executor, "network-auth.apply-listener-guard.inspect-existing", commands.inspect);
  if (existing.status === 0) {
    throw driverOperationError("network-auth.apply-listener-guard.inspect-existing", commandFailureFields(existing, "CELLD_LISTENER_GUARD_TABLE_EXISTS"), "refusing to replace an existing listener guard table");
  }
  for (const [index, args] of commands.apply.entries()) {
    const operation = `network-auth.apply-listener-guard.${listenerGuardApplyStep(index, guard.same_fleet_addresses.length)}`;
    const result = executeListenerGuardCommand(executor, operation, args);
    if (result.status !== 0) {
      throw driverOperationError(operation, commandFailureFields(result, "CELLD_LISTENER_GUARD_APPLY_FAILED"), "listener guard apply command failed");
    }
  }
  const timestamp = now.toISOString();
  guard.status = "applied";
  guard.applied_at = timestamp;
  guard.updated_at = timestamp;
  inventory.updated_at = timestamp;
  try {
    persist(inventory);
  } catch (error) {
    throw annotateDriverError(error, {
      operation: "network-auth.apply-listener-guard.persist-applied",
      errorCode: "CELLD_LISTENER_GUARD_PERSIST_FAILED",
    });
  }
  return guard;
}

export function removeListenerGuard(inventory, guard, { executor = rawCommand, persist = persistNetworkAuthInventory, dockerRunner = run, namespaceInode, now = new Date() } = {}) {
  if (!["planned", "applied"].includes(guard.status)) throw new Error("only a planned or applied listener guard can be removed");
  exactNamespace(inventory, guard, { dockerRunner, namespaceInode });
  const commands = listenerGuardCommands(inventory, guard);
  const deletion = executor("sudo", ["-n", "nsenter", ...commands.remove], { timeout: 30_000 });
  const remaining = executor("sudo", ["-n", "nsenter", ...commands.verify_absent], { timeout: 30_000 });
  if (remaining.status !== 0 || new RegExp(`\\b${guard.nft_family}\\s+${guard.nft_table}\\b`).test(remaining.stdout ?? "")) throw new Error(`listener guard removal failed: ${sha256(`${deletion.stderr ?? ""}\n${remaining.stderr ?? ""}`)}`);
  const timestamp = now.toISOString();
  guard.status = "removed";
  guard.removed_at = timestamp;
  guard.updated_at = timestamp;
  inventory.updated_at = timestamp;
  inventory.state = inventory.faults.every((entry) => entry.status === "healed") && inventory.guards.every((entry) => entry.status === "removed") && inventory.proxies.every((entry) => entry.status === "removed") ? "clean" : "active";
  persist(inventory);
  return guard;
}

export function directionalPartitionCommands(inventory, fault) {
  const errors = validateNetworkAuthInventory(inventory);
  if (errors.length || !inventory.faults.includes(fault) || fault.status === "healed") throw new Error("partition command target is not a live exact-run fault");
  const namespace = inventory.namespaces.find((entry) => entry.container === fault.source_container && entry.inode === fault.source_namespace_inode);
  if (!namespace) throw new Error("partition command namespace is not inventory bound");
  const prefix = ["--target", String(namespace.pid), "--net", "--", "nft"];
  const addressFamily = isIP(fault.destination_address) === 6 ? "ip6" : "ip";
  const input = fault.direction === "management_to_celld";
  return {
    inspect: [...prefix, "list", "table", fault.nft_family, fault.nft_table],
    apply: [
      [...prefix, "add", "table", fault.nft_family, fault.nft_table],
      [...prefix, "add", "chain", fault.nft_family, fault.nft_table, fault.nft_chain, "{", "type", "filter", "hook", input ? "input" : "output", "priority", "-150", ";", "policy", "accept", ";", "}"],
      [...prefix, "add", "rule", fault.nft_family, fault.nft_table, fault.nft_chain, addressFamily, "daddr", fault.destination_address, "tcp", "dport", String(fault.destination_port), "counter", "drop", "comment", fault.nft_comment],
    ],
    heal: [...prefix, "delete", "table", fault.nft_family, fault.nft_table],
    verify_absent: [...prefix, "list", "tables"],
  };
}

export function applyDirectionalPartition(inventory, fault, { executor = rawCommand, persist = persistNetworkAuthInventory, dockerRunner = run, namespaceInode, now = new Date() } = {}) {
  if (fault.status !== "planned") throw new Error("only a planned directional partition can be applied");
  exactNamespace(inventory, fault, { dockerRunner, namespaceInode });
  persist(inventory);
  const commands = directionalPartitionCommands(inventory, fault);
  const existing = executor("sudo", ["-n", "nsenter", ...commands.inspect], { timeout: 30_000 });
  if (existing.status === 0) throw new Error("refusing to replace an existing exact-name nftables table");
  for (const args of commands.apply) {
    const result = executor("sudo", ["-n", "nsenter", ...args], { timeout: 30_000 });
    if (result.status !== 0) throw new Error(`directional partition apply failed: ${sha256(result.stderr ?? "")}`);
  }
  const timestamp = now.toISOString();
  fault.status = "applied";
  fault.applied_at = timestamp;
  fault.updated_at = timestamp;
  inventory.updated_at = timestamp;
  persist(inventory);
  return fault;
}

export function healDirectionalPartition(inventory, fault, { executor = rawCommand, persist = persistNetworkAuthInventory, dockerRunner = run, namespaceInode, now = new Date() } = {}) {
  if (!["planned", "applied"].includes(fault.status)) throw new Error("only a planned or applied directional partition can be healed");
  exactNamespace(inventory, fault, { dockerRunner, namespaceInode });
  const commands = directionalPartitionCommands(inventory, fault);
  const deletion = executor("sudo", ["-n", "nsenter", ...commands.heal], { timeout: 30_000 });
  const remaining = executor("sudo", ["-n", "nsenter", ...commands.verify_absent], { timeout: 30_000 });
  if (remaining.status !== 0 || new RegExp(`\\b${fault.nft_family}\\s+${fault.nft_table}\\b`).test(remaining.stdout ?? "")) throw new Error(`directional partition heal failed: ${sha256(`${deletion.stderr ?? ""}\n${remaining.stderr ?? ""}`)}`);
  const timestamp = now.toISOString();
  fault.status = "healed";
  fault.healed_at = timestamp;
  fault.updated_at = timestamp;
  inventory.updated_at = timestamp;
  inventory.state = inventory.faults.every((entry) => entry.status === "healed") && inventory.guards.every((entry) => entry.status === "removed") && inventory.proxies.every((entry) => entry.status === "removed") ? "clean" : "active";
  persist(inventory);
  return fault;
}

export function recoverNetworkAuthInventory(inventory, {
  runId = inventory?.run_id,
  runRoot = inventory?.run_root,
  hostSha256 = inventory?.host_sha256,
  executor = rawCommand,
  persist = persistNetworkAuthInventory,
  dockerRunner = run,
  namespaceInode,
  now = () => new Date(),
} = {}) {
  const errors = validateNetworkAuthInventory(inventory, { runId, runRoot, hostSha256 });
  if (errors.length) throw new Error(errors.join("; "));
  const failures = [];
  for (const fault of [...inventory.faults].reverse()) {
    if (fault.status === "healed") continue;
    try {
      healDirectionalPartition(inventory, fault, {
        executor, persist, dockerRunner, namespaceInode, now: now(),
      });
    } catch (error) {
      failures.push(error);
    }
  }
  for (const guard of [...inventory.guards].reverse()) {
    if (guard.status === "removed") continue;
    try {
      removeListenerGuard(inventory, guard, {
        executor, persist, dockerRunner, namespaceInode, now: now(),
      });
    } catch (error) {
      failures.push(error);
    }
  }
  try {
    cleanupMtlsProxies(inventory, { executor, persist, now });
  } catch (error) {
    failures.push(error);
  }
  inventory.updated_at = now().toISOString();
  inventory.state = failures.length === 0 && inventory.faults.every((fault) => fault.status === "healed") && inventory.guards.every((guard) => guard.status === "removed") && inventory.proxies.every((proxy) => proxy.status === "removed")
    ? "clean"
    : "cleanup_residue";
  persist(inventory);
  if (failures.length) throw new AggregateError(failures, "exact-run network partition cleanup left residue");
  return {
    status: "PASS",
    run_id: inventory.run_id,
    inventory_state: inventory.state,
    healed_fault_ids: inventory.faults.map((fault) => fault.id),
    removed_guard_ids: inventory.guards.map((guard) => guard.id),
  };
}

export function networkAuthInventoryLocations(config) {
  const errors = validateOrchestrationConfig(config);
  if (errors.length) throw new Error(errors.join("; "));
  return [...SCENARIOS].map((scenarioId) => {
    const runRoot = join(config.working_root, `${scenarioId.toLowerCase()}-network`, config.run_id);
    return { scenario_id: scenarioId, run_root: runRoot, inventory_path: join(runRoot, "network-auth-inventory.json") };
  });
}

export function cleanupNetworkAuthInventories(config, {
  exists = existsSync,
  readInventory = (path) => protectedJson(path, "network/auth mutation inventory"),
  host = hostname(),
  executor = rawCommand,
  persist = persistNetworkAuthInventory,
  dockerRunner = run,
  namespaceInode,
  now = () => new Date(),
} = {}) {
  const results = [];
  const failures = [];
  const hostSha256 = sha256(host);
  for (const location of networkAuthInventoryLocations(config)) {
    if (!exists(location.inventory_path)) continue;
    try {
      const inventory = readInventory(location.inventory_path);
      results.push({
        scenario_id: location.scenario_id,
        ...recoverNetworkAuthInventory(inventory, {
          runId: config.run_id,
          runRoot: location.run_root,
          hostSha256,
          executor,
          persist,
          dockerRunner,
          namespaceInode,
          now,
        }),
      });
    } catch (error) {
      failures.push(error);
    }
  }
  if (failures.length) throw new AggregateError(failures, "one or more exact-run network inventories retained residue");
  return { status: "PASS", run_id: config.run_id, inventories: results };
}

export function readManagementProviderCounter(runtime, { runner = run } = {}) {
  const ledgerPath = runtime?.fleet?.callback?.effect_ledger_file_ref;
  if (!isAbsolute(ledgerPath ?? "") || !existsSync(ledgerPath)) throw new Error("management provider counter ledger is unavailable");
  const metadata = lstatSync(ledgerPath);
  if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) throw new Error("management provider counter ledger is not protected");
  const output = runner("sqlite3", [
    "-readonly", "-batch", "-noheader", ledgerPath,
    "PRAGMA query_only=ON; SELECT COALESCE(SUM(provider_dispatch_count),0) FROM celld_effects;",
  ], { timeout: 30_000 });
  if (!/^\d+$/.test(output.trim())) throw new Error("management provider counter query returned an invalid value");
  const value = Number(output.trim());
  if (!Number.isSafeInteger(value) || value < 0) throw new Error("management provider counter exceeds the safe evidence range");
  return value;
}

function providerCounter(runtime) {
  return runtime.readProviderCounter?.() ?? readManagementProviderCounter(runtime);
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

function protectedJson(path, description) {
  if (!isAbsolute(path) || !existsSync(path)) throw new Error(`${description} is missing`);
  const metadata = lstatSync(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) throw new Error(`${description} must be a protected regular non-symlink file`);
  return JSON.parse(readFileSync(path, "utf8"));
}

function canonicalJson(value) {
  if (value === null || typeof value === "boolean" || typeof value === "string") return JSON.stringify(value);
  if (typeof value === "number") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
}

function workerKey(path) {
  const values = new Map(readFileSync(path, "utf8").split(/\r?\n/).filter(Boolean).map((line) => line.split(/=(.*)/s).slice(0, 2)));
  const keyId = values.get("CELL_AUTH_KEY_ID"), key = values.get("CELL_AUTH_KEY");
  if (!keyId || !key || Buffer.byteLength(key) < 32) throw new Error("protected Worker keyring is invalid");
  return { keyId, key };
}

function signedHeaders({ method, path, operationId, generation, body, keyId, key, timestamp = new Date().toISOString(), nonce = randomBytes(16).toString("hex") }) {
  const digest = sha256(body);
  const canonical = [method, path, operationId, String(generation), timestamp, nonce, digest].join("\n");
  return {
    "x-agentic-key-id": keyId, "x-agentic-timestamp": timestamp, "x-agentic-nonce": nonce,
    "x-agentic-generation": String(generation), "x-agentic-operation-id": operationId,
    "x-agentic-body-sha256": digest, "x-agentic-signature": createHmac("sha256", key).update(canonical).digest("hex"),
  };
}

function decodedBoundedResponse(status, bytes, restrictedValues) {
  if (bytes.length > 4096) throw new Error("Worker denial response exceeds 4 KiB");
  const restrictedAbsent = restrictedValues.every((value) => typeof value === "string" && value.length > 0 && !bytes.includes(Buffer.from(value)));
  let value;
  try { value = JSON.parse(bytes.toString("utf8")); } catch { throw new Error("Worker denial response is not JSON"); }
  return { status, code: value?.error?.code ?? null, restricted_absent: restrictedAbsent };
}

function boundedHttpsRequest(url, { method, headers, body, restrictedValues, tls }) {
  if (!(tls?.ca === null || Buffer.isBuffer(tls?.ca)) || !Buffer.isBuffer(tls?.identity) || (tls?.servername !== undefined && typeof tls.servername !== "string")) throw new Error("private Celld HTTPS requires explicit trust and protected client identity bytes");
  return new Promise((resolvePromise, rejectPromise) => {
    const request = httpsRequest(url, {
      method,
      headers,
      ...(tls.ca === null ? {} : { ca: tls.ca }),
      cert: tls.identity,
      key: tls.identity,
      ...(tls.servername === undefined ? {} : { servername: tls.servername }),
      rejectUnauthorized: true,
      agent: false,
      timeout: 10_000,
    }, (response) => {
      const chunks = [];
      let length = 0;
      response.on("data", (chunk) => {
        length += chunk.length;
        if (length > 4096) {
          request.destroy(new Error("Worker denial response exceeds 4 KiB"));
          return;
        }
        chunks.push(chunk);
      });
      response.once("end", () => {
        try { resolvePromise(decodedBoundedResponse(response.statusCode, Buffer.concat(chunks), restrictedValues)); }
        catch (error) { rejectPromise(error); }
      });
    });
    request.once("timeout", () => request.destroy(new Error("private Celld HTTPS request timed out")));
    request.once("error", rejectPromise);
    if (body !== undefined) request.write(body);
    request.end();
  });
}

async function boundedRequest(endpoint, path, { method, headers, body, restrictedValues = [], tls = null }) {
  const url = new URL(path, endpoint);
  if (url.protocol === "https:") return boundedHttpsRequest(url, { method, headers, body, restrictedValues, tls });
  if (url.protocol !== "http:" || tls !== null) throw new Error("Worker request transport is invalid");
  const response = await fetch(url, { method, headers, body, redirect: "error", signal: AbortSignal.timeout(10_000) });
  return decodedBoundedResponse(response.status, Buffer.from(await response.arrayBuffer()), restrictedValues);
}

export function privateCelldRoute(inventory, { readProtected = readFileSync, inspect = lstatSync } = {}) {
  const errors = validateNetworkAuthInventory(inventory);
  const proxy = inventory?.proxies?.[0];
  if (errors.length || !proxy || proxy.status !== "started") throw new Error("private Celld route requires an exact started proxy inventory");
  for (const path of [proxy.ca_file_ref, proxy.management_client_identity_file_ref]) {
    const metadata = inspect(path);
    if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) throw new Error("private Celld route material is not a protected regular file");
  }
  return {
    endpoint: `https://${proxy.listen_address}:${proxy.listen_port}`,
    tls: { ca: Buffer.from(readProtected(proxy.ca_file_ref)), identity: Buffer.from(readProtected(proxy.management_client_identity_file_ref)) },
  };
}

function protectedIdentityBytes(path) {
  const metadata = lstatSync(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) throw new Error("mTLS probe identity is not a protected regular file");
  return Buffer.from(readFileSync(path));
}

export async function probeMtlsTransportNegatives(inventory, {
  requester = boundedRequest,
  readIdentity = protectedIdentityBytes,
  routeProvider = privateCelldRoute,
  now = () => new Date(),
} = {}) {
  const route = routeProvider(inventory);
  const proxy = inventory.proxies[0];
  const identities = mtlsNegativeIdentityFiles(inventory);
  const privateCa = route.tls.ca;
  const cases = [
    { class: "wrong_san", role: "management", tls: { ca: privateCa, identity: route.tls.identity, servername: "wrong.invalid" } },
    { class: "wrong_cn", role: "management", tls: { ca: privateCa, identity: readIdentity(identities.wrong_cn.identity_file_ref) } },
    { class: "public_root", role: "management", tls: { ca: null, identity: route.tls.identity } },
    { class: "expired_certificate", role: "management", tls: { ca: privateCa, identity: readIdentity(identities.expired_certificate.identity_file_ref) } },
    { class: "cross_fleet_certificate", role: "cross_fleet", tls: { ca: privateCa, identity: readIdentity(identities.cross_fleet_certificate.identity_file_ref) } },
  ];
  const attempts = [];
  for (const entry of cases) {
    const startedAt = now().toISOString();
    let responseReceived = false;
    try {
      await requester(route.endpoint, `/qualification-transport/${randomUUID()}`, { method: "GET", headers: {}, tls: entry.tls });
      responseReceived = true;
    } catch {
      // A negative transport case passes only when it cannot reach Celld HTTP.
    }
    const endedAt = now().toISOString();
    attempts.push({ class: entry.class, attempt: 0, role: entry.role, started_at: startedAt, ended_at: endedAt, outcome: responseReceived ? "allowed" : "denied", status: null, code: responseReceived ? "transport.unexpected_http" : "transport.denied" });
  }
  const allowed = attempts.filter((attempt) => attempt.outcome === "allowed");
  if (allowed.length) throw new Error(`mTLS negative matrix reached Celld HTTP: ${allowed.map((attempt) => attempt.class).join(",")}`);
  return attempts;
}

function workerEndpoint(fleet) {
  const output = run("docker", ["port", fleet.nodes[0].name, "8080/tcp"]);
  const match = /^127\.0\.0\.1:(\d+)$/.exec(output);
  if (!match) throw new Error("Worker listener is not published on host loopback only");
  return `http://127.0.0.1:${match[1]}`;
}

function commandBody(operationId, instanceId) {
  const payload = {};
  const command = { operation_id: operationId, instance_id: instanceId, generation: 1, action: "observe", payload };
  return JSON.stringify({ document_type: "instance-cell-command", schema_version: "1", ...command, request_hash: sha256(canonicalJson(command)), issued_at: new Date().toISOString() });
}

async function negativeRequest(route, keyring, kind, attempt) {
  const instanceId = `negative-${kind}`;
  const operationId = `negative-${kind}-${attempt}`;
  const path = `/instance-cells/${instanceId}/commands`;
  const originalBody = commandBody(operationId, instanceId);
  let body = originalBody;
  const attachIdentity = (result) => ({ ...result, operation_id_sha256: sha256(operationId) });
  let headers = signedHeaders({ method: "POST", path, operationId, generation: 1, body, ...keyring });
  if (kind === "forged_body") body = `${originalBody.slice(0, -1)},"tampered":true}`;
  else if (kind === "forged_mac") headers["x-agentic-signature"] = "0".repeat(64);
  else if (kind === "stale_timestamp") {
    const timestamp = new Date(Date.now() - 10 * 60_000).toISOString();
    headers = signedHeaders({ method: "POST", path, operationId, generation: 1, body, timestamp, ...keyring });
  } else if (kind === "wrong_key") headers = signedHeaders({ method: "POST", path, operationId, generation: 1, body, keyId: keyring.keyId, key: randomBytes(32).toString("hex") });
  else if (kind === "zero_generation") headers = signedHeaders({ method: "POST", path, operationId, generation: 0, body, ...keyring });
  else if (kind === "wrong_generation") headers["x-agentic-generation"] = "2";
  if (kind === "nonce_replay") {
    const getPath = `/instance-cells/replay-${attempt}`;
    const getHeaders = signedHeaders({ method: "GET", path: getPath, operationId, generation: 1, body: "", ...keyring });
    const first = await boundedRequest(route.endpoint, getPath, { method: "GET", headers: getHeaders, tls: route.tls });
    if (first.status !== 404 || first.code !== "cell.missing") throw new Error("nonce replay setup did not authenticate without a provider effect");
    return attachIdentity(await boundedRequest(route.endpoint, getPath, { method: "GET", headers: getHeaders, tls: route.tls }));
  }
  return attachIdentity(await boundedRequest(route.endpoint, path, { method: "POST", headers: { "content-type": "application/json", ...headers }, body, tls: route.tls }));
}

function tcpConnected(host, port) {
  return new Promise((resolvePromise) => {
    const socket = connect({ host, port });
    const timer = setTimeout(() => { socket.destroy(); resolvePromise(false); }, 500);
    socket.once("connect", () => { clearTimeout(timer); socket.destroy(); resolvePromise(true); });
    socket.once("error", () => { clearTimeout(timer); resolvePromise(false); });
  });
}

export async function probeProxyBypass(address, { probe = tcpConnected, now = () => new Date() } = {}) {
  if (!validIpAddress(address) || !/^(?:10\.|172\.|192\.168\.)/.test(address)) throw new Error("proxy bypass probe target is not a private node address");
  const startedAt = now().toISOString();
  const connected = await probe(address, 8081);
  const attempt = { class: "proxy_bypass", attempt: 0, role: "management", started_at: startedAt, ended_at: now().toISOString(), outcome: connected ? "allowed" : "denied", status: null, code: connected ? "network.unexpected_bypass" : "network.no_bypass" };
  if (connected) throw new Error("Celld plaintext listener bypass is reachable");
  return attempt;
}

function openEnvironmentProxyTrap() {
  return new Promise((resolvePromise, rejectPromise) => {
    let connections = 0;
    const server = createNetServer((socket) => { connections += 1; socket.destroy(); });
    server.once("error", rejectPromise);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (!address || typeof address === "string") { server.close(); rejectPromise(new Error("environment proxy trap did not bind a loopback port")); return; }
      resolvePromise({
        url: `http://127.0.0.1:${address.port}`,
        connections: () => connections,
        close: () => new Promise((resolveClose, rejectClose) => server.close((error) => error ? rejectClose(error) : resolveClose())),
      });
    });
  });
}

export async function probeEnvironmentProxy(route, keyring, {
  requester = boundedRequest,
  openTrap = openEnvironmentProxyTrap,
  environment = process.env,
  now = () => new Date(),
} = {}) {
  if (typeof route?.endpoint !== "string" || !route.endpoint.startsWith("https://") || !Buffer.isBuffer(route?.tls?.ca) || !Buffer.isBuffer(route?.tls?.identity) || typeof keyring?.keyId !== "string" || typeof keyring?.key !== "string") throw new Error("environment proxy probe requires the exact private route and request keyring");
  const trap = await openTrap();
  if (!/^http:\/\/127\.0\.0\.1:\d+$/.test(trap?.url ?? "") || typeof trap?.connections !== "function" || typeof trap?.close !== "function") throw new Error("environment proxy trap identity is invalid");
  const names = ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy"];
  const previous = new Map(names.map((name) => [name, environment[name]]));
  const startedAt = now().toISOString();
  let response;
  let operationId;
  let primaryError = null;
  try {
    for (const name of names.slice(0, 6)) environment[name] = trap.url;
    environment.NO_PROXY = "";
    environment.no_proxy = "";
    operationId = `environment-proxy-${randomBytes(12).toString("hex")}`;
    const path = `/instance-cells/environment-proxy-${randomUUID()}`;
    response = await requester(route.endpoint, path, { method: "GET", headers: signedHeaders({ method: "GET", path, operationId, generation: 1, body: "", ...keyring }), tls: route.tls });
    if (response.status !== 404 || response.code !== "cell.missing" || trap.connections() !== 0) throw new Error("environment proxy intercepted or altered the private Celld route");
  } catch (error) {
    primaryError = error;
  } finally {
    for (const [name, value] of previous) {
      if (value === undefined) delete environment[name];
      else environment[name] = value;
    }
    try { await trap.close(); } catch (error) { primaryError ??= error; }
  }
  if (primaryError) throw primaryError;
  return { class: "environment_proxy", attempt: 0, role: "management", started_at: startedAt, ended_at: now().toISOString(), outcome: "denied", status: response.status, code: "environment_proxy.ignored", operation_id_sha256: sha256(operationId) };
}

const PROBE_ROLES = Object.freeze(["isolation", "public", "cross-fleet"]);

function exactProbeIdentity(runId, role = "isolation") {
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(runId)) throw new Error("probe cleanup run identity is invalid");
  if (!PROBE_ROLES.includes(role)) throw new Error("probe role is invalid");
  const digestInput = role === "isolation" ? runId : `${runId}\n${role}`;
  const network = `celld-probe-${sha256(digestInput).slice(0, 16)}`;
  return { role, network, container: `${network}-client`, labels: { "dev.agentic-sandbox.run": runId, "dev.agentic-sandbox.scope": "celld-qualification", "dev.agentic-sandbox.probe-role": role } };
}

function inspectDocker(runner, kind, name) {
  try { return JSON.parse(runner("docker", [kind, "inspect", name])); } catch { return null; }
}

function assertProbeLabels(document, expected, name, network = false) {
  const labels = network ? document?.[0]?.Labels : document?.[0]?.Config?.Labels;
  for (const [key, value] of Object.entries(expected)) if (labels?.[key] !== value) throw new Error(`refusing unowned probe resource ${name}`);
}

export function cleanupProbeResources(runId, { runner = run, roles = PROBE_ROLES } = {}) {
  if (!Array.isArray(roles) || roles.length === 0 || new Set(roles).size !== roles.length || roles.some((role) => !PROBE_ROLES.includes(role))) throw new Error("probe cleanup roles are invalid");
  const removed = [];
  runner("docker", ["info", "--format", "{{.ServerVersion}}"], { timeout: 30_000 });
  for (const role of roles) {
    const identity = exactProbeIdentity(runId, role);
    const container = inspectDocker(runner, "container", identity.container);
    if (container) {
      assertProbeLabels(container, identity.labels, identity.container);
      runner("docker", ["rm", "--force", "--volumes", identity.container], { timeout: 120_000 });
      removed.push(identity.container);
    }
    const network = inspectDocker(runner, "network", identity.network);
    if (network) {
      assertProbeLabels(network, identity.labels, identity.network, true);
      runner("docker", ["network", "rm", identity.network], { timeout: 120_000 });
      removed.push(identity.network);
    }
  }
  return { status: "PASS", run_id: runId, removed, residue: [] };
}

function fleetNodeAddress(runtime) {
  const document = JSON.parse(run("docker", ["inspect", runtime.fleet.nodes[0].name]));
  const address = document?.[0]?.NetworkSettings?.Networks?.[runtime.fleet.network.name]?.IPAddress;
  if (!validIpAddress(address) || !/^(?:10\.|172\.|192\.168\.)/.test(address)) throw new Error("fleet node did not receive a private address");
  return address;
}

export function validateTcpProbeResult(result, attempts) {
  if (result?.attempts !== attempts || !Number.isSafeInteger(result?.max_in_flight) || result.max_in_flight < 1 || result.max_in_flight > PROBE_CONCURRENCY || !Array.isArray(result?.observations) || result.observations.length !== attempts) throw new Error("network route probe returned invalid bounded evidence");
  for (const [index, observation] of result.observations.entries()) {
    if (observation?.attempt !== index || typeof observation?.connected !== "boolean" || !validTimestamp(observation?.started_at) || !validTimestamp(observation?.ended_at)) throw new Error("network route probe returned invalid attempt evidence");
  }
  const succeeded = result.observations.filter((observation) => observation.connected).length;
  if (result.succeeded !== succeeded || result.denied !== attempts - succeeded) throw new Error("network route probe aggregate does not match raw attempts");
  return result;
}

async function runRoleTcpProbe(runtime, { role, address, port, attempts, statistics }) {
  if (!PROBE_ROLES.includes(role) || role === "isolation" || !validIpAddress(address) || !Number.isSafeInteger(port) || port < 1 || port > 65535 || !Number.isSafeInteger(attempts) || attempts < 1 || attempts > 1_000) throw new Error("network role probe parameters are invalid");
  const probe = exactProbeIdentity(runtime.runId, role);
  try {
    run("docker", ["network", "create", ...(role === "cross-fleet" ? ["--internal"] : []), ...labelsToArgs(probe.labels), probe.network]);
    const program = "const net=require('node:net');const [host,portValue,countValue,limitValue]=process.argv.slice(1);const port=Number(portValue),count=Number(countValue),limit=Number(limitValue),observations=new Array(count);let next=0,active=0,done=0,ok=0,max=0;function launch(){while(active<limit&&next<count){const index=next++;active++;max=Math.max(max,active);const startedAt=new Date().toISOString(),s=net.connect({host,port});let settled=false;const finishOne=(connected)=>{if(settled)return;settled=true;active--;done++;if(connected)ok++;observations[index]={attempt:index,started_at:startedAt,ended_at:new Date().toISOString(),connected};if(done===count)process.stdout.write(JSON.stringify({attempts:count,succeeded:ok,denied:count-ok,max_in_flight:max,observations}));else launch()};const t=setTimeout(()=>{s.destroy();finishOne(false)},500);s.once('connect',()=>{clearTimeout(t);s.destroy();finishOne(true)});s.once('error',()=>{clearTimeout(t);finishOne(false)})}}launch()";
    const result = validateTcpProbeResult(JSON.parse(run("docker", ["run", "--rm", "--name", probe.container, ...labelsToArgs(probe.labels), "--network", probe.network, "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges:true", "--pids-limit", "32", "--memory", "64m", NODE_PROBE_IMAGE, "node", "-e", program, address, String(port), String(attempts), String(PROBE_CONCURRENCY)], { timeout: 120_000 })), attempts);
    statistics.max_in_flight = Math.max(statistics.max_in_flight ?? 0, result.max_in_flight);
    return result;
  } finally {
    cleanupProbeResources(runtime.runId, { roles: [role] });
  }
}

function namespaceTcpConnected(runtime, sourceContainer, route, address, port) {
  if (!CONTAINER_NAME.test(sourceContainer ?? "") || !["celld_to_management", "celld_to_store", "node_to_peer"].includes(route) || !validIpAddress(address) || !Number.isSafeInteger(port)) throw new Error("namespace route probe target is invalid");
  const name = `celld-route-${sha256(`${runtime.runId}\n${route}`).slice(0, 16)}`;
  const labels = { ...exactLabels(runtime.runId), "dev.agentic-sandbox.probe-role": route };
  const program = "const net=require('node:net');const [host,portValue]=process.argv.slice(1);const startedAt=new Date().toISOString(),socket=net.connect({host,port:Number(portValue)});let settled=false;const finish=(connected)=>{if(settled)return;settled=true;socket.destroy();process.stdout.write(JSON.stringify({started_at:startedAt,ended_at:new Date().toISOString(),connected}))};const timer=setTimeout(()=>finish(false),1000);socket.once('connect',()=>{clearTimeout(timer);finish(true)});socket.once('error',()=>{clearTimeout(timer);finish(false)})";
  const result = JSON.parse(run("docker", ["run", "--rm", "--name", name, ...labelsToArgs(labels), "--network", `container:${sourceContainer}`, "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges:true", "--pids-limit", "16", "--memory", "32m", NODE_PROBE_IMAGE, "node", "-e", program, address, String(port)], { timeout: 30_000 }));
  if (typeof result?.connected !== "boolean" || !validTimestamp(result?.started_at) || !validTimestamp(result?.ended_at)) throw new Error("namespace route probe returned invalid evidence");
  return result.connected;
}

function storageServiceAddress(runtime, service = "s3gateway1") {
  if (service !== "s3gateway1" || typeof runtime?.storage?.project !== "string") throw new Error("storage route service identity is invalid");
  const ids = run("docker", ["ps", "--filter", `label=com.docker.compose.project=${runtime.storage.project}`, "--filter", `label=com.docker.compose.service=${service}`, "--format", "{{.ID}}"], { timeout: 30_000 }).split(/\r?\n/).filter(Boolean);
  if (ids.length !== 1) throw new Error("storage route service is not an exact running container");
  const value = JSON.parse(run("docker", ["container", "inspect", ids[0]], { timeout: 30_000 }))?.[0];
  const labels = value?.Config?.Labels;
  const address = value?.NetworkSettings?.Networks?.[runtime.fleet.network.name]?.IPAddress;
  if (value?.State?.Running !== true || labels?.["com.docker.compose.project"] !== runtime.storage.project || labels?.["com.docker.compose.service"] !== service || !validIpAddress(address)) throw new Error("storage route service does not match the exact fixture network");
  return address;
}

const ROUTE_MATRIX = Object.freeze(["management_to_celld", "celld_to_management", "celld_to_store", "node_to_peer"]);

async function observeRouteMatrix(runtime) {
  const source = runtime.networkInventory.namespaces[0];
  const peerAddress = runtime.networkInventory.proxies[1]?.listen_address;
  if (!validIpAddress(peerAddress)) throw new Error("peer route target is absent from the exact proxy inventory");
  const storeAddress = storageServiceAddress(runtime);
  const observed = [];
  const record = async (route, probe) => {
    const reachable = await probe();
    observed.push({ route, reachable, observed_at: new Date().toISOString() });
  };
  await record("management_to_celld", () => probeMtlsProxy(runtime.networkInventory.proxies[0]));
  await record("celld_to_management", () => namespaceTcpConnected(runtime, source.container, "celld_to_management", runtime.managementHost, 8122));
  await record("celld_to_store", () => namespaceTcpConnected(runtime, source.container, "celld_to_store", storeAddress, 8334));
  await record("node_to_peer", () => namespaceTcpConnected(runtime, source.container, "node_to_peer", peerAddress, 8081));
  return observed;
}

export function validateDirectionalRouteMatrices(direction, before, during, healed) {
  if (!ROUTE_MATRIX.includes(direction)) throw new Error("partition matrix direction is invalid");
  const validate = (matrix, name) => {
    if (!Array.isArray(matrix) || matrix.length !== ROUTE_MATRIX.length || new Set(matrix.map((entry) => entry?.route)).size !== ROUTE_MATRIX.length || matrix.some((entry) => !ROUTE_MATRIX.includes(entry?.route) || typeof entry?.reachable !== "boolean" || !validTimestamp(entry?.observed_at))) throw new Error(`${name} partition route matrix is invalid`);
    return new Map(matrix.map((entry) => [entry.route, entry.reachable]));
  };
  const beforeMap = validate(before, "before"), duringMap = validate(during, "during"), healedMap = validate(healed, "healed");
  const passed = ROUTE_MATRIX.every((route) => beforeMap.get(route) === true && healedMap.get(route) === true && duringMap.get(route) === (route !== direction));
  if (!passed) throw new Error("directional partition changed the wrong route or did not heal");
  return true;
}

async function runDirectionalPartitionCampaign(runtime) {
  const inventory = runtime.networkInventory;
  const source = inventory.namespaces[0];
  const peerAddress = inventory.proxies[1]?.listen_address;
  if (!validIpAddress(peerAddress)) throw new Error("partition campaign peer target is absent from the exact proxy inventory");
  const destinations = {
    management_to_celld: { address: inventory.proxies[0].listen_address, port: inventory.proxies[0].listen_port },
    celld_to_management: { address: runtime.managementHost, port: 8122 },
    celld_to_store: { address: storageServiceAddress(runtime), port: 8334 },
    node_to_peer: { address: peerAddress, port: 8081 },
  };
  const trials = [];
  for (const direction of ROUTE_MATRIX) {
    const destination = destinations[direction];
    const fault = planDirectionalPartition(inventory, { direction, sourceContainer: source.container, sourceNamespaceInode: source.inode, destinationAddress: destination.address, destinationPort: destination.port });
    const before = await observeRouteMatrix(runtime);
    applyDirectionalPartition(inventory, fault);
    let during;
    let observationError = null;
    try { during = await observeRouteMatrix(runtime); }
    catch (error) { observationError = error; }
    finally { healDirectionalPartition(inventory, fault); }
    if (observationError) throw observationError;
    const healed = await observeRouteMatrix(runtime);
    validateDirectionalRouteMatrices(direction, before, during, healed);
    trials.push({
      fault_id: fault.id,
      boundary: fault.boundary,
      direction,
      source_container: fault.source_container,
      source_namespace_inode: fault.source_namespace_inode,
      destination: { address: fault.destination_address, port: fault.destination_port },
      nft: { family: fault.nft_family, table: fault.nft_table, chain: fault.nft_chain, comment: fault.nft_comment },
      before,
      during,
      healed,
      applied_at: fault.applied_at,
      healed_at: fault.healed_at,
      cleanup_verified: true,
    });
  }
  return trials;
}

async function runIsolation(runtime, timeline) {
  const attemptsPerClass = 1_000;
  const providerBefore = providerCounter(runtime);
  const probeStatistics = {};
  const nodeIp = fleetNodeAddress(runtime);
  const publicProbe = await runRoleTcpProbe(runtime, { role: "public", address: nodeIp, port: 8081, attempts: attemptsPerClass, statistics: probeStatistics });
  const publicDenied = publicProbe.denied;
  const crossFleetProbe = await runRoleTcpProbe(runtime, { role: "cross-fleet", address: nodeIp, port: 8081, attempts: attemptsPerClass, statistics: probeStatistics });
  const crossFleetDenied = crossFleetProbe.denied;
  const routeAttempts = [
    ...publicProbe.observations.map((observation) => ({ class: "public_internal", attempt: observation.attempt, role: "public", started_at: observation.started_at, ended_at: observation.ended_at, outcome: observation.connected ? "allowed" : "denied", status: null, code: observation.connected ? "network.unexpected_route" : "network.no_route" })),
    ...crossFleetProbe.observations.map((observation) => ({ class: "cross_fleet", attempt: observation.attempt, role: "cross_fleet", started_at: observation.started_at, ended_at: observation.ended_at, outcome: observation.connected ? "allowed" : "denied", status: null, code: observation.connected ? "network.unexpected_route" : "network.no_route" })),
  ];
  const gatewayAccess = await openStorageGatewayAccess(runtime.storage, { services: ["s3gateway1"] });
  const [storageEndpoint] = gatewayAccess.endpoints;
  const profile = {
    schema_version: STORAGE_PROFILE_SCHEMA, profile_id: runtime.storage.project, run_id: runtime.runId,
    dialect: "s3-v1", scope: "live_candidate", endpoint: storageEndpoint, region: runtime.storage.region,
    addressing_mode: "path", bucket: runtime.storage.bucket, run_prefix: runtime.storage.run_prefix,
    identity_file_ref: runtime.storage.revoked_identity_file_ref, ca_file_ref: runtime.storage.ca_file_ref,
    backend: { product: runtime.storage.backend.product, version: runtime.storage.backend.version, artifact_sha256: runtime.storage.backend.artifact_sha256, configuration_sha256: runtime.storage.backend.configuration_sha256, gateway_endpoints: [storageEndpoint], topology: runtime.storage.backend.topology },
    limits: runtime.storage.limits,
  };
  let client = null;
  let crossBucketDenied = 0;
  let operationError = null;
  try {
    client = new S3V1Client(profile);
    for (let index = 0; index < attemptsPerClass; index += 1) {
      const requestStartedAt = new Date().toISOString();
      const response = await client.listPrefix();
      const requestEndedAt = new Date().toISOString();
      const rejected = [401, 403].includes(response.status);
      if (rejected) crossBucketDenied += 1;
      routeAttempts.push({ class: "cross_scope_store", attempt: index, role: "store", started_at: requestStartedAt, ended_at: requestEndedAt, outcome: rejected ? "denied" : "allowed", status: response.status, code: rejected ? "s3.access_denied" : "s3.unexpected_response" });
    }
  } catch (error) {
    operationError = error;
  } finally {
    const cleanupErrors = [];
    try { client?.close(); } catch (error) { cleanupErrors.push(error); }
    try { await gatewayAccess.close(); } catch (error) { cleanupErrors.push(error); }
    if (cleanupErrors.length) throw new AggregateError([...(operationError ? [operationError] : []), ...cleanupErrors], "network isolation gateway cleanup failed");
  }
  if (operationError) throw operationError;
  const partitionTrials = await runDirectionalPartitionCampaign(runtime);
  const diagnosis = diagnoseFleet(runtime.fleetPath);
  const providerAfter = providerCounter(runtime);
  const providerEffects = providerAfter - providerBefore;
  if (providerEffects < 0) throw new Error("management provider counter regressed during isolation probes");
  timeline.push({ scenario: "UAT-CELLD-010", public_internal: { attempts: attemptsPerClass, denied: publicDenied }, cross_fleet: { attempts: attemptsPerClass, denied: crossFleetDenied }, cross_bucket: { attempts: attemptsPerClass, denied: crossBucketDenied }, provider_counter: { source: "management-effect-ledger", before: providerBefore, after: providerAfter, delta: providerEffects }, fleet_status: diagnosis.status });
  const denied = publicDenied + crossFleetDenied + crossBucketDenied;
  return { assertions: [{ id: "CELLD.010.ISOLATION", measurements: { classes: ["public_internal", "cross_fleet", "cross_bucket"], forbidden_attempts: attemptsPerClass * 3, denied, succeeded: attemptsPerClass * 3 - denied, provider_counter_observed: true, provider_counter_before: providerBefore, provider_counter_after: providerAfter, provider_effects: providerEffects, routes_healed: diagnosis.status === "READY", directional_partitions: partitionTrials.length, partition_matrices_complete: partitionTrials.length === ROUTE_MATRIX.length, probe_concurrency_limit: PROBE_CONCURRENCY, probe_max_in_flight: probeStatistics.max_in_flight } }], route_attempts: routeAttempts, partition_trials: partitionTrials, probe_pool: { limit: PROBE_CONCURRENCY, max_in_flight: probeStatistics.max_in_flight }, provider_counter: { source: "management-effect-ledger", observed: true, before: providerBefore, after: providerAfter, delta: providerEffects }, metrics: [{ name: "forbidden_routes_denied", value: denied, unit: "requests" }], faults: partitionTrials.map((trial) => ({ kind: trial.direction, fault_id: trial.fault_id, healed: trial.cleanup_verified })) };
}

async function runAuthentication(runtime, timeline) {
  const route = privateCelldRoute(runtime.networkInventory), keyring = workerKey(runtime.fleet.worker_vars_file_ref);
  const providerBefore = providerCounter(runtime);
  let denied = 0;
  const probeStatistics = {};
  const nodeAddress = fleetNodeAddress(runtime);
  const transportAttempts = [...await probeMtlsTransportNegatives(runtime.networkInventory), await probeProxyBypass(nodeAddress), await probeEnvironmentProxy(route, keyring)];
  const routeAttempts = [...transportAttempts];
  timeline.push(...transportAttempts.map((attempt) => ({ scenario: "UAT-CELLD-012", transport: attempt })));
  for (const kind of DENIAL_CLASSES) {
    const codes = new Map();
    let results;
    if (kind === "public_route" || kind === "cross_fleet_request") {
      const probe = await runRoleTcpProbe(runtime, {
        role: kind === "public_route" ? "public" : "cross-fleet",
        address: nodeAddress,
        port: 8081,
        attempts: 1_000,
        statistics: probeStatistics,
      });
      results = probe.observations.map((observation) => ({ status: null, code: observation.connected ? "network.unexpected_route" : "network.no_route", connected: observation.connected, attempt: observation.attempt, started_at: observation.started_at, ended_at: observation.ended_at }));
    } else {
      results = await mapBounded(Array.from({ length: 1_000 }, (_value, attempt) => attempt), PROBE_CONCURRENCY, async (attempt) => {
        const requestStartedAt = new Date().toISOString();
        const result = await negativeRequest(route, keyring, kind, attempt);
        return { ...result, attempt, started_at: requestStartedAt, ended_at: new Date().toISOString() };
      }, probeStatistics);
    }
    let classDenied = 0;
    for (const result of results) {
      const expected = kind === "public_route" || kind === "cross_fleet_request"
        ? result.connected === false && result.code === "network.no_route"
        : kind === "nonce_replay"
          ? result.status === 409 && result.code === "cell.signature_replayed"
          : result.status === 401 && typeof result.code === "string" && result.code.startsWith("cell.signature_");
      if (expected) { denied += 1; classDenied += 1; }
      codes.set(result.code, (codes.get(result.code) ?? 0) + 1);
      routeAttempts.push({
        class: kind,
        attempt: result.attempt,
        role: kind === "public_route" ? "public" : kind === "cross_fleet_request" ? "cross_fleet" : "management",
        started_at: result.started_at,
        ended_at: result.ended_at,
        outcome: expected ? "denied" : "allowed",
        status: result.status,
        code: result.code,
        ...(result.operation_id_sha256 ? { operation_id_sha256: result.operation_id_sha256 } : {}),
      });
    }
    timeline.push({ scenario: "UAT-CELLD-012", class: kind, attempts: 1_000, denied: classDenied, codes: Object.fromEntries(codes) });
  }
  for (const kind of DENIAL_CLASSES.filter((value) => value !== "public_route" && value !== "cross_fleet_request")) {
    const path = `/instance-cells/negative-${kind}`;
    const operationId = `absence-${kind}-${randomBytes(8).toString("hex")}`;
    const result = await boundedRequest(route.endpoint, path, { method: "GET", headers: signedHeaders({ method: "GET", path, operationId, generation: 1, body: "", ...keyring }), tls: route.tls });
    if (result.status !== 404 || result.code !== "cell.missing") throw new Error(`negative authentication class created durable state: ${kind}`);
  }
  const validOperation = `valid-${randomBytes(12).toString("hex")}`, validPath = `/instance-cells/${randomUUID()}`;
  const validHeaders = signedHeaders({ method: "GET", path: validPath, operationId: validOperation, generation: 1, body: "", ...keyring });
  const validStartedAt = new Date().toISOString();
  const valid = await boundedRequest(route.endpoint, validPath, { method: "GET", headers: validHeaders, restrictedValues: [validHeaders["x-agentic-signature"], keyring.key], tls: route.tls });
  const validEndedAt = new Date().toISOString();
  if (valid.status !== 404 || valid.code !== "cell.missing") throw new Error("valid signed Worker identity did not authenticate exactly once");
  routeAttempts.push({ class: "valid_private_control", attempt: 0, role: "management", started_at: validStartedAt, ended_at: validEndedAt, outcome: "allowed", status: valid.status, code: valid.code, operation_id_sha256: sha256(validOperation), correlation_sha256: sha256(`${validOperation}\n${valid.status}\n${valid.code}`) });
  const providerAfter = providerCounter(runtime);
  const providerEffects = providerAfter - providerBefore;
  if (providerEffects < 0) throw new Error("management provider counter regressed during authentication probes");
  timeline.push({ scenario: "UAT-CELLD-012", valid_operation_sha256: sha256(validOperation), status: valid.status, code: valid.code, provider_counter: { source: "management-effect-ledger", before: providerBefore, after: providerAfter, delta: providerEffects } });
  return { assertions: [
    { id: "CELLD.012.DENIAL", measurements: { classes: DENIAL_CLASSES, attempts_per_class: 1_000, attempts: 9_000, denied, provider_counter_observed: true, provider_counter_before: providerBefore, provider_counter_after: providerAfter, provider_effects: providerEffects, probe_concurrency_limit: PROBE_CONCURRENCY, probe_max_in_flight: probeStatistics.max_in_flight } },
    { id: "CELLD.012.VALID", measurements: { attempts: 1, successes: 1, correlated: true, signature_value_absent: valid.restricted_absent, identity_removed: false } },
  ], route_attempts: routeAttempts, partition_trials: [], probe_pool: { limit: PROBE_CONCURRENCY, max_in_flight: probeStatistics.max_in_flight }, provider_counter: { source: "management-effect-ledger", observed: true, before: providerBefore, after: providerAfter, delta: providerEffects }, metrics: [{ name: "signed_negative_denials", value: denied, unit: "requests" }], faults: [{ kind: "signed_authentication_negative_matrix", classes: DENIAL_CLASSES.length }] };
}

function artifact(path, relativePath, mimeType) {
  const bytes = readFileSync(path);
  return { path: relativePath, mime_type: mimeType, sha256: sha256(bytes), bytes: bytes.length, contains_restricted_data: false };
}

function unavailable(profile, scenarioId, runId, startedAt, reasonCode) {
  return { schema_version: OBSERVATION_SCHEMA, driver_id: DRIVER_ID, run_id: runId, scenario_id: scenarioId, started_at: startedAt, ended_at: new Date().toISOString(), mutation_started: false, prerequisites: [{ id: "CELLD_NETWORK_AUTH", status: "unavailable", reason_code: reasonCode }], assertions: [], identities: { profile_id: profile.profile_id, sandbox_git: profile.expected_sandbox_git, environment_host_sha256: profile.environment.host_sha256, driver_version: DRIVER_VERSION }, metrics: [], faults: [], artifacts: [], cleanup: { status: "not_required", assertions: [] } };
}

export async function executeNetworkAuthDriver({ scenarioId, runId, liveProfilePath, artifactDir }, dependencies = {}) {
  const startedAt = new Date().toISOString();
  const errorFields = { scenarioId };
  const profile = await withDriverOperation("network-auth.load-profile", errorFields, () => protectedJson(liveProfilePath, "live profile"));
  const profileErrors = await withDriverOperation("network-auth.validate-profile", errorFields, () => validateLiveProfile(profile));
  if (profileErrors.length) throw driverOperationError("network-auth.validate-profile", errorFields, profileErrors.join("; "));
  const entry = profile.drivers?.[DRIVER_ID];
  if (!entry?.enabled) return unavailable(profile, scenarioId, runId, startedAt, "CELLD_NETWORK_AUTH_DRIVER_DISABLED");
  const git = await withDriverOperation("network-auth.git-identity", errorFields, () => dependencies.gitCommit?.() ?? run("git", ["rev-parse", "HEAD"]));
  const host = await withDriverOperation("network-auth.host-identity", errorFields, async () => dependencies.hostname?.() ?? (await import("node:os")).hostname());
  if (!SCENARIOS.has(scenarioId) || profile.run_id !== runId || profile.expected_sandbox_git !== git || profile.environment.host_sha256 !== sha256(host)) throw driverOperationError("network-auth.validate-identity", errorFields, "network/auth live identity does not match the requested run");
  const config = await withDriverOperation("network-auth.load-config", errorFields, () => protectedJson(entry.config_path, "orchestration config"));
  const configErrors = await withDriverOperation("network-auth.validate-config", errorFields, () => validateOrchestrationConfig(config));
  if (configErrors.length || config.run_id !== runId) throw driverOperationError("network-auth.validate-config", errorFields, [...configErrors, "config run identity mismatch"].join("; "));
  if (process.platform !== "linux") return unavailable(profile, scenarioId, runId, startedAt, "CELLD_NETWORK_AUTH_LINUX_REQUIRED");
  if (spawnSync("docker", ["image", "inspect", NODE_PROBE_IMAGE], { encoding: "utf8", shell: false }).status !== 0) return unavailable(profile, scenarioId, runId, startedAt, "CELLD_NETWORK_PROBE_IMAGE_UNAVAILABLE");

  mkdirSync(artifactDir, { recursive: true, mode: 0o700 }); chmodSync(artifactDir, 0o700);
  const timeline = [];
  let storage = null, fleet = null, fleetPath = null, campaign = null, networkInventory = null, management = null;
  let cleanupStatus = "failed";
  const cleanupAssertions = [];
  try {
    const root = join(config.working_root, `${scenarioId.toLowerCase()}-network`, runId);
    storage = await withDriverOperation("network-auth.prepare-storage-fixture", errorFields, () => prepareFixture({ fixtureProfile: "titan-single-host-storage", runId, root }));
    await withDriverOperation("network-auth.start-storage-fixture", errorFields, () => startFixture(storage));
    fleet = await withDriverOperation("network-auth.prepare-fleet", errorFields, () => prepareFleet({ storageConfigPath: join(root, "fixture.json") }));
    fleetPath = join(root, "fleet.json");
    await withDriverOperation("network-auth.deploy-worker", errorFields, () => deployFleetWorker(fleetPath));
    if ((await withDriverOperation("network-auth.start-fleet", errorFields, () => startFleet(fleetPath))).status !== "READY") throw driverOperationError("network-auth.start-fleet", errorFields, "network/auth fleet is not ready");
    networkInventory = await withDriverOperation("network-auth.create-inventory", errorFields, () => createNetworkAuthInventory({ runId, runRoot: root, host }));
    const namespaces = await withDriverOperation("network-auth.observe-namespaces", errorFields, () => observeFleetNetworkNamespaces(networkInventory, fleet));
    const fleetAddresses = namespaces.map((namespace) => namespace.address);
    for (const namespace of namespaces) {
      await withDriverOperation("network-auth.plan-listener-guard", errorFields, () => planListenerGuard(networkInventory, { sourceContainer: namespace.container, sourceNamespaceInode: namespace.inode, sameFleetAddresses: fleetAddresses }));
    }
    await withDriverOperation("network-auth.persist-guard-plan", errorFields, () => persistNetworkAuthInventory(networkInventory));
    for (const guard of networkInventory.guards) await withDriverOperation("network-auth.apply-listener-guard", errorFields, () => applyListenerGuard(networkInventory, guard));
    const proxyBinaryPath = join(dirname(config.callback_relay_binary_path), "agentic-celld-mtls-proxy");
    const proxyBinary = await withDriverOperation("network-auth.inspect-mtls-proxy-binary", errorFields, () => lstatSync(proxyBinaryPath));
    if (!proxyBinary.isFile() || proxyBinary.isSymbolicLink() || (proxyBinary.mode & 0o111) === 0) throw driverOperationError("network-auth.inspect-mtls-proxy-binary", errorFields, "network/auth mTLS proxy executable is unavailable");
    const proxyBinarySha256 = await withDriverOperation("network-auth.hash-mtls-proxy-binary", errorFields, () => sha256(readFileSync(proxyBinaryPath)));
    for (const namespace of namespaces) {
      await withDriverOperation("network-auth.plan-mtls-proxy", errorFields, () => planMtlsProxy(networkInventory, {
        nodeContainer: namespace.container,
        listenAddress: namespace.address,
        binarySha256: proxyBinarySha256,
        imageRef: config.docker_image_ref,
      }));
    }
    await withDriverOperation("network-auth.persist-proxy-plan", errorFields, () => persistNetworkAuthInventory(networkInventory));
    await withDriverOperation("network-auth.prepare-mtls-certificates", errorFields, () => prepareMtlsProxyCertificates(networkInventory));
    for (const proxy of networkInventory.proxies) await withDriverOperation("network-auth.start-mtls-proxy", errorFields, () => startMtlsProxy(networkInventory, proxy, { binaryPath: proxyBinaryPath }));
    await withDriverOperation("network-auth.wait-mtls-proxies", errorFields, () => waitMtlsProxies(networkInventory));
    const managementHost = await withDriverOperation("network-auth.resolve-storage-gateway", errorFields, () => storageGateway(fleet));
    management = await withDriverOperation("network-auth.launch-management", errorFields, () => launchManagement(config, fleet, managementHost, {
      celldEndpoint: `https://${networkInventory.proxies[0].listen_address}:${networkInventory.proxies[0].listen_port}`,
      tlsCaFile: networkInventory.proxies[0].ca_file_ref,
      tlsIdentityFile: networkInventory.proxies[0].management_client_identity_file_ref,
    }));
    await withDriverOperation("network-auth.wait-management", errorFields, () => waitManagement(management, fleet));
    await withDriverOperation("network-auth.start-callback-relays", errorFields, () => startCallbackRelays(fleetPath, { relayBinaryPath: config.callback_relay_binary_path }));
    const runtime = { config, storage, fleet, fleetPath, runId, management, managementHost, networkInventory };
    campaign = await withDriverOperation(scenarioId === "UAT-CELLD-010" ? "network-auth.run-isolation" : "network-auth.run-authentication", errorFields, () => (dependencies.runScenario ?? (scenarioId === "UAT-CELLD-010" ? runIsolation : runAuthentication))(runtime, timeline));
  } finally {
    try { cleanupProbeResources(runId); cleanupAssertions.push("exact network probe container and network removed"); } catch (error) { cleanupAssertions.push(`network probe cleanup digest ${sha256(error.message)}`); }
    try { await stopManagementAndWait(management, "SIGKILL"); cleanupAssertions.push("network/auth management process terminated"); } catch (error) { cleanupAssertions.push(`management cleanup digest ${sha256(error.message)}`); }
    try { if (networkInventory) recoverNetworkAuthInventory(networkInventory); cleanupAssertions.push("exact network partitions and mTLS proxies removed"); } catch (error) { cleanupAssertions.push(`network mutation cleanup digest ${sha256(error.message)}`); }
    try { if (fleetPath && existsSync(fleetPath)) cleanupFleet(fleetPath); cleanupAssertions.push("exact network/auth fleet removed"); } catch (error) { cleanupAssertions.push(`fleet cleanup digest ${sha256(error.message)}`); }
    try { if (storage) cleanupFixture(storage); cleanupAssertions.push("exact network/auth storage fixture removed"); } catch (error) { cleanupAssertions.push(`storage cleanup digest ${sha256(error.message)}`); }
    cleanupStatus = cleanupAssertions.some((value) => value.includes("digest")) ? "failed" : "passed";
  }
  if (!campaign) throw driverOperationError("network-auth.run-campaign", errorFields, "network/auth campaign produced no measurements");
  if (scenarioId === "UAT-CELLD-012") campaign.assertions.find((assertion) => assertion.id === "CELLD.012.VALID").measurements.identity_removed = !existsSync(fleet.worker_vars_file_ref);
  const suffix = scenarioId.toLowerCase(), evidenceName = `network-auth-evidence-${suffix}.json`, timelineName = `network-auth-timeline-${suffix}.jsonl`;
  const evidencePath = join(artifactDir, evidenceName), timelinePath = join(artifactDir, timelineName);
  const evidenceEndedAt = new Date().toISOString();
  const clean = cleanupStatus === "passed" && networkInventory?.state === "clean";
  const evidence = {
    schema_version: "agentic-sandbox.celld-network-auth-evidence/v1",
    run_id: runId,
    scenario_id: scenarioId,
    started_at: startedAt,
    ended_at: evidenceEndedAt,
    probe_pool: campaign.probe_pool,
    provider_counter: campaign.provider_counter,
    route_attempts: campaign.route_attempts,
    partition_trials: campaign.partition_trials,
    timeline_sha256: sha256(timeline.map((row) => JSON.stringify(row)).join("\n")),
    cleanup: { inventory_state: networkInventory?.state ?? "cleanup_residue", nft_rules_absent: clean, listener_guards_absent: clean, proxies_absent: clean, namespaces_absent: clean },
  };
  writeFileSync(evidencePath, `${JSON.stringify(evidence)}\n`, { mode: 0o600, flag: "wx" });
  writeFileSync(timelinePath, `${timeline.map((row) => JSON.stringify(row)).join("\n")}\n`, { mode: 0o600, flag: "wx" });
  const artifacts = [artifact(evidencePath, `artifacts/${evidenceName}`, "application/json"), artifact(timelinePath, `artifacts/${timelineName}`, "application/x-ndjson")];
  return { schema_version: OBSERVATION_SCHEMA, driver_id: DRIVER_ID, run_id: runId, scenario_id: scenarioId, started_at: startedAt, ended_at: evidenceEndedAt, mutation_started: true, prerequisites: [{ id: "CELLD_NETWORK_AUTH", status: "available", reason_code: "CELLD_NETWORK_AUTH_READY" }, { id: "CELLD_PRIVATE_FLEET", status: "available", reason_code: "CELLD_PRIVATE_FLEET_READY" }], assertions: campaign.assertions.map((item) => ({ ...item, evidence_refs: artifacts.map((entryArtifact) => entryArtifact.path) })), identities: { profile_id: profile.profile_id, sandbox_git: profile.expected_sandbox_git, environment_host_sha256: profile.environment.host_sha256, driver_version: DRIVER_VERSION }, metrics: campaign.metrics, faults: campaign.faults, artifacts, cleanup: { status: cleanupStatus, assertions: cleanupAssertions } };
}

async function main(args) {
  if (args[0] === "cleanup") {
    const configPath = resolve(argument(args, "--config"));
    const config = protectedJson(configPath, "orchestration config");
    const errors = validateOrchestrationConfig(config);
    if (errors.length || configPath !== join(config.working_root, "orchestration.json")) throw new Error([...errors, "config is not the fixed run-root path"].join("; "));
    const partitions = cleanupNetworkAuthInventories(config);
    const probes = cleanupProbeResources(config.run_id);
    process.stdout.write(`${JSON.stringify({ status: "PASS", run_id: config.run_id, partitions, probes })}\n`);
    return;
  }
  const observation = await executeNetworkAuthDriver({ scenarioId: argument(args, "--scenario-id"), runId: argument(args, "--run-id"), liveProfilePath: resolve(argument(args, "--profile")), artifactDir: resolve(argument(args, "--artifact-dir")) });
  process.stdout.write(`${JSON.stringify(observation)}\n`);
}

if (process.argv[1] && SCRIPT_PATH === resolve(process.argv[1])) main(process.argv.slice(2)).catch(emitDriverError);
