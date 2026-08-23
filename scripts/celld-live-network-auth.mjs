#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash, createHmac, randomBytes, randomUUID } from "node:crypto";
import { chmodSync, existsSync, lstatSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { connect, isIP } from "node:net";
import { hostname } from "node:os";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";
import { connect as tlsConnect } from "node:tls";
import { fileURLToPath } from "node:url";

import { cleanupFleet, deployFleetWorker, diagnoseFleet, prepareFleet, startCallbackRelays, startFleet } from "./celld-fleet-fixture.mjs";
import { cleanupFixture, prepareFixture, startFixture } from "./celld-seaweedfs-fixture.mjs";
import { openStorageGatewayAccess } from "./celld-storage-gateway-access.mjs";
import { S3V1Client, STORAGE_PROFILE_SCHEMA } from "./celld-storage-qualifier.mjs";
import { launchManagement, stopManagementAndWait, storageGateway, validateOrchestrationConfig, waitManagement } from "./celld-live-orchestration.mjs";
import { validateLiveProfile } from "./celld-uat-live-protocol.mjs";

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
  const allowed = new Set(["schema_version", "run_id", "run_root", "owner", "host_sha256", "created_at", "updated_at", "state", "namespaces", "proxies", "faults"]);
  for (const key of Object.keys(inventory)) if (!allowed.has(key)) errors.push(`inventory.${key} is not allowed`);
  if (inventory.schema_version !== NETWORK_AUTH_INVENTORY_SCHEMA) errors.push("network inventory schema is invalid");
  if (!RUN_ID.test(inventory.run_id ?? "") || (runId && inventory.run_id !== runId)) errors.push("network inventory run ID is invalid");
  if (!isAbsolute(inventory.run_root ?? "") || !resolve(inventory.run_root).startsWith("/dev/shm/") || !resolve(inventory.run_root).split("/").includes(inventory.run_id) || (runRoot && resolve(inventory.run_root) !== resolve(runRoot))) errors.push("network inventory root is invalid");
  if (inventory.owner?.repository !== NETWORK_AUTH_OWNER.repository || inventory.owner?.workflow !== NETWORK_AUTH_OWNER.workflow || inventory.owner?.run_id !== inventory.run_id) errors.push("network inventory owner is invalid");
  if (!SHA256.test(inventory.host_sha256 ?? "") || (hostSha256 && inventory.host_sha256 !== hostSha256)) errors.push("network inventory host is invalid");
  if (!validTimestamp(inventory.created_at) || !validTimestamp(inventory.updated_at)) errors.push("network inventory timestamps are invalid");
  if (!["prepared", "active", "cleanup_residue", "clean"].includes(inventory.state)) errors.push("network inventory state is invalid");
  if (!Array.isArray(inventory.namespaces) || !Array.isArray(inventory.proxies) || !Array.isArray(inventory.faults)) return [...errors, "network inventory namespaces/proxies/faults must be arrays"];
  const containers = new Set(), inodes = new Set();
  for (const [index, namespace] of inventory.namespaces.entries()) {
    if (!CONTAINER_NAME.test(namespace?.container ?? "") || !Number.isSafeInteger(namespace?.pid) || namespace.pid < 1 || !Number.isSafeInteger(namespace?.inode) || namespace.inode < 1 || namespace?.run_label !== inventory.run_id || containers.has(namespace.container) || inodes.has(namespace.inode)) errors.push(`network inventory namespace is invalid at index ${index}`);
    containers.add(namespace?.container); inodes.add(namespace?.inode);
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
  return { tls_root: tlsRoot, management_client_identity_file_ref: clientIdentity, servers };
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
      inventory.state = inventory.proxies.every((entry) => entry.status === "removed") && inventory.faults.every((entry) => entry.status === "healed") ? "clean" : "active";
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
  return { status: result.status, stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
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

export function directionalPartitionCommands(inventory, fault) {
  const errors = validateNetworkAuthInventory(inventory);
  if (errors.length || !inventory.faults.includes(fault) || fault.status === "healed") throw new Error("partition command target is not a live exact-run fault");
  const namespace = inventory.namespaces.find((entry) => entry.container === fault.source_container && entry.inode === fault.source_namespace_inode);
  if (!namespace) throw new Error("partition command namespace is not inventory bound");
  const prefix = ["--target", String(namespace.pid), "--net", "--", "nft"];
  const addressFamily = isIP(fault.destination_address) === 6 ? "ip6" : "ip";
  return {
    inspect: [...prefix, "list", "table", fault.nft_family, fault.nft_table],
    apply: [
      [...prefix, "add", "table", fault.nft_family, fault.nft_table],
      [...prefix, "add", "chain", fault.nft_family, fault.nft_table, fault.nft_chain, "{", "type", "filter", "hook", "output", "priority", "-150", ";", "policy", "accept", ";", "}"],
      [...prefix, "add", "rule", fault.nft_family, fault.nft_table, fault.nft_chain, addressFamily, "daddr", fault.destination_address, "tcp", "dport", String(fault.destination_port), "counter", "drop", "comment", fault.nft_comment],
    ],
    heal: [...prefix, "delete", "table", fault.nft_family, fault.nft_table],
  };
}

export function applyDirectionalPartition(inventory, fault, { executor = rawCommand, persist = persistNetworkAuthInventory, dockerRunner = run, namespaceInode, now = new Date() } = {}) {
  if (fault.status !== "planned") throw new Error("only a planned directional partition can be applied");
  exactNamespace(inventory, fault, { dockerRunner, namespaceInode });
  persist(inventory);
  const commands = directionalPartitionCommands(inventory, fault);
  const existing = executor("nsenter", commands.inspect, { timeout: 30_000 });
  if (existing.status === 0) throw new Error("refusing to replace an existing exact-name nftables table");
  for (const args of commands.apply) {
    const result = executor("nsenter", args, { timeout: 30_000 });
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
  const deletion = executor("nsenter", commands.heal, { timeout: 30_000 });
  if (deletion.status !== 0) {
    const remaining = executor("nsenter", commands.inspect, { timeout: 30_000 });
    if (remaining.status === 0) throw new Error(`directional partition heal failed: ${sha256(deletion.stderr ?? "")}`);
  }
  const timestamp = now.toISOString();
  fault.status = "healed";
  fault.healed_at = timestamp;
  fault.updated_at = timestamp;
  inventory.updated_at = timestamp;
  inventory.state = inventory.faults.every((entry) => entry.status === "healed") && inventory.proxies.every((entry) => entry.status === "removed") ? "clean" : "active";
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
  try {
    cleanupMtlsProxies(inventory, { executor, persist, now });
  } catch (error) {
    failures.push(error);
  }
  inventory.updated_at = now().toISOString();
  inventory.state = failures.length === 0 && inventory.faults.every((fault) => fault.status === "healed") && inventory.proxies.every((proxy) => proxy.status === "removed")
    ? "clean"
    : "cleanup_residue";
  persist(inventory);
  if (failures.length) throw new AggregateError(failures, "exact-run network partition cleanup left residue");
  return {
    status: "PASS",
    run_id: inventory.run_id,
    inventory_state: inventory.state,
    healed_fault_ids: inventory.faults.map((fault) => fault.id),
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

async function boundedRequest(endpoint, path, { method, headers, body, restrictedValues = [] }) {
  const response = await fetch(new URL(path, endpoint), { method, headers, body, redirect: "error", signal: AbortSignal.timeout(10_000) });
  const bytes = Buffer.from(await response.arrayBuffer());
  if (bytes.length > 4096) throw new Error("Worker denial response exceeds 4 KiB");
  const restrictedAbsent = restrictedValues.every((value) => typeof value === "string" && value.length > 0 && !bytes.includes(Buffer.from(value)));
  let value;
  try { value = JSON.parse(bytes.toString("utf8")); } catch { throw new Error("Worker denial response is not JSON"); }
  return { status: response.status, code: value?.error?.code ?? null, restricted_absent: restrictedAbsent };
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

async function negativeRequest(endpoint, keyring, kind, attempt) {
  const instanceId = `negative-${kind}`;
  const operationId = `negative-${kind}-${attempt}`;
  const path = `/instance-cells/${instanceId}/commands`;
  const originalBody = commandBody(operationId, instanceId);
  let body = originalBody;
  let headers = signedHeaders({ method: "POST", path, operationId, generation: 1, body, ...keyring });
  if (kind === "forged_body") body = `${originalBody.slice(0, -1)},"tampered":true}`;
  else if (kind === "forged_mac") headers["x-agentic-signature"] = "0".repeat(64);
  else if (kind === "stale_timestamp") {
    const timestamp = new Date(Date.now() - 10 * 60_000).toISOString();
    headers = signedHeaders({ method: "POST", path, operationId, generation: 1, body, timestamp, ...keyring });
  } else if (kind === "wrong_key") headers = signedHeaders({ method: "POST", path, operationId, generation: 1, body, keyId: keyring.keyId, key: randomBytes(32).toString("hex") });
  else if (kind === "zero_generation") headers = signedHeaders({ method: "POST", path, operationId, generation: 0, body, ...keyring });
  else if (kind === "wrong_generation") headers["x-agentic-generation"] = "2";
  else if (kind === "public_route" || kind === "cross_fleet_request") headers = {};
  if (kind === "nonce_replay") {
    const getPath = `/instance-cells/replay-${attempt}`;
    const getHeaders = signedHeaders({ method: "GET", path: getPath, operationId, generation: 1, body: "", ...keyring });
    const first = await boundedRequest(endpoint, getPath, { method: "GET", headers: getHeaders });
    if (first.status !== 404 || first.code !== "cell.missing") throw new Error("nonce replay setup did not authenticate without a provider effect");
    return boundedRequest(endpoint, getPath, { method: "GET", headers: getHeaders });
  }
  return boundedRequest(endpoint, path, { method: "POST", headers: { "content-type": "application/json", ...headers }, body });
}

async function tcpDenied(host, port, attempts, statistics) {
  const results = await mapBounded(Array.from({ length: attempts }, (_value, index) => index), PROBE_CONCURRENCY, async () => {
    return new Promise((resolvePromise) => {
      const socket = connect({ host, port });
      const timer = setTimeout(() => { socket.destroy(); resolvePromise(false); }, 500);
      socket.once("connect", () => { clearTimeout(timer); socket.destroy(); resolvePromise(true); });
      socket.once("error", () => { clearTimeout(timer); resolvePromise(false); });
    });
  }, statistics);
  return results.filter((connected) => !connected).length;
}

function exactProbeIdentity(runId) {
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(runId)) throw new Error("probe cleanup run identity is invalid");
  const network = `celld-probe-${sha256(runId).slice(0, 16)}`;
  return { network, container: `${network}-client`, labels: { "dev.agentic-sandbox.run": runId, "dev.agentic-sandbox.scope": "celld-qualification" } };
}

function inspectDocker(runner, kind, name) {
  try { return JSON.parse(runner("docker", [kind, "inspect", name])); } catch { return null; }
}

function assertProbeLabels(document, expected, name, network = false) {
  const labels = network ? document?.[0]?.Labels : document?.[0]?.Config?.Labels;
  for (const [key, value] of Object.entries(expected)) if (labels?.[key] !== value) throw new Error(`refusing unowned probe resource ${name}`);
}

export function cleanupProbeResources(runId, { runner = run } = {}) {
  const identity = exactProbeIdentity(runId);
  const removed = [];
  runner("docker", ["info", "--format", "{{.ServerVersion}}"], { timeout: 30_000 });
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
  return { status: "PASS", run_id: runId, removed, residue: [] };
}

async function runIsolation(runtime, timeline) {
  const attemptsPerClass = 1_000;
  const providerBefore = providerCounter(runtime);
  const probeStatistics = {};
  const publicDenied = await tcpDenied("127.0.0.1", 8081, attemptsPerClass, probeStatistics);
  const nodeDocument = JSON.parse(run("docker", ["inspect", runtime.fleet.nodes[0].name]));
  const nodeIp = nodeDocument?.[0]?.NetworkSettings?.Networks?.[runtime.fleet.network.name]?.IPAddress;
  if (!/^172\.|^10\.|^192\.168\./.test(nodeIp ?? "")) throw new Error("fleet node did not receive a private address");
  const probe = exactProbeIdentity(runtime.runId);
  let crossFleetDenied = 0;
  try {
    run("docker", ["network", "create", "--internal", "--label", `dev.agentic-sandbox.run=${runtime.runId}`, "--label", "dev.agentic-sandbox.scope=celld-qualification", probe.network]);
    const program = "const net=require('node:net');const [host,portValue,countValue,limitValue]=process.argv.slice(1);const port=Number(portValue),count=Number(countValue),limit=Number(limitValue);let next=0,active=0,done=0,ok=0,max=0;function launch(){while(active<limit&&next<count){next++;active++;max=Math.max(max,active);const s=net.connect({host,port});let settled=false;const finishOne=(connected)=>{if(settled)return;settled=true;active--;done++;if(connected)ok++;if(done===count)process.stdout.write(JSON.stringify({attempts:count,succeeded:ok,denied:count-ok,max_in_flight:max}));else launch()};const t=setTimeout(()=>{s.destroy();finishOne(false)},500);s.once('connect',()=>{clearTimeout(t);s.destroy();finishOne(true)});s.once('error',()=>{clearTimeout(t);finishOne(false)})}}launch()";
    const result = JSON.parse(run("docker", ["run", "--rm", "--name", probe.container, "--label", `dev.agentic-sandbox.run=${runtime.runId}`, "--label", "dev.agentic-sandbox.scope=celld-qualification", "--network", probe.network, "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges:true", "--pids-limit", "32", "--memory", "64m", NODE_PROBE_IMAGE, "node", "-e", program, nodeIp, "8081", String(attemptsPerClass), String(PROBE_CONCURRENCY)], { timeout: 120_000 }));
    if (!Number.isSafeInteger(result.max_in_flight) || result.max_in_flight > PROBE_CONCURRENCY) throw new Error("cross-fleet probe exceeded the bounded pool");
    probeStatistics.max_in_flight = Math.max(probeStatistics.max_in_flight ?? 0, result.max_in_flight);
    crossFleetDenied = result.denied;
  } finally {
    cleanupProbeResources(runtime.runId);
  }
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
      const response = await client.listPrefix();
      if ([401, 403].includes(response.status)) crossBucketDenied += 1;
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
  const diagnosis = diagnoseFleet(runtime.fleetPath);
  const providerAfter = providerCounter(runtime);
  const providerEffects = providerAfter - providerBefore;
  if (providerEffects < 0) throw new Error("management provider counter regressed during isolation probes");
  timeline.push({ scenario: "UAT-CELLD-010", public_internal: { attempts: attemptsPerClass, denied: publicDenied }, cross_fleet: { attempts: attemptsPerClass, denied: crossFleetDenied }, cross_bucket: { attempts: attemptsPerClass, denied: crossBucketDenied }, provider_counter: { source: "management-effect-ledger", before: providerBefore, after: providerAfter, delta: providerEffects }, fleet_status: diagnosis.status });
  const denied = publicDenied + crossFleetDenied + crossBucketDenied;
  return { assertions: [{ id: "CELLD.010.ISOLATION", measurements: { classes: ["public_internal", "cross_fleet", "cross_bucket"], forbidden_attempts: attemptsPerClass * 3, denied, succeeded: attemptsPerClass * 3 - denied, provider_counter_observed: true, provider_counter_before: providerBefore, provider_counter_after: providerAfter, provider_effects: providerEffects, routes_healed: diagnosis.status === "READY", probe_concurrency_limit: PROBE_CONCURRENCY, probe_max_in_flight: probeStatistics.max_in_flight } }], metrics: [{ name: "forbidden_routes_denied", value: denied, unit: "requests" }], faults: [{ kind: "isolated_cross_fleet_probe", healed: true }] };
}

async function runAuthentication(runtime, timeline) {
  const endpoint = workerEndpoint(runtime.fleet), keyring = workerKey(runtime.fleet.worker_vars_file_ref);
  const providerBefore = providerCounter(runtime);
  let denied = 0;
  const probeStatistics = {};
  for (const kind of DENIAL_CLASSES) {
    const codes = new Map();
    const results = await mapBounded(Array.from({ length: 1_000 }, (_value, attempt) => attempt), PROBE_CONCURRENCY, (attempt) => negativeRequest(endpoint, keyring, kind, attempt), probeStatistics);
    for (const result of results) {
      const expected = kind === "nonce_replay" ? result.status === 409 && result.code === "cell.signature_replayed" : result.status === 401 && typeof result.code === "string" && result.code.startsWith("cell.signature_");
      if (expected) denied += 1;
      codes.set(result.code, (codes.get(result.code) ?? 0) + 1);
    }
    timeline.push({ scenario: "UAT-CELLD-012", class: kind, attempts: 1_000, denied: [...codes.values()].reduce((sum, value) => sum + value, 0), codes: Object.fromEntries(codes) });
  }
  for (const kind of DENIAL_CLASSES) {
    const path = `/instance-cells/negative-${kind}`;
    const operationId = `absence-${kind}-${randomBytes(8).toString("hex")}`;
    const result = await boundedRequest(endpoint, path, { method: "GET", headers: signedHeaders({ method: "GET", path, operationId, generation: 1, body: "", ...keyring }) });
    if (result.status !== 404 || result.code !== "cell.missing") throw new Error(`negative authentication class created durable state: ${kind}`);
  }
  const validOperation = `valid-${randomBytes(12).toString("hex")}`, validPath = `/instance-cells/${randomUUID()}`;
  const validHeaders = signedHeaders({ method: "GET", path: validPath, operationId: validOperation, generation: 1, body: "", ...keyring });
  const valid = await boundedRequest(endpoint, validPath, { method: "GET", headers: validHeaders, restrictedValues: [validHeaders["x-agentic-signature"], keyring.key] });
  if (valid.status !== 404 || valid.code !== "cell.missing") throw new Error("valid signed Worker identity did not authenticate exactly once");
  const providerAfter = providerCounter(runtime);
  const providerEffects = providerAfter - providerBefore;
  if (providerEffects < 0) throw new Error("management provider counter regressed during authentication probes");
  timeline.push({ scenario: "UAT-CELLD-012", valid_operation_sha256: sha256(validOperation), status: valid.status, code: valid.code, provider_counter: { source: "management-effect-ledger", before: providerBefore, after: providerAfter, delta: providerEffects } });
  return { assertions: [
    { id: "CELLD.012.DENIAL", measurements: { classes: DENIAL_CLASSES, attempts_per_class: 1_000, attempts: 9_000, denied, provider_counter_observed: true, provider_counter_before: providerBefore, provider_counter_after: providerAfter, provider_effects: providerEffects, probe_concurrency_limit: PROBE_CONCURRENCY, probe_max_in_flight: probeStatistics.max_in_flight } },
    { id: "CELLD.012.VALID", measurements: { attempts: 1, successes: 1, correlated: true, signature_value_absent: valid.restricted_absent, identity_removed: false } },
  ], metrics: [{ name: "signed_negative_denials", value: denied, unit: "requests" }], faults: [{ kind: "signed_authentication_negative_matrix", classes: DENIAL_CLASSES.length }] };
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
  const profile = protectedJson(liveProfilePath, "live profile");
  const profileErrors = validateLiveProfile(profile);
  if (profileErrors.length) throw new Error(profileErrors.join("; "));
  const entry = profile.drivers?.[DRIVER_ID];
  if (!entry?.enabled) return unavailable(profile, scenarioId, runId, startedAt, "CELLD_NETWORK_AUTH_DRIVER_DISABLED");
  const git = dependencies.gitCommit?.() ?? run("git", ["rev-parse", "HEAD"]);
  const host = dependencies.hostname?.() ?? (await import("node:os")).hostname();
  if (!SCENARIOS.has(scenarioId) || profile.run_id !== runId || profile.expected_sandbox_git !== git || profile.environment.host_sha256 !== sha256(host)) throw new Error("network/auth live identity does not match the requested run");
  const config = protectedJson(entry.config_path, "orchestration config");
  const configErrors = validateOrchestrationConfig(config);
  if (configErrors.length || config.run_id !== runId) throw new Error([...configErrors, "config run identity mismatch"].join("; "));
  if (process.platform !== "linux") return unavailable(profile, scenarioId, runId, startedAt, "CELLD_NETWORK_AUTH_LINUX_REQUIRED");
  if (spawnSync("docker", ["image", "inspect", NODE_PROBE_IMAGE], { encoding: "utf8", shell: false }).status !== 0) return unavailable(profile, scenarioId, runId, startedAt, "CELLD_NETWORK_PROBE_IMAGE_UNAVAILABLE");

  mkdirSync(artifactDir, { recursive: true, mode: 0o700 }); chmodSync(artifactDir, 0o700);
  const timeline = [];
  let storage = null, fleet = null, fleetPath = null, campaign = null, networkInventory = null, management = null;
  let cleanupStatus = "failed";
  const cleanupAssertions = [];
  try {
    const root = join(config.working_root, `${scenarioId.toLowerCase()}-network`, runId);
    storage = prepareFixture({ fixtureProfile: "titan-single-host-storage", runId, root });
    startFixture(storage);
    fleet = prepareFleet({ storageConfigPath: join(root, "fixture.json") });
    fleetPath = join(root, "fleet.json");
    await deployFleetWorker(fleetPath);
    if (startFleet(fleetPath).status !== "READY") throw new Error("network/auth fleet is not ready");
    networkInventory = createNetworkAuthInventory({ runId, runRoot: root, host });
    const namespaces = observeFleetNetworkNamespaces(networkInventory, fleet);
    const proxyBinaryPath = join(dirname(config.callback_relay_binary_path), "agentic-celld-mtls-proxy");
    const proxyBinary = lstatSync(proxyBinaryPath);
    if (!proxyBinary.isFile() || proxyBinary.isSymbolicLink() || (proxyBinary.mode & 0o111) === 0) throw new Error("network/auth mTLS proxy executable is unavailable");
    const proxyBinarySha256 = sha256(readFileSync(proxyBinaryPath));
    for (const namespace of namespaces) {
      planMtlsProxy(networkInventory, {
        nodeContainer: namespace.container,
        listenAddress: namespace.address,
        binarySha256: proxyBinarySha256,
        imageRef: config.docker_image_ref,
      });
    }
    persistNetworkAuthInventory(networkInventory);
    prepareMtlsProxyCertificates(networkInventory);
    for (const proxy of networkInventory.proxies) startMtlsProxy(networkInventory, proxy, { binaryPath: proxyBinaryPath });
    await waitMtlsProxies(networkInventory);
    const managementHost = storageGateway(fleet);
    management = launchManagement(config, fleet, managementHost, {
      celldEndpoint: `https://${networkInventory.proxies[0].listen_address}:${networkInventory.proxies[0].listen_port}`,
      tlsCaFile: networkInventory.proxies[0].ca_file_ref,
      tlsIdentityFile: networkInventory.proxies[0].management_client_identity_file_ref,
    });
    await waitManagement(management, fleet);
    startCallbackRelays(fleetPath, { relayBinaryPath: config.callback_relay_binary_path });
    const runtime = { config, storage, fleet, fleetPath, runId, management, managementHost, networkInventory };
    campaign = await (dependencies.runScenario ?? (scenarioId === "UAT-CELLD-010" ? runIsolation : runAuthentication))(runtime, timeline);
  } finally {
    try { cleanupProbeResources(runId); cleanupAssertions.push("exact network probe container and network removed"); } catch (error) { cleanupAssertions.push(`network probe cleanup digest ${sha256(error.message)}`); }
    try { await stopManagementAndWait(management, "SIGKILL"); cleanupAssertions.push("network/auth management process terminated"); } catch (error) { cleanupAssertions.push(`management cleanup digest ${sha256(error.message)}`); }
    try { if (networkInventory) recoverNetworkAuthInventory(networkInventory); cleanupAssertions.push("exact network partitions and mTLS proxies removed"); } catch (error) { cleanupAssertions.push(`network mutation cleanup digest ${sha256(error.message)}`); }
    try { if (fleetPath && existsSync(fleetPath)) cleanupFleet(fleetPath); cleanupAssertions.push("exact network/auth fleet removed"); } catch (error) { cleanupAssertions.push(`fleet cleanup digest ${sha256(error.message)}`); }
    try { if (storage) cleanupFixture(storage); cleanupAssertions.push("exact network/auth storage fixture removed"); } catch (error) { cleanupAssertions.push(`storage cleanup digest ${sha256(error.message)}`); }
    cleanupStatus = cleanupAssertions.some((value) => value.includes("digest")) ? "failed" : "passed";
  }
  if (!campaign) throw new Error("network/auth campaign produced no measurements");
  if (scenarioId === "UAT-CELLD-012") campaign.assertions.find((assertion) => assertion.id === "CELLD.012.VALID").measurements.identity_removed = !existsSync(fleet.worker_vars_file_ref);
  const suffix = scenarioId.toLowerCase(), evidenceName = `network-auth-evidence-${suffix}.json`, timelineName = `network-auth-timeline-${suffix}.jsonl`;
  const evidencePath = join(artifactDir, evidenceName), timelinePath = join(artifactDir, timelineName);
  const evidence = { schema_version: "agentic-sandbox.celld-network-auth-evidence/v1", run_id: runId, scenario_id: scenarioId, measurements: Object.fromEntries(campaign.assertions.map((item) => [item.id, item.measurements])), timeline_sha256: sha256(timeline.map((row) => JSON.stringify(row)).join("\n")) };
  writeFileSync(evidencePath, `${JSON.stringify(evidence)}\n`, { mode: 0o600, flag: "wx" });
  writeFileSync(timelinePath, `${timeline.map((row) => JSON.stringify(row)).join("\n")}\n`, { mode: 0o600, flag: "wx" });
  const artifacts = [artifact(evidencePath, `artifacts/${evidenceName}`, "application/json"), artifact(timelinePath, `artifacts/${timelineName}`, "application/x-ndjson")];
  return { schema_version: OBSERVATION_SCHEMA, driver_id: DRIVER_ID, run_id: runId, scenario_id: scenarioId, started_at: startedAt, ended_at: new Date().toISOString(), mutation_started: true, prerequisites: [{ id: "CELLD_NETWORK_AUTH", status: "available", reason_code: "CELLD_NETWORK_AUTH_READY" }, { id: "CELLD_PRIVATE_FLEET", status: "available", reason_code: "CELLD_PRIVATE_FLEET_READY" }], assertions: campaign.assertions.map((item) => ({ ...item, evidence_refs: artifacts.map((entryArtifact) => entryArtifact.path) })), identities: { profile_id: profile.profile_id, sandbox_git: profile.expected_sandbox_git, environment_host_sha256: profile.environment.host_sha256, driver_version: DRIVER_VERSION }, metrics: campaign.metrics, faults: campaign.faults, artifacts, cleanup: { status: cleanupStatus, assertions: cleanupAssertions } };
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

if (process.argv[1] && SCRIPT_PATH === resolve(process.argv[1])) main(process.argv.slice(2)).catch((error) => { process.stderr.write(`CELLD_NETWORK_AUTH_DRIVER_ERROR ${sha256(error.message)}\n`); process.exitCode = 3; });
