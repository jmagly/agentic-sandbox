#!/usr/bin/env node

import { createHash } from "node:crypto";
import { chmodSync, lstatSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { hostname } from "node:os";
import { basename, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

import { evaluateStorageEvidence, STORAGE_PROFILE_SCHEMA, validateStorageProfile } from "./celld-storage-qualifier.mjs";
import { runS3Qualification } from "./celld-storage-race-runner.mjs";
import { cleanupFixture, fixtureEnvironment, parseComposePs, validateFixtureConfig } from "./celld-seaweedfs-fixture.mjs";

const OBSERVATION_SCHEMA = "agentic-sandbox.celld-live-observation/v1";
const DRIVER_FAILURE_STAGES = new Set([
  "fixture-pull",
  "fixture-start",
  "fixture-readiness",
  "gateway-discovery",
  "profile-validation",
  "storage-measurement",
  "evidence-evaluation",
]);

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

export function formatStorageDriverFailure(error) {
  const stage = DRIVER_FAILURE_STAGES.has(error?.stage) ? error.stage : "unclassified";
  const cleanup = ["passed", "failed", "unknown"].includes(error?.cleanupStatus) ? error.cleanupStatus : "unknown";
  const suppliedDigest = typeof error?.causeSha256 === "string" && /^[0-9a-f]{64}$/.test(error.causeSha256)
    ? error.causeSha256
    : null;
  const causeDigest = suppliedDigest ?? sha256(String(error?.message ?? "unknown storage driver failure"));
  return `CELLD_STORAGE_DRIVER_ERROR stage=${stage} cleanup=${cleanup} cause_sha256=${causeDigest}`;
}

function argument(args, name) {
  const index = args.indexOf(name);
  if (index < 0 || !args[index + 1]) throw new Error(`${name} is required`);
  return args[index + 1];
}

function run(program, args, options = {}) {
  const outcome = spawnSync(program, args, { encoding: "utf8", shell: false, ...options });
  if (outcome.error || outcome.status !== 0) throw new Error(`${basename(program)} failed: ${(outcome.error?.message ?? outcome.stderr ?? "").trim()}`);
  return outcome.stdout.trim();
}

function compose(config, args, timeout = 600_000) {
  return run("docker", ["compose", "-f", config.compose_file, "-p", config.project, ...args], { env: fixtureEnvironment(config), timeout });
}

export function publishedGatewayEndpoint(output, service) {
  const serviceRows = parseComposePs(output).filter((row) => row?.Service === service);
  const publishers = serviceRows.flatMap((row) => Array.isArray(row.Publishers) ? row.Publishers : []);
  const matches = publishers.filter((publisher) =>
    Number(publisher?.TargetPort) === 8334
    && Number.isSafeInteger(Number(publisher?.PublishedPort))
    && Number(publisher.PublishedPort) > 0
    && Number(publisher.PublishedPort) <= 65_535
    && publisher?.Protocol === "tcp"
    && publisher?.URL === "127.0.0.1");
  if (matches.length !== 1) throw new Error(`could not resolve ${service} TLS port`);
  return `https://127.0.0.1:${Number(matches[0].PublishedPort)}`;
}

function endpoint(config, service) {
  const output = compose(config, ["ps", "--format", "json", service]);
  return publishedGatewayEndpoint(output, service);
}

function artifact(path, relativePath, mimeType) {
  const bytes = readFileSync(path);
  return { path: relativePath, mime_type: mimeType, sha256: sha256(bytes), bytes: bytes.length, contains_restricted_data: false };
}

function gitCommit() {
  return run("git", ["rev-parse", "HEAD"]);
}

function unavailable({ driverId, scenarioId, runId, liveProfile, startedAt, reasonCode }) {
  return {
    schema_version: OBSERVATION_SCHEMA,
    driver_id: driverId,
    run_id: runId,
    scenario_id: scenarioId,
    started_at: startedAt,
    ended_at: new Date().toISOString(),
    mutation_started: false,
    prerequisites: [{ id: "CELLD_STORAGE_FIXTURE", status: "unavailable", reason_code: reasonCode }],
    assertions: [],
    identities: {
      profile_id: liveProfile.profile_id,
      sandbox_git: liveProfile.expected_sandbox_git,
      environment_host_sha256: liveProfile.environment.host_sha256,
      driver_version: "celld-live-storage-topology/v1",
    },
    metrics: [], faults: [], artifacts: [],
    cleanup: { status: "not_required", assertions: [] },
  };
}

export async function executeStorageDriver({ scenarioId, runId, liveProfilePath, artifactDir }) {
  const startedAt = new Date().toISOString();
  const driverId = "celld-live-storage-topology";
  const liveProfile = JSON.parse(readFileSync(liveProfilePath, "utf8"));
  const entry = liveProfile.drivers?.[driverId];
  if (!entry?.enabled) return unavailable({ driverId, scenarioId, runId, liveProfile, startedAt, reasonCode: "CELLD_STORAGE_DRIVER_DISABLED" });
  if (scenarioId !== "UAT-CELLD-010" || liveProfile.run_id !== runId || liveProfile.expected_sandbox_git !== gitCommit()) throw new Error("live storage driver identity does not match the requested run");
  if (liveProfile.environment.host_sha256 !== sha256(hostname())) throw new Error("live storage driver host identity does not match the protected profile");
  const configMetadata = lstatSync(entry.config_path);
  if (!configMetadata.isFile() || configMetadata.isSymbolicLink() || (configMetadata.mode & 0o077) !== 0) throw new Error("fixture config must be a protected regular non-symlink file");
  const config = JSON.parse(readFileSync(entry.config_path, "utf8"));
  const configErrors = validateFixtureConfig(config);
  if (configErrors.length) throw new Error(configErrors.join("; "));
  const rootMetadata = lstatSync(config.run_root);
  if (!rootMetadata.isDirectory() || rootMetadata.isSymbolicLink() || (rootMetadata.mode & 0o077) !== 0) throw new Error("fixture run root must be a protected regular directory");
  if (config.run_id !== runId || config.fixture_profile !== "titan-single-host-storage" || !config.promoting) throw new Error("live driver requires the exact promoting Titan storage profile");
  if (process.platform !== "linux") return unavailable({ driverId, scenarioId, runId, liveProfile, startedAt, reasonCode: "CELLD_STORAGE_LINUX_REQUIRED" });
  if (spawnSync("docker", ["compose", "version"], { encoding: "utf8", shell: false }).status !== 0) return unavailable({ driverId, scenarioId, runId, liveProfile, startedAt, reasonCode: "CELLD_STORAGE_DOCKER_UNAVAILABLE" });

  mkdirSync(artifactDir, { recursive: true, mode: 0o700 });
  const rawRows = [];
  let cleanupStatus = "failed";
  let cleanupAssertions = [];
  let storageEvidence;
  let activeStage = "fixture-pull";
  let primaryFailure = null;
  try {
    compose(config, ["pull", "--quiet"], 900_000);
    activeStage = "fixture-start";
    compose(config, ["up", "-d", "--wait", "--wait-timeout", "240"], 600_000);
    activeStage = "fixture-readiness";
    const services = compose(config, ["ps", "--services", "--status", "running"]).split(/\r?\n/).filter(Boolean);
    const required = ["postgres", "master1", "master2", "master3", "volume1", "volume2", "volume3", "filer1", "filer2", "filer3", "s3gateway1", "s3gateway2"];
    if (required.some((service) => !services.includes(service))) throw new Error("not every required SeaweedFS fixture service is running");
    activeStage = "gateway-discovery";
    const endpoints = [endpoint(config, "s3gateway1"), endpoint(config, "s3gateway2")];
    activeStage = "profile-validation";
    const profile = {
      schema_version: STORAGE_PROFILE_SCHEMA,
      profile_id: config.project,
      run_id: config.run_id,
      dialect: "s3-v1",
      scope: "live_candidate",
      endpoint: endpoints[0],
      region: config.region,
      addressing_mode: "path",
      bucket: config.bucket,
      run_prefix: config.run_prefix,
      identity_file_ref: config.identity_file_ref,
      ca_file_ref: config.ca_file_ref,
      backend: {
        product: config.backend.product,
        version: config.backend.version,
        artifact_sha256: config.backend.artifact_sha256,
        configuration_sha256: config.backend.configuration_sha256,
        gateway_endpoints: endpoints,
        topology: config.backend.topology,
      },
      limits: config.limits,
    };
    const profileErrors = validateStorageProfile(profile);
    if (profileErrors.length) throw new Error(profileErrors.join("; "));
    activeStage = "storage-measurement";
    storageEvidence = await runS3Qualification(profile, {
      adminIdentityFileRef: config.admin_identity_file_ref,
      revokedIdentityFileRef: config.revoked_identity_file_ref,
      rawRows,
    });
    activeStage = "evidence-evaluation";
    const evaluated = evaluateStorageEvidence(storageEvidence);
    if (evaluated.status === "ERROR") throw new Error(`${evaluated.reason_code}: ${evaluated.errors.join("; ")}`);
  } catch (error) {
    primaryFailure = {
      stage: activeStage,
      causeSha256: sha256(String(error?.message ?? error)),
    };
  } finally {
    try {
      cleanupFixture(config, { removeRoot: false });
      cleanupStatus = "passed";
      cleanupAssertions = ["all Compose services, networks, and named volumes removed", "run bucket emptied and delete attempted"];
    } catch (error) {
      cleanupAssertions = [`cleanup error digest ${sha256(error.message)}`];
    }
  }
  if (primaryFailure) {
    const error = new Error("storage driver stage failed");
    error.stage = primaryFailure.stage;
    error.causeSha256 = primaryFailure.causeSha256;
    error.cleanupStatus = cleanupStatus;
    throw error;
  }
  if (!storageEvidence) throw new Error("storage measurement did not produce an evidence envelope");

  const evidencePath = join(artifactDir, "storage-evidence.json");
  const rawPath = join(artifactDir, "storage-rounds.jsonl");
  writeFileSync(evidencePath, `${JSON.stringify(storageEvidence)}\n`, { mode: 0o600, flag: "wx" });
  writeFileSync(rawPath, `${rawRows.map((row) => JSON.stringify(row)).join("\n")}\n`, { mode: 0o600, flag: "wx" });
  chmodSync(evidencePath, 0o600);
  chmodSync(rawPath, 0o600);
  const evidenceArtifact = artifact(evidencePath, "artifacts/storage-evidence.json", "application/json");
  const rawArtifact = artifact(rawPath, "artifacts/storage-rounds.jsonl", "application/x-ndjson");
  return {
    schema_version: OBSERVATION_SCHEMA,
    driver_id: driverId,
    run_id: runId,
    scenario_id: scenarioId,
    started_at: startedAt,
    ended_at: new Date().toISOString(),
    mutation_started: true,
    prerequisites: [
      { id: "CELLD_STORAGE_FIXTURE", status: "available", reason_code: "CELLD_STORAGE_EXACT_PIN_READY" },
      { id: "CELLD_STORAGE_TLS", status: "available", reason_code: "CELLD_STORAGE_TLS_GATEWAYS_READY" },
      { id: "CELLD_STORAGE_SCOPE", status: "available", reason_code: "CELLD_STORAGE_RUN_BUCKET_READY" },
    ],
    assertions: [{ id: "CELLD.010.STORAGE", measurements: storageEvidence, evidence_refs: [evidenceArtifact.path, rawArtifact.path] }],
    identities: {
      profile_id: liveProfile.profile_id,
      sandbox_git: liveProfile.expected_sandbox_git,
      environment_host_sha256: liveProfile.environment.host_sha256,
      driver_version: "celld-live-storage-topology/v1",
    },
    metrics: [], faults: [], artifacts: [evidenceArtifact, rawArtifact],
    cleanup: { status: cleanupStatus, assertions: cleanupAssertions },
  };
}

async function main(args) {
  const observation = await executeStorageDriver({
    scenarioId: argument(args, "--scenario-id"),
    runId: argument(args, "--run-id"),
    liveProfilePath: resolve(argument(args, "--profile")),
    artifactDir: resolve(argument(args, "--artifact-dir")),
  });
  process.stdout.write(`${JSON.stringify(observation)}\n`);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  main(process.argv.slice(2)).catch((error) => {
    process.stderr.write(`${formatStorageDriverFailure(error)}\n`);
    process.exitCode = 3;
  });
}
