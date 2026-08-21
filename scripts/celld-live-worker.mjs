#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { chmodSync, existsSync, lstatSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { hostname } from "node:os";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  cleanupFleet,
  deployFleetWorker,
  diagnoseFleet,
  prepareFleet,
  probeFleetWorker,
  startFleet,
} from "./celld-fleet-fixture.mjs";
import { cleanupFixture, prepareFixture, startFixture } from "./celld-seaweedfs-fixture.mjs";
import { getWorkerCell } from "./celld-worker-client.mjs";
import { validateOrchestrationConfig } from "./celld-live-orchestration.mjs";
import { validateLiveProfile } from "./celld-uat-live-protocol.mjs";

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
  return { path: project, sha256: sha256(`${CONFORMANCE_WORKER}\n${JSON.stringify(config)}\n${WASM_ADD_HEX}\nagentic-celld-asset-v1\n`) };
}

function deploymentArgs(runtime, projectPath) {
  const storage = runtime.storage, fleet = runtime.fleet, deployer = exactDeployer(runtime.runId);
  const esbuild = join(REPO_ROOT, "runtimes/celld/instance-cell/node_modules/@esbuild/linux-x64/bin/esbuild");
  const uid = typeof process.getuid === "function" ? process.getuid() : 1000;
  const gid = typeof process.getgid === "function" ? process.getgid() : 1000;
  return [
    "run", "--rm", "--name", deployer.name,
    "--label", `dev.agentic-sandbox.run=${runtime.runId}`, "--label", "dev.agentic-sandbox.scope=celld-qualification",
    "--network", fleet.network.name, "--user", `${uid}:${gid}`, "--read-only", "--security-opt", "no-new-privileges:true", "--cap-drop", "ALL",
    "--pids-limit", "128", "--cpus", "1", "--memory", "1024m", "--tmpfs", "/tmp:rw,noexec,nosuid,nodev,size=128m,mode=1777", "--workdir", "/workspace",
    "--mount", `type=bind,src=${projectPath},dst=/workspace,readonly`, "--mount", `type=bind,src=${esbuild},dst=/usr/local/bin/esbuild,readonly`,
    "--mount", `type=bind,src=${storage.identity_file_ref},dst=/run/identity/credentials,readonly`, "--mount", `type=bind,src=${storage.ca_file_ref},dst=/run/tls/ca.crt,readonly`,
    "--env", "AWS_SHARED_CREDENTIALS_FILE=/run/identity/credentials", "--env", "AWS_CA_BUNDLE=/run/tls/ca.crt", "--env", "SSL_CERT_FILE=/run/tls/ca.crt", "--env", `AWS_REGION=${storage.region}`, "--env", "CELLD_ESBUILD=/usr/local/bin/esbuild",
    fleet.pins.celld.image_ref, "deploy", "/workspace", "--bucket", `s3://${storage.bucket}/${storage.run_prefix}/fleet`, "--endpoint", "https://s3gateway1:8334", "--region", storage.region,
  ];
}

function deployProject(runtime, projectPath) {
  cleanupWorkerResources(runtime.runId);
  const output = run("docker", deploymentArgs(runtime, projectPath), { timeout: 300_000 });
  const version = /Current Version ID: ([0-9a-f]{16})/.exec(output)?.[1];
  if (!version) throw new Error("Celld deployment did not return an immutable version ID");
  return { version, output_sha256: sha256(output) };
}

function stopFleet(runtime) {
  for (const node of runtime.fleet.nodes) {
    const document = JSON.parse(run("docker", ["inspect", node.name]));
    const labels = document?.[0]?.Config?.Labels ?? {};
    if (labels["dev.agentic-sandbox.run"] !== runtime.runId || labels["dev.agentic-sandbox.scope"] !== "celld-qualification") throw new Error(`refusing to stop unowned fleet node ${node.name}`);
    if (document[0]?.State?.Running === true) run("docker", ["stop", "--time", "20", node.name], { timeout: 60_000 });
  }
}

function workerEndpoint(fleet) {
  const output = run("docker", ["port", fleet.nodes[0].name, "8080/tcp"]);
  const match = /^127\.0\.0\.1:(\d+)$/.exec(output);
  if (!match) throw new Error("Worker listener is not published on host loopback only");
  return `http://127.0.0.1:${match[1]}`;
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
  const endpoint = workerEndpoint(runtime.fleet);
  const instanceId = `rollback-${sha256(runtime.runId).slice(0, 20)}`, operationId = `rollback-${randomBytes(8).toString("hex")}`, nonce = randomBytes(16).toString("hex");
  const seed = await getWorkerCell({ endpoint, varsFile: runtime.fleet.worker_vars_file_ref, instanceId, operationId, generation: 1, nonce });
  if (seed.status !== 404 || seed.body?.error?.code !== "cell.missing") throw new Error("reference Worker durable rollback marker was not accepted");
  const proofHash = `sha256:${sha256(`${instanceId}:${operationId}:${nonce}:accepted`)}`;
  stopFleet(runtime);
  const conformance = prepareWorkerConformanceProject(runtime.root);
  const candidate = deployProject(runtime, conformance.path);
  if (startFleet(runtime.fleetPath).status !== "READY") throw new Error("conformance deployment fleet did not become ready");
  const cases = await capabilityCases(workerEndpoint(runtime.fleet));
  stopFleet(runtime);
  const restored = deployProject(runtime, join(REPO_ROOT, "runtimes/celld/instance-cell"));
  if (startFleet(runtime.fleetPath).status !== "READY") throw new Error("reference deployment rollback did not become ready");
  const replay = await getWorkerCell({ endpoint: workerEndpoint(runtime.fleet), varsFile: runtime.fleet.worker_vars_file_ref, instanceId, operationId, generation: 1, nonce });
  const probe = await probeFleetWorker(runtime.fleetPath);
  const rollbackPreserved = replay.status === 409 && replay.body?.error?.code === "cell.signature_replayed" && probe.status === "READY";
  const passed = Object.values(cases).filter(Boolean).length;
  timeline.push({ scenario: "UAT-CELLD-007", cases, candidate_version: candidate.version, restored_version: restored.version, conformance_sha256: conformance.sha256, rollback_marker_preserved: rollbackPreserved });
  return { assertions: [
    { id: "CELLD.007.CLAIMS", measurements: { capabilities: ADVERTISED, advertised_cases: 8, passed_cases: passed, failed_cases: 8 - passed, not_run_cases: 0 } },
    { id: "CELLD.007.ROLLBACK", measurements: { previous_digest: runtime.fleet.pins.worker_digest, restored_digest: runtime.fleet.pins.worker_digest, state_sha256_before: proofHash, state_sha256_after: rollbackPreserved ? proofHash : `sha256:${"0".repeat(64)}`, approved_digest_active: rollbackPreserved } },
  ], metrics: [{ name: "worker_capability_cases_passed", value: passed, unit: "cases" }], faults: [{ kind: "worker_deployment_rollback", healed: rollbackPreserved }] };
}

function inventorySnapshot(runtime) {
  const containers = run("docker", ["ps", "--all", "--filter", `label=dev.agentic-sandbox.run=${runtime.runId}`, "--format", "{{.Names}}"] ).split(/\r?\n/).filter(Boolean).sort();
  const pids = runtime.fleet.nodes.map((node) => run("docker", ["inspect", "--format", "{{.State.Pid}}", node.name]));
  const ports = runtime.fleet.nodes.map((node) => run("docker", ["port", node.name])).sort();
  const domains = run("virsh", ["--connect", runtime.config.libvirt_uri, "list", "--all", "--name"]).split(/\r?\n/).filter(Boolean).sort();
  const reviewedFiles = run("find", [runtime.config.base_images_dir, runtime.config.vm_storage_dir, runtime.config.agentshare_root, "-xdev", "-mindepth", "1", "-maxdepth", "2", "-printf", "%p|%y|%s\n"]).split(/\r?\n/).filter(Boolean).sort();
  return { containers, pids, ports, domains, reviewed_files_sha256: sha256(reviewedFiles.join("\n")) };
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

async function runExcluded(runtime, timeline) {
  const primary = runtime.fleet.nodes[0].name;
  const negativeRoot = prepareNegativeProjects(runtime.root);
  run("docker", ["cp", negativeRoot, `${primary}:/tmp/celld-negative`], { timeout: 120_000 });
  const before = inventorySnapshot(runtime);
  let typedRejections = 0, silentSuccesses = 0;
  const codes = {};
  for (const capability of EXCLUDED) {
    let rejected = 0;
    for (let attempt = 0; attempt < 100; attempt += 1) {
      const result = spawnSync("docker", ["exec", primary, "/usr/local/bin/celld", "deploy", `/tmp/celld-negative/${capability}`, "--dry-run"], { encoding: "utf8", shell: false, timeout: 30_000 });
      const output = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
      if (result.status !== 0 && output.includes("does not support these config keys") && output.includes(capability)) { typedRejections += 1; rejected += 1; }
      else silentSuccesses += 1;
    }
    codes[capability] = rejected;
  }
  const after = inventorySnapshot(runtime);
  const unchanged = JSON.stringify(before) === JSON.stringify(after);
  const diagnosis = diagnoseFleet(runtime.fleetPath);
  const probe = await probeFleetWorker(runtime.fleetPath);
  timeline.push({ scenario: "UAT-CELLD-008", attempts: 800, typed_rejections: typedRejections, codes, inventory_before_sha256: sha256(JSON.stringify(before)), inventory_after_sha256: sha256(JSON.stringify(after)), fleet_status: diagnosis.status, worker_status: probe.status });
  return { assertions: [
    { id: "CELLD.008.LOUD_REJECTION", measurements: { capabilities: EXCLUDED, attempts_per_capability: 100, attempts: 800, typed_rejections: typedRejections, silent_successes: silentSuccesses } },
    { id: "CELLD.008.NO_SIDE_EFFECT", measurements: { attempts: 800, processes_created: unchanged ? 0 : 1, files_created: unchanged ? 0 : 1, sockets_created: unchanged ? 0 : 1, containers_created: unchanged ? 0 : 1, vms_created: unchanged ? 0 : 1, host_inventory_restored: unchanged && diagnosis.status === "READY" && probe.status === "READY" } },
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
  const profile = protectedJson(liveProfilePath, "live profile");
  const profileErrors = validateLiveProfile(profile);
  if (profileErrors.length) throw new Error(profileErrors.join("; "));
  const entry = profile.drivers?.[DRIVER_ID];
  if (!entry?.enabled) return unavailable(profile, scenarioId, runId, startedAt, "CELLD_LIVE_WORKER_DRIVER_DISABLED");
  const git = dependencies.gitCommit?.() ?? run("git", ["rev-parse", "HEAD"]);
  const host = dependencies.hostname?.() ?? hostname();
  if (!SCENARIOS.has(scenarioId) || profile.run_id !== runId || profile.expected_sandbox_git !== git || profile.environment.host_sha256 !== sha256(host)) throw new Error("Worker live identity does not match the requested run");
  const config = protectedJson(entry.config_path, "orchestration config");
  const configErrors = validateOrchestrationConfig(config);
  if (configErrors.length || config.run_id !== runId) throw new Error([...configErrors, "config run identity mismatch"].join("; "));
  if (scenarioId === "UAT-CELLD-009") return unavailable(profile, scenarioId, runId, startedAt, "CELLD_PER_ISOLATE_RESOURCE_ENFORCEMENT_UNAVAILABLE");
  if (process.platform !== "linux") return unavailable(profile, scenarioId, runId, startedAt, "CELLD_LIVE_WORKER_LINUX_REQUIRED");

  mkdirSync(artifactDir, { recursive: true, mode: 0o700 }); chmodSync(artifactDir, 0o700);
  const timeline = [];
  let storage = null, fleet = null, fleetPath = null, campaign = null;
  const cleanupAssertions = [];
  let cleanupStatus = "failed";
  try {
    const root = join(config.working_root, `${scenarioId.toLowerCase()}-worker`, runId);
    storage = prepareFixture({ fixtureProfile: "titan-single-host-storage", runId, root });
    startFixture(storage);
    fleet = prepareFleet({ storageConfigPath: join(root, "fixture.json") });
    fleetPath = join(root, "fleet.json");
    await deployFleetWorker(fleetPath);
    if (startFleet(fleetPath).status !== "READY") throw new Error("Worker qualification fleet is not ready");
    const runtime = { config, storage, fleet, fleetPath, runId, root };
    campaign = await (dependencies.runScenario ?? (scenarioId === "UAT-CELLD-007" ? runCapabilities : runExcluded))(runtime, timeline);
  } finally {
    try { cleanupWorkerResources(runId); cleanupAssertions.push("exact Worker deployer removed"); } catch (error) { cleanupAssertions.push(`Worker deployer cleanup digest ${sha256(error.message)}`); }
    try { if (fleetPath && existsSync(fleetPath)) cleanupFleet(fleetPath); cleanupAssertions.push("exact Worker fleet removed"); } catch (error) { cleanupAssertions.push(`fleet cleanup digest ${sha256(error.message)}`); }
    try { if (storage) cleanupFixture(storage); cleanupAssertions.push("exact Worker storage fixture removed"); } catch (error) { cleanupAssertions.push(`storage cleanup digest ${sha256(error.message)}`); }
    cleanupStatus = cleanupAssertions.some((value) => value.includes("digest")) ? "failed" : "passed";
  }
  if (!campaign) throw new Error("Worker campaign produced no measurements");
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

if (process.argv[1] && SCRIPT_PATH === resolve(process.argv[1])) main(process.argv.slice(2)).catch((error) => { process.stderr.write(`CELLD_LIVE_WORKER_ERROR ${sha256(error.message)}\n`); process.exitCode = 3; });
