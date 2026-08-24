#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { chmodSync, closeSync, existsSync, fsyncSync, lstatSync, mkdirSync, openSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { CELLD_MIGRATION_SCOPE, REQUIRED_WRITER_CLASSES, rehearseOfflineMigration } from "./celld-offline-migration.mjs";
import { cleanupFleet, deployFleetWorker, diagnoseFleet, prepareFleet, probeFleetWorker, startFleet, stopFleetForWorkerDeployment } from "./celld-fleet-fixture.mjs";
import { cleanupFixture, fixtureEnvironment, prepareFixture, startFixture, validateFixtureConfig } from "./celld-seaweedfs-fixture.mjs";
import { openStorageGatewayAccess } from "./celld-storage-gateway-access.mjs";
import { runS3Qualification } from "./celld-storage-race-runner.mjs";
import { evaluateStorageEvidence, S3V1Client, STORAGE_PROFILE_SCHEMA } from "./celld-storage-qualifier.mjs";
import { sendWorkerCommand } from "./celld-worker-client.mjs";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const RUN_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const COPYABLE_HEADERS = new Set(["cache-control", "content-disposition", "content-encoding", "content-language", "content-type", "expires", "x-amz-storage-class", "x-amz-website-redirect-location"]);
const MIGRATION_JOURNAL_SCHEMA = "agentic-sandbox.celld-migration-journal/v1";
const FAILURE_STAGE = /^[a-z][a-z0-9_-]{0,63}$/;
const LIVE_MIGRATION_FLEET_READINESS_POLICY = Object.freeze({
  maxAttempts: 20,
  deadlineMs: 60_000,
  backoffMs: 750,
});

function sha256(value) { return createHash("sha256").update(value).digest("hex"); }
function sleep(ms) { return new Promise((resolvePromise) => setTimeout(resolvePromise, ms)); }
function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;
  return JSON.stringify(value);
}

function run(program, args, options = {}) {
  const result = spawnSync(program, args, { encoding: "utf8", shell: false, ...options });
  if (result.error || result.status !== 0) throw new Error(`${basename(program)} failed: ${(result.error?.message ?? result.stderr ?? "").trim()}`);
  return result.stdout.trim();
}

function compose(config, args, timeout = 600_000) {
  return run("docker", ["compose", "-f", config.compose_file, "-p", config.project, ...args], { env: fixtureEnvironment(config), timeout });
}

function protectedJson(path, description) {
  if (!isAbsolute(path) || !existsSync(path)) throw new Error(`${description} is missing`);
  const metadata = lstatSync(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) throw new Error(`${description} must be a protected regular non-symlink file`);
  return JSON.parse(readFileSync(path, "utf8"));
}

function argument(args, name) {
  const index = args.indexOf(name);
  if (index < 0 || !args[index + 1]) throw new Error(`${name} is required`);
  return args[index + 1];
}

function storageProfile(config, identityFileRef, gateways) {
  if (!Array.isArray(gateways) || gateways.length === 0) throw new Error("migration storage gateway access is unavailable");
  return {
    schema_version: STORAGE_PROFILE_SCHEMA, profile_id: config.project, run_id: config.run_id, dialect: "s3-v1", scope: "live_candidate",
    endpoint: gateways[0], region: config.region, addressing_mode: "path", bucket: config.bucket, run_prefix: config.run_prefix,
    identity_file_ref: identityFileRef, ca_file_ref: config.ca_file_ref,
    backend: { product: config.backend.product, version: config.backend.version, artifact_sha256: config.backend.artifact_sha256, configuration_sha256: config.backend.configuration_sha256, gateway_endpoints: gateways, topology: config.backend.topology },
    limits: config.limits,
  };
}

function writeArtifact(path, body) {
  const bytes = Buffer.isBuffer(body) ? body : Buffer.from(body);
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  writeFileSync(path, bytes, { mode: 0o600, flag: "wx" });
  chmodSync(path, 0o600);
  return { path: `artifacts/${basename(path)}`, sha256: sha256(bytes), bytes: bytes.length };
}

function atomicProtectedJson(path, value) {
  const temporary = `${path}.new`;
  writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600, flag: "wx" });
  chmodSync(temporary, 0o600);
  syncFile(temporary);
  renameSync(temporary, path);
  chmodSync(path, 0o600);
  syncFile(path);
  syncDirectory(dirname(path));
}

function syncFile(path) {
  const descriptor = openSync(path, "r");
  try { fsyncSync(descriptor); } finally { closeSync(descriptor); }
}

function syncDirectory(path) {
  const descriptor = openSync(path, "r");
  try { fsyncSync(descriptor); } finally { closeSync(descriptor); }
}

export class LiveMigrationJournal {
  constructor(path, { runId, destinationRunId, now = () => new Date() }) {
    if (!RUN_ID.test(runId ?? "") || !RUN_ID.test(destinationRunId ?? "") || runId === destinationRunId || path !== join(`/dev/shm/agentic-celld-storage/${runId}`, "migration-journal.json")) throw new Error("migration journal identity is invalid");
    if (existsSync(path) || existsSync(`${path}.new`)) throw new Error("migration journal already exists; prior outcome requires exact recovery");
    this.path = path;
    this.runId = runId;
    this.destinationRunId = destinationRunId;
    this.now = now;
    const document = {
      schema_version: MIGRATION_JOURNAL_SCHEMA,
      run_id: runId,
      destination_run_id: destinationRunId,
      scope: CELLD_MIGRATION_SCOPE,
      created_at: now().toISOString(),
      entries: [],
    };
    writeFileSync(path, `${JSON.stringify(document, null, 2)}\n`, { mode: 0o600, flag: "wx" });
    chmodSync(path, 0o600);
    syncFile(path);
    syncDirectory(dirname(path));
  }

  load() {
    const document = protectedJson(this.path, "migration journal");
    if (document.schema_version !== MIGRATION_JOURNAL_SCHEMA || document.run_id !== this.runId || document.destination_run_id !== this.destinationRunId || document.scope !== CELLD_MIGRATION_SCOPE || !Array.isArray(document.entries)) throw new Error("migration journal header is invalid");
    let previous = null;
    for (const [index, entry] of document.entries.entries()) {
      const { entry_sha256: observed, ...hashed } = entry ?? {};
      if (entry.sequence !== index + 1 || entry.previous_entry_sha256 !== previous || !/^[0-9a-f]{64}$/.test(observed ?? "") || sha256(canonical(hashed)) !== observed) throw new Error("migration journal hash chain is invalid");
      previous = observed;
    }
    return document;
  }

  append(entry) {
    const document = this.load();
    const previous = document.entries.at(-1)?.entry_sha256 ?? null;
    const base = { sequence: document.entries.length + 1, ...entry, recorded_at: this.now().toISOString(), previous_entry_sha256: previous };
    const record = { ...base, entry_sha256: sha256(canonical(base)) };
    document.entries.push(record);
    atomicProtectedJson(this.path, document);
    return record;
  }

  plan({ phase, mutation, details }) {
    if (typeof phase !== "string" || typeof mutation !== "string" || !details || typeof details !== "object" || Array.isArray(details)) throw new Error("migration journal plan is invalid");
    const document = this.load();
    const completed = new Set(document.entries.filter((entry) => entry.event === "completed").map((entry) => entry.plan_id));
    if (document.entries.some((entry) => entry.event === "planned" && !completed.has(entry.id))) throw new Error("migration journal contains an incomplete prior mutation");
    return this.append({ id: `mutation-${document.entries.length + 1}`, event: "planned", status: "planned", phase, mutation, details });
  }

  complete(planId, details) {
    const document = this.load();
    const plan = document.entries.find((entry) => entry.event === "planned" && entry.id === planId);
    if (!plan || document.entries.some((entry) => entry.event === "completed" && entry.plan_id === planId) || !details || typeof details !== "object" || Array.isArray(details)) throw new Error("migration journal completion is invalid");
    return this.append({ id: `completion-${document.entries.length + 1}`, event: "completed", status: "completed", plan_id: planId, phase: plan.phase, mutation: plan.mutation, details });
  }

  evidence() {
    const document = this.load();
    const completed = new Set(document.entries.filter((entry) => entry.event === "completed").map((entry) => entry.plan_id));
    const incomplete = document.entries.filter((entry) => entry.event === "planned" && !completed.has(entry.id)).map((entry) => entry.id);
    return { ...document, journal_sha256: sha256(canonical(document)), incomplete_plan_ids: incomplete };
  }
}

export function buildMigrationFailureEvidence({ sourceStorage, destinationStorage, destinationRunId, startedAt, endedAt, sandboxGit, operationError, cleanupErrors, journalEvidence, journalErrorSha256, retainedNamespaces, failureStage = "unknown" }) {
  if (!sourceStorage || !RUN_ID.test(sourceStorage.run_id ?? "") || !RUN_ID.test(destinationRunId ?? "") || !/^[0-9a-f]{40}$/.test(sandboxGit ?? "") || !Array.isArray(cleanupErrors) || typeof retainedNamespaces !== "boolean" || !FAILURE_STAGE.test(failureStage)) throw new Error("migration failure evidence inputs are invalid");
  const lastJournalEntry = journalEvidence?.entries?.at(-1) ?? null;
  return {
    schema_version: "agentic-sandbox.celld-offline-migration-error/v1",
    run_id: sourceStorage.run_id,
    destination_run_id: destinationRunId,
    started_at: startedAt,
    ended_at: endedAt,
    sandbox_git: sandboxGit,
    source_backend: sourceStorage.backend,
    destination_backend: destinationStorage?.backend ?? null,
    source_namespace_sha256: sha256(`${sourceStorage.bucket}/${sourceStorage.run_prefix}`),
    destination_namespace_sha256: destinationStorage ? sha256(`${destinationStorage.bucket}/${destinationStorage.run_prefix}`) : null,
    error_sha256: operationError ? sha256(operationError.message) : null,
    failure_stage: failureStage,
    operation_context: sanitizeOperationError(operationError, failureStage),
    cleanup_errors: cleanupErrors,
    migration_journal: journalEvidence,
    journal_error_sha256: journalErrorSha256,
    last_phase: lastJournalEntry?.phase ?? null,
    last_mutation: lastJournalEntry?.mutation ?? null,
    retain_namespaces: retainedNamespaces,
    retained_state: retainedNamespaces ? "both object-store namespaces stopped and write-denied for operator recovery" : "none",
    storage_boundary: { migrated: "Celld object-store namespace only", sandbox_local_storage: "not_targeted" },
  };
}

export function summarizeDestinationQualificationRows(rows, limits) {
  if (!Array.isArray(rows) || !limits) throw new Error("destination qualification raw evidence is invalid");
  const rounds = { create: new Set(), overwrite: new Set() };
  const denials = new Set();
  const requiredDenials = new Set(["invalid_identity", "expired_identity", "wrong_bucket", "cross_bucket"]);
  for (const row of rows) {
    if (row?.family === "create" || row?.family === "overwrite") {
      const ceiling = limits[`${row.family}_rounds`];
      if (!Number.isSafeInteger(row.round) || row.round < 0 || row.round >= ceiling || rounds[row.family].has(row.round)) throw new Error("destination qualification raw rounds are incomplete or duplicated");
      rounds[row.family].add(row.round);
    } else if (row?.family === "denial") {
      if (!requiredDenials.has(row.class) || denials.has(row.class)) throw new Error("destination qualification denial evidence is incomplete or duplicated");
      denials.add(row.class);
    } else {
      throw new Error("destination qualification raw evidence contains an unknown family");
    }
  }
  if (rounds.create.size !== limits.create_rounds || rounds.overwrite.size !== limits.overwrite_rounds || denials.size !== requiredDenials.size) throw new Error("destination qualification raw evidence is incomplete or duplicated");
  return { create_rows: rounds.create.size, overwrite_rows: rounds.overwrite.size, denial_rows: denials.size, total_rows: rows.length };
}

export function sanitizeOperationError(error, failureStage) {
  if (!error) return null;
  const context = {
    failure_stage: FAILURE_STAGE.test(failureStage ?? "") ? failureStage : "unknown",
    name: typeof error.name === "string" && error.name ? error.name : "Error",
    message_sha256: sha256(error.message ?? ""),
  };
  if (typeof error.program === "string" && /^[A-Za-z0-9_.-]{1,64}$/.test(error.program)) context.program = error.program;
  if (typeof error.operation === "string" && /^[a-z][a-z0-9_]{0,63}$/.test(error.operation)) context.operation = error.operation;
  if (Number.isInteger(error.exitStatus)) context.exit_status = error.exitStatus;
  if (typeof error.signal === "string" && /^[A-Z0-9_-]{1,32}$/.test(error.signal)) context.signal = error.signal;
  if (typeof error.errorCode === "string" && /^[A-Z0-9_-]{1,64}$/.test(error.errorCode)) context.error_code = error.errorCode;
  if (typeof error.timedOut === "boolean") context.timed_out = error.timedOut;
  if (/^[0-9a-f]{64}$/.test(error.stdoutSha256 ?? "")) context.stdout_sha256 = error.stdoutSha256;
  if (/^[0-9a-f]{64}$/.test(error.stderrSha256 ?? "")) context.stderr_sha256 = error.stderrSha256;
  const fleetDiagnosis = sanitizeFleetDiagnosisEvidence(error.evidence);
  if (fleetDiagnosis) context.fleet_diagnosis = fleetDiagnosis;
  return context;
}

function sanitizeFleetDiagnosisEvidence(evidence) {
  if (!evidence || typeof evidence !== "object" || Array.isArray(evidence)) return null;
  const failure = evidence.failure && typeof evidence.failure === "object" && !Array.isArray(evidence.failure)
    ? evidence.failure
    : {};
  const membership = evidence.membership && typeof evidence.membership === "object" && !Array.isArray(evidence.membership)
    ? evidence.membership
    : {};
  const nodes = Array.isArray(evidence.nodes) ? evidence.nodes : [];
  const sanitized = {
    schema_version: evidence.schema_version === "agentic-sandbox.celld-fleet-diagnosis/v1" ? evidence.schema_version : undefined,
    status: evidence.status === "READY" || evidence.status === "NOT_READY" ? evidence.status : undefined,
    reason_code: evidence.reason_code === "CELLD_FLEET_STARTUP_NOT_READY" ? evidence.reason_code : undefined,
    retryable: typeof evidence.retryable === "boolean" ? evidence.retryable : undefined,
    membership: {
      expected: Number.isSafeInteger(membership.expected) ? membership.expected : undefined,
      running: Number.isSafeInteger(membership.running) ? membership.running : undefined,
      reserve: Number.isSafeInteger(membership.reserve) ? membership.reserve : undefined,
      probe: membership.probe === "passed" || membership.probe === "failed" ? membership.probe : undefined,
      attempts: Number.isSafeInteger(membership.attempts) ? membership.attempts : undefined,
      probe_sha256: /^[0-9a-f]{64}$/.test(membership.probe_sha256 ?? "") ? membership.probe_sha256 : undefined,
    },
    failure: {
      attempts: Number.isSafeInteger(failure.attempts) ? failure.attempts : undefined,
      max_attempts: Number.isSafeInteger(failure.max_attempts) ? failure.max_attempts : undefined,
      deadline_ms: Number.isSafeInteger(failure.deadline_ms) ? failure.deadline_ms : undefined,
      backoff_ms: Number.isSafeInteger(failure.backoff_ms) ? failure.backoff_ms : undefined,
      reason_code: typeof failure.reason_code === "string" && /^CELLD_[A-Z0-9_]{1,96}$/.test(failure.reason_code) ? failure.reason_code : undefined,
      evidence_sha256: /^[0-9a-f]{64}$/.test(failure.evidence_sha256 ?? "") ? failure.evidence_sha256 : undefined,
      expected_node_ids_sha256: /^[0-9a-f]{64}$/.test(failure.expected_node_ids_sha256 ?? "") ? failure.expected_node_ids_sha256 : undefined,
    },
    nodes: {
      total: nodes.length,
      running: nodes.filter((node) => node?.running === true).length,
      public_endpoints: nodes.filter((node) => typeof node?.public_endpoint === "string" && node.public_endpoint.length > 0).length,
      reserve_running: nodes.filter((node) => node?.role === "reserve" && node?.running === true).length,
    },
  };
  return JSON.parse(JSON.stringify(sanitized));
}

function decodeXml(value) {
  return value.replace(/<!\[CDATA\[([\s\S]*?)\]\]>/g, "$1").replace(/&#(\d+);/g, (_, code) => String.fromCodePoint(Number(code))).replace(/&#x([0-9a-f]+);/gi, (_, code) => String.fromCodePoint(Number.parseInt(code, 16))).replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, "\"").replace(/&apos;/g, "'").replace(/&amp;/g, "&");
}

function tag(block, name) {
  const match = new RegExp(`<${name}>([\\s\\S]*?)</${name}>`).exec(block);
  return match ? decodeXml(match[1]) : null;
}

function copyableMetadata(headers) {
  return Object.fromEntries(Object.entries(headers ?? {}).flatMap(([rawName, rawValue]) => {
    const name = rawName.toLowerCase();
    if (!COPYABLE_HEADERS.has(name) && !name.startsWith("x-amz-meta-")) return [];
    const value = Array.isArray(rawValue) ? rawValue.join(",") : String(rawValue);
    return [[name, value]];
  }).sort(([left], [right]) => left.localeCompare(right)));
}

export class LiveS3MigrationStore {
  constructor(config, options = {}) {
    this.id = config.project;
    this.scope = CELLD_MIGRATION_SCOPE;
    this.config = config;
    this.profile = options.profile ?? storageProfile(config, config.admin_identity_file_ref, options.gatewayEndpoints);
    this.client = options.client ?? new S3V1Client(this.profile);
  }

  async ensureBucket() {
    const result = await this.client.createBucket(this.config.bucket);
    if (result.status >= 200 && result.status < 300) return;
    if (result.status === 409 && await this.ready()) return;
    throw new Error(`migration bucket create returned ${result.status}`);
  }

  async ready() {
    try { return (await this.client.listPrefix()).status === 200; } catch { return false; }
  }

  async list() {
    const entries = [];
    const keys = new Set();
    const tokens = new Set();
    let continuationToken = null;
    do {
      const query = { "list-type": 2, prefix: `${this.profile.run_prefix}/`, "max-keys": 1000 };
      if (continuationToken) query["continuation-token"] = continuationToken;
      const response = await this.client.request("GET", "", { includePrefix: false, query });
      if (response.status !== 200) throw new Error(`migration list returned ${response.status}`);
      const xml = response.body.toString("utf8");
      for (const match of xml.matchAll(/<Contents>([\s\S]*?)<\/Contents>/g)) {
        const fullKey = tag(match[1], "Key");
        const prefix = `${this.profile.run_prefix}/`;
        if (!fullKey?.startsWith(prefix) || fullKey.length === prefix.length) throw new Error("migration listing escaped the exact run prefix");
        const key = fullKey.slice(prefix.length);
        if (keys.has(key)) throw new Error("migration listing duplicated a key");
        keys.add(key);
        const head = await this.client.head(key);
        if (head.status !== 200 || !/^\d+$/.test(String(head.headers["content-length"] ?? ""))) throw new Error(`migration HEAD failed for ${sha256(key)}`);
        entries.push({ key, size: Number(head.headers["content-length"]), metadata: copyableMetadata(head.headers) });
      }
      continuationToken = tag(xml, "IsTruncated") === "true" ? tag(xml, "NextContinuationToken") : null;
      if (tag(xml, "IsTruncated") === "true" && !continuationToken) throw new Error("migration listing omitted its continuation token");
      if (continuationToken && tokens.has(continuationToken)) throw new Error("migration listing repeated its continuation token");
      if (continuationToken) tokens.add(continuationToken);
    } while (continuationToken);
    return entries;
  }

  async get(key) {
    const response = await this.client.get(key);
    if (response.status !== 200) throw new Error(`migration GET failed for ${sha256(key)}`);
    return { body: response.body, metadata: copyableMetadata(response.headers) };
  }

  async put(key, body, metadata) {
    const response = await this.client.put(key, body, { headers: metadata });
    if (response.status < 200 || response.status >= 300) throw new Error(`migration PUT failed for ${sha256(key)}`);
  }

  async delete(key) {
    const response = await this.client.delete(key);
    if (response.status < 200 || response.status >= 300) throw new Error(`migration DELETE failed for ${sha256(key)}`);
  }

  close() { this.client.close(); }
}

function inspect(name) {
  const result = spawnSync("docker", ["container", "inspect", name], { encoding: "utf8", shell: false, timeout: 30_000 });
  if (result.status !== 0) return null;
  return JSON.parse(result.stdout);
}

function assertOwned(document, runId, name) {
  const labels = document?.[0]?.Config?.Labels ?? {};
  if (labels["dev.agentic-sandbox.run"] !== runId || labels["dev.agentic-sandbox.scope"] !== "celld-qualification") throw new Error(`refusing foreign migration container ${name}`);
}

function runningFleetNodes(fleet) {
  let running = 0;
  for (const node of fleet.nodes) {
    const document = inspect(node.name);
    if (!document) continue;
    assertOwned(document, fleet.run_id, node.name);
    if (document[0]?.State?.Running === true) running += 1;
  }
  return running;
}

function stopAuthorityFleet(authority) {
  const present = authority.fleet.nodes.filter((node) => inspect(node.name) !== null).length;
  if (present === 0) return;
  if (present !== authority.fleet.nodes.length) throw new Error("migration fleet is only partially present");
  const result = stopFleetForWorkerDeployment(authority.fleetPath);
  if (result.status !== "STOPPED" || result.nodes.length !== authority.fleet.nodes.length) throw new Error("migration fleet did not reach the inventoried stopped state");
}

function bucketPolicyObservation(config) {
  const value = protectedJson(join(config.run_root, "s3.json"), "SeaweedFS identity policy");
  const identity = value.identities?.find((candidate) => candidate.name === "run-bucket");
  if (!identity || !Array.isArray(identity.actions)) throw new Error("run-bucket identity policy is missing");
  const allowed = new Set([`Read:${config.bucket}`, `List:${config.bucket}`, `Tagging:${config.bucket}`, `Write:${config.bucket}`]);
  if (identity.actions.some((action) => !allowed.has(action)) || new Set(identity.actions).size !== identity.actions.length) throw new Error("run-bucket identity policy exceeds the reviewed scope");
  return { writable: identity.actions.includes(`Write:${config.bucket}`), policy_sha256: sha256(canonical({ name: identity.name, actions: identity.actions })) };
}

async function setBucketWrite(config, writable, store) {
  const path = join(config.run_root, "s3.json");
  const value = protectedJson(path, "SeaweedFS identity policy");
  const identity = value.identities?.find((candidate) => candidate.name === "run-bucket");
  if (!identity || !Array.isArray(identity.actions)) throw new Error("run-bucket identity policy is missing");
  const allowed = new Set([`Read:${config.bucket}`, `List:${config.bucket}`, `Tagging:${config.bucket}`, `Write:${config.bucket}`]);
  if (identity.actions.some((action) => !allowed.has(action))) throw new Error("run-bucket identity policy exceeds the reviewed scope");
  const desired = [`Read:${config.bucket}`, `List:${config.bucket}`, `Tagging:${config.bucket}`, ...(writable ? [`Write:${config.bucket}`] : [])];
  if (JSON.stringify(identity.actions) === JSON.stringify(desired)) return;
  identity.actions = desired;
  const temporary = `${path}.migration-${randomBytes(8).toString("hex")}`;
  writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600, flag: "wx" });
  chmodSync(temporary, 0o600);
  renameSync(temporary, path);
  compose(config, ["restart", "s3gateway1", "s3gateway2"], 120_000);
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (await store.ready()) return;
    await sleep(200);
  }
  throw new Error("SeaweedFS gateway did not recover after identity-policy reload");
}

function workerEndpoint(fleet) {
  const output = run("docker", ["port", fleet.nodes[0].name, "8080/tcp"]);
  const match = /^127\.0\.0\.1:(\d+)$/.exec(output);
  if (!match) throw new Error("migration Worker listener is not loopback-only");
  return `http://127.0.0.1:${match[1]}`;
}

export class LiveMigrationControl {
  constructor(source, destination, journal) {
    this.authorities = new Map([[source.store.id, source], [destination.store.id, destination]]);
    this.source = source;
    this.destination = destination;
    this.journal = journal;
    if (!(journal instanceof LiveMigrationJournal)) throw new Error("live migration control requires the exact durable journal");
  }

  async stopAllWriters() { for (const authority of this.authorities.values()) stopAuthorityFleet(authority); }

  async listWriters() {
    const observations = await this.observeAuthorities();
    const observedAt = new Date().toISOString();
    return REQUIRED_WRITER_CLASSES.map((writerClass) => ({
      class: writerClass,
      running: observations.some((observation) => observation.running_writer_classes.includes(writerClass)),
      observed_at: observedAt,
      observation_source: writerClass === "deployment_cli" ? "exact-owned Docker deployer inspection"
        : writerClass === "management_reconciler" ? "qualification lifecycle excludes a management reconciler process"
          : "exact-owned Celld fleet container inspection",
    }));
  }

  async setApplicationAuthority(id) {
    if (id !== null && !this.authorities.has(id)) throw new Error("migration application authority is unknown");
    await this.stopAllWriters();
    for (const authority of this.authorities.values()) await setBucketWrite(authority.storage, false, authority.store);
    if (id !== null) {
      const authority = this.authorities.get(id);
      await setBucketWrite(authority.storage, true, authority.store);
      if (startFleet(authority.fleetPath, { readinessPolicy: LIVE_MIGRATION_FLEET_READINESS_POLICY }).status !== "READY") throw new Error("migration authority did not become ready");
    }
  }

  async observeAuthorities() {
    const observedAt = new Date().toISOString();
    return [...this.authorities].map(([id, authority]) => {
      const runningNodes = runningFleetNodes(authority.fleet);
      const deployer = `${authority.storage.project}-celld-worker-deploy`;
      const deployerDocument = inspect(deployer);
      if (deployerDocument) assertOwned(deployerDocument, authority.storage.run_id, deployer);
      const deployerRunning = deployerDocument?.[0]?.State?.Running === true;
      const policy = bucketPolicyObservation(authority.storage);
      return {
        id,
        policy_writable: policy.writable,
        policy_sha256: policy.policy_sha256,
        fleet_running: runningNodes === authority.fleet.nodes.length,
        running_writer_classes: [
          ...(runningNodes > 0 ? ["celld_nodes", "worker_alarms"] : []),
          ...(deployerRunning ? ["deployment_cli"] : []),
        ],
        running_nodes: runningNodes,
        expected_nodes: authority.fleet.nodes.length,
        observed_at: observedAt,
      };
    });
  }

  async planMigrationMutation(record) { return this.journal.plan(record); }

  async completeMigrationMutation(planId, details) { return this.journal.complete(planId, details); }

  async probeWriteDenied(id) {
    const authority = this.authorities.get(id);
    if (!authority) throw new Error("migration denial authority is unknown");
    const client = new S3V1Client({ ...authority.store.profile, identity_file_ref: authority.storage.identity_file_ref });
    const key = `migration/write-denial-${sha256(id).slice(0, 12)}`;
    try {
      const response = await client.put(key, "must-be-denied", { ifNoneMatch: "*" });
      if ([401, 403].includes(response.status)) return true;
      if (response.status >= 200 && response.status < 300) await authority.store.delete(key);
      return false;
    } finally { client.close(); }
  }

  async runCanary(id) {
    const authority = this.authorities.get(id);
    return diagnoseFleet(authority.fleetPath).status === "READY" && (await probeFleetWorker(authority.fleetPath)).status === "READY";
  }

  async recordCutover(id) {
    const observations = await this.observeAuthorities();
    if (observations.some((entry) => entry.policy_writable || entry.fleet_running || entry.running_writer_classes.length > 0)) throw new Error("cutover must be recorded while both authorities are quiesced");
    return { schema_version: "agentic-sandbox.celld-migration-cutover/v1", authority_id: id, recorded_at: new Date().toISOString(), identity_sha256: sha256(id), authority_observations: observations };
  }

  async createApplicationWrite(id) {
    const authority = this.authorities.get(id);
    const suffix = randomBytes(12).toString("hex");
    const operationId = `migration-write-${suffix}`;
    const result = await sendWorkerCommand({
      endpoint: workerEndpoint(authority.fleet),
      varsFile: authority.fleet.worker_vars_file_ref,
      instanceId: `migration-${suffix}`,
      operationId,
      generation: 1,
      action: "provision",
      payload: { qualification: "offline-migration", phase: "post-cutover" },
      nonce: randomBytes(16).toString("hex"),
    });
    return result.status === 202
      && result.body?.document_type === "instance-cell-state"
      && result.body?.generation === 1
      && result.body?.effects?.some((effect) => effect.operation_id === operationId);
  }
}

export async function executeLiveOfflineMigration({ sourceConfigPath, destinationRoot, destinationRunId, artifactPath }) {
  const sourceStorage = protectedJson(sourceConfigPath, "source storage config");
  const sourceErrors = validateFixtureConfig(sourceStorage);
  if (sourceErrors.length || sourceStorage.fixture_profile !== "titan-single-host-storage") throw new Error(sourceErrors[0] ?? "source must use the promoting Titan fixture");
  if (resolve(sourceConfigPath) !== join(sourceStorage.run_root, "fixture.json") || sourceStorage.run_root !== `/dev/shm/agentic-celld-storage/${sourceStorage.run_id}`) throw new Error("source migration identity is invalid");
  if (!RUN_ID.test(destinationRunId) || destinationRunId === sourceStorage.run_id || destinationRoot !== `/dev/shm/agentic-celld-migration/${destinationRunId}`) throw new Error("destination migration identity is invalid");
  if (!isAbsolute(artifactPath) || artifactPath !== resolve("artifacts/celld-offline-migration.json")) throw new Error("migration artifact path must be the fixed repository artifact");
  const startedAt = new Date().toISOString();
  const qualificationPath = join(dirname(artifactPath), "celld-offline-migration-destination-qualification.json");
  const qualificationRawPath = join(dirname(artifactPath), "celld-offline-migration-destination-qualification.jsonl");
  const qualificationErrorPath = join(dirname(artifactPath), "celld-offline-migration-destination-qualification-error.json");
  const migrationErrorPath = join(dirname(artifactPath), "celld-offline-migration-error.json");
  let destinationStorage = null, sourceFleet = null, destinationFleet = null, sourceFleetPath = null, destinationFleetPath = null;
  let sourceStore = null, destinationStore = null, sourceGatewayAccess = null, destinationGatewayAccess = null;
  let migrationJournal = null;
  let migrationBoundaryStarted = false;
  let failureStage = "initializing";
  let sanitized = null;
  let operationError = null;
  const cleanupErrors = [];
  let retainedNamespaces = false;
  try {
    failureStage = "start-source-storage";
    startFixture(sourceStorage);
    failureStage = "prepare-destination-storage";
    destinationStorage = prepareFixture({ fixtureProfile: "titan-single-host-storage", runId: destinationRunId, root: destinationRoot });
    failureStage = "start-destination-storage";
    startFixture(destinationStorage);
    failureStage = "open-source-gateway";
    sourceGatewayAccess = await openStorageGatewayAccess(sourceStorage, { services: ["s3gateway1"] });
    failureStage = "open-destination-gateways";
    destinationGatewayAccess = await openStorageGatewayAccess(destinationStorage, { services: ["s3gateway1", "s3gateway2"] });
    const qualificationRows = [];
    let destinationQualification;
    let qualificationRowSummary;
    try {
      failureStage = "qualify-destination-storage";
      destinationQualification = await runS3Qualification(storageProfile(destinationStorage, destinationStorage.identity_file_ref, destinationGatewayAccess.endpoints), {
        adminIdentityFileRef: destinationStorage.admin_identity_file_ref,
        revokedIdentityFileRef: destinationStorage.revoked_identity_file_ref,
        rawRows: qualificationRows,
      });
      qualificationRowSummary = summarizeDestinationQualificationRows(qualificationRows, destinationStorage.limits);
    } catch (error) {
      const rawArtifact = writeArtifact(qualificationRawPath, `${qualificationRows.map((row) => JSON.stringify(row)).join("\n")}\n`);
      writeArtifact(qualificationErrorPath, `${JSON.stringify({ schema_version: "agentic-sandbox.celld-migration-destination-qualification-error/v1", run_id: sourceStorage.run_id, destination_run_id: destinationRunId, error_sha256: sha256(error.message), raw_artifact: rawArtifact }, null, 2)}\n`);
      throw error;
    }
    const qualificationArtifact = writeArtifact(qualificationPath, `${JSON.stringify(destinationQualification)}\n`);
    const qualificationRawArtifact = writeArtifact(qualificationRawPath, `${qualificationRows.map((row) => JSON.stringify(row)).join("\n")}\n`);
    failureStage = "evaluate-destination-storage";
    const qualificationVerdict = evaluateStorageEvidence(destinationQualification);
    if (qualificationVerdict.status !== "PASS" || qualificationVerdict.live_qualification !== true) throw new Error(`destination storage qualification failed: ${qualificationVerdict.reason_code}`);
    failureStage = "prepare-source-fleet";
    sourceFleet = prepareFleet({ storageConfigPath: sourceConfigPath });
    failureStage = "prepare-destination-fleet";
    destinationFleet = prepareFleet({ storageConfigPath: join(destinationRoot, "fixture.json") });
    sourceFleetPath = join(sourceStorage.run_root, "fleet.json");
    destinationFleetPath = join(destinationStorage.run_root, "fleet.json");
    failureStage = "deploy-source-worker";
    await deployFleetWorker(sourceFleetPath);
    failureStage = "deploy-destination-worker";
    await deployFleetWorker(destinationFleetPath);
    failureStage = "start-source-fleet";
    if (startFleet(sourceFleetPath, { readinessPolicy: LIVE_MIGRATION_FLEET_READINESS_POLICY }).status !== "READY") throw new Error("migration source fleet did not become ready");
    const initialSuffix = randomBytes(12).toString("hex");
    const initialOperationId = `migration-source-write-${initialSuffix}`;
    failureStage = "seed-source-worker";
    const initial = await sendWorkerCommand({
      endpoint: workerEndpoint(sourceFleet),
      varsFile: sourceFleet.worker_vars_file_ref,
      instanceId: `migration-source-${initialSuffix}`,
      operationId: initialOperationId,
      generation: 1,
      action: "provision",
      payload: { qualification: "offline-migration", phase: "source-seed" },
      nonce: randomBytes(16).toString("hex"),
    });
    if (initial.status !== 202 || initial.body?.document_type !== "instance-cell-state" || !initial.body?.effects?.some((effect) => effect.operation_id === initialOperationId)) throw new Error("migration source seed write failed");
    sourceStore = new LiveS3MigrationStore(sourceStorage, { gatewayEndpoints: sourceGatewayAccess.endpoints });
    destinationStore = new LiveS3MigrationStore(destinationStorage, { gatewayEndpoints: destinationGatewayAccess.endpoints });
    failureStage = "ensure-destination-bucket";
    await destinationStore.ensureBucket();
    const sourceAuthority = { storage: sourceStorage, fleet: sourceFleet, fleetPath: sourceFleetPath, store: sourceStore };
    const destinationAuthority = { storage: destinationStorage, fleet: destinationFleet, fleetPath: destinationFleetPath, store: destinationStore };
    migrationBoundaryStarted = true;
    failureStage = "create-migration-journal";
    migrationJournal = new LiveMigrationJournal(join(sourceStorage.run_root, "migration-journal.json"), { runId: sourceStorage.run_id, destinationRunId });
    failureStage = "rehearse-offline-migration";
    const evidence = await rehearseOfflineMigration({ source: sourceStore, destination: destinationStore, control: new LiveMigrationControl(sourceAuthority, destinationAuthority, migrationJournal) });
    failureStage = "validate-migration-journal";
    const journalEvidence = migrationJournal.evidence();
    if (journalEvidence.incomplete_plan_ids.length !== 0) throw new Error("successful migration journal retains an incomplete mutation");
    sanitized = {
      ...evidence,
      run_id: sourceStorage.run_id,
      destination_run_id: destinationRunId,
      started_at: startedAt,
      sandbox_git: run("git", ["rev-parse", "HEAD"]),
      host_sha256: sourceFleet.host_sha256,
      source_backend: sourceStorage.backend,
      destination_backend: destinationStorage.backend,
      source_bucket_sha256: sha256(sourceStorage.bucket),
      destination_bucket_sha256: sha256(destinationStorage.bucket),
      source_namespace_sha256: sha256(`${sourceStorage.bucket}/${sourceStorage.run_prefix}`),
      destination_namespace_sha256: sha256(`${destinationStorage.bucket}/${destinationStorage.run_prefix}`),
      pins: sourceFleet.pins,
      migration_journal: journalEvidence,
      destination_qualification: {
        reason_code: qualificationVerdict.reason_code,
        evidence_artifact: qualificationArtifact,
        raw_artifact: qualificationRawArtifact,
        raw_rows: qualificationRowSummary,
      },
      storage_boundary: {
        migrated: "Celld object-store namespace only",
        sandbox_local_filesystems: "not_targeted",
        volume_mounts: "not_targeted",
        vm_disks: "not_targeted",
        agentshare: "not_targeted",
        workspaces: "not_targeted",
        management_state: "not_targeted",
      },
    };
  } catch (error) {
    operationError = error;
  } finally {
    try { if (destinationFleetPath && existsSync(destinationFleetPath)) cleanupFleet(destinationFleetPath); } catch (error) { cleanupErrors.push(`destination-fleet:${sha256(error.message)}`); }
    try { if (sourceFleetPath && existsSync(sourceFleetPath)) cleanupFleet(sourceFleetPath); } catch (error) { cleanupErrors.push(`source-fleet:${sha256(error.message)}`); }
    retainedNamespaces = operationError !== null && migrationBoundaryStarted;
    if (retainedNamespaces) {
      try { if (sourceStore) await setBucketWrite(sourceStorage, false, sourceStore); } catch (error) { cleanupErrors.push(`source-policy-deny:${sha256(error.message)}`); }
      try { if (destinationStore) await setBucketWrite(destinationStorage, false, destinationStore); } catch (error) { cleanupErrors.push(`destination-policy-deny:${sha256(error.message)}`); }
    } else if (cleanupErrors.length === 0) {
      try { if (sourceStore) await setBucketWrite(sourceStorage, true, sourceStore); } catch (error) { cleanupErrors.push(`source-policy:${sha256(error.message)}`); }
    } else if (sourceStore) cleanupErrors.push("source-policy:not-restored-before-writer-cleanup");
    try { sourceStore?.close(); } catch (error) { cleanupErrors.push(`source-client:${sha256(error.message)}`); }
    try { destinationStore?.close(); } catch (error) { cleanupErrors.push(`destination-client:${sha256(error.message)}`); }
    try { if (destinationGatewayAccess) await destinationGatewayAccess.close(); } catch (error) { cleanupErrors.push(`destination-forwarder:${sha256(error.message)}`); }
    try { if (sourceGatewayAccess) await sourceGatewayAccess.close(); } catch (error) { cleanupErrors.push(`source-forwarder:${sha256(error.message)}`); }
    if (retainedNamespaces) {
      try { if (destinationStorage) compose(destinationStorage, ["stop", "--timeout", "30"], 120_000); } catch (error) { cleanupErrors.push(`destination-store-stop:${sha256(error.message)}`); }
      try { compose(sourceStorage, ["stop", "--timeout", "30"], 120_000); } catch (error) { cleanupErrors.push(`source-store-stop:${sha256(error.message)}`); }
    } else {
      try { if (destinationStorage) cleanupFixture(destinationStorage); } catch (error) { cleanupErrors.push(`destination-store:${sha256(error.message)}`); }
      try { cleanupFixture(sourceStorage, { removeRoot: false }); } catch (error) { cleanupErrors.push(`source-store:${sha256(error.message)}`); }
    }
  }
  if (operationError || cleanupErrors.length) {
    let journalEvidence = null;
    let journalErrorSha256 = null;
    try { journalEvidence = migrationJournal?.evidence() ?? null; } catch (error) { journalErrorSha256 = sha256(error.message); }
    const failure = buildMigrationFailureEvidence({ sourceStorage, destinationStorage, destinationRunId, startedAt, endedAt: new Date().toISOString(), sandboxGit: run("git", ["rev-parse", "HEAD"]), operationError, cleanupErrors, journalEvidence, journalErrorSha256, retainedNamespaces, failureStage });
    writeArtifact(migrationErrorPath, `${JSON.stringify(failure, null, 2)}\n`);
    process.stderr.write(`${JSON.stringify({ status: "FAIL", reason_code: "CELLD_LIVE_OFFLINE_MIGRATION_FAILED", failure_stage: failure.failure_stage, operation_context: failure.operation_context, error_sha256: failure.error_sha256, retain_namespaces: failure.retain_namespaces })}\n`);
    throw new Error(`offline migration failed: operation=${failure.error_sha256 ?? "none"} cleanup=${sha256(cleanupErrors.join(","))}`);
  }
  sanitized.ended_at = new Date().toISOString();
  sanitized.cleanup = {
    status: "passed",
    source_application_policy: "restored_writable_for_reusable_disposable_fixture",
    source_fleet: "removed",
    destination_fleet: "removed",
    source_store: "stopped_and_named_volumes_removed_run_root_retained",
    destination_store: "removed",
  };
  writeArtifact(artifactPath, `${JSON.stringify(sanitized, null, 2)}\n`);
  return sanitized;
}

async function main(args) {
  const result = await executeLiveOfflineMigration({ sourceConfigPath: resolve(argument(args, "--source-config")), destinationRoot: resolve(argument(args, "--destination-root")), destinationRunId: argument(args, "--destination-run-id"), artifactPath: resolve(argument(args, "--artifact")) });
  process.stdout.write(`${JSON.stringify({ status: "PASS", run_id: result.run_id, forward: result.forward, reverse: result.reverse, dual_authority_observed: result.dual_authority_observed, local_storage_touched: result.local_storage_touched })}\n`);
}

if (process.argv[1] && resolve(process.argv[1]) === SCRIPT_PATH) main(process.argv.slice(2)).catch((error) => { process.stderr.write(`CELLD_LIVE_OFFLINE_MIGRATION_ERROR ${sha256(error.message)}\n`); process.exitCode = 4; });
