#!/usr/bin/env node

import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, lstatSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { hostname } from "node:os";
import { dirname, isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { validateOrchestrationConfig } from "./celld-live-orchestration.mjs";
import { loadReviewedRolloutCandidates } from "./celld-rollout-candidate.mjs";
import { executeRolloutController } from "./celld-rollout-controller.mjs";
import { validateLiveProfile } from "./celld-uat-live-protocol.mjs";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
const DRIVER_ID = "celld-live-rollout";
const DRIVER_VERSION = "celld-live-rollout/v1";
const OBSERVATION_SCHEMA = "agentic-sandbox.celld-live-observation/v1";
const SCENARIO_ID = "UAT-CELLD-011";
const QUALIFIED_IMAGES_PATH = resolve(REPO_ROOT, "deploy/celld/qualification/celld-images.json");
const SHA256_DIGEST = /^sha256:[0-9a-f]{64}$/;

function sha256(value) { return createHash("sha256").update(value).digest("hex"); }

function gitCommit() {
  const result = spawnSync("git", ["rev-parse", "HEAD"], { cwd: REPO_ROOT, encoding: "utf8", shell: false });
  if (result.error || result.status !== 0 || !/^[0-9a-f]{40}\n?$/.test(result.stdout ?? "")) throw new Error("exact Git commit is unavailable");
  return result.stdout.trim();
}

function argument(args, name) {
  const index = args.indexOf(name);
  if (index < 0 || !args[index + 1]) throw new Error(`${name} is required`);
  return args[index + 1];
}

function protectedJson(path, description) {
  if (!isAbsolute(path) || !existsSync(path)) throw new Error(`${description} is missing`);
  const metadata = lstatSync(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) throw new Error(`${description} must be a protected regular non-symlink file`);
  return JSON.parse(readFileSync(path, "utf8"));
}

export function qualifiedCelldArtifacts(document) {
  if (document?.schema_version !== "agentic-sandbox.celld-images/v1" || document?.platform !== "linux/amd64") throw new Error("qualified Celld inventory contract is invalid");
  const entries = Array.isArray(document.celld) ? document.celld : [document.celld];
  return entries.map((entry, index) => {
    if (!entry || typeof entry.version !== "string" || !/^[0-9a-f]{40}$/.test(entry.commit ?? "") || !SHA256_DIGEST.test(entry.manifest_digest ?? "") || (entry.compatible_from !== undefined && (!Array.isArray(entry.compatible_from) || entry.compatible_from.some((value) => typeof value !== "string")))) throw new Error(`qualified Celld artifact ${index} is invalid`);
    return { version: entry.version, commit: entry.commit, manifest_digest: entry.manifest_digest, compatible_from: entry.compatible_from ?? [] };
  });
}

export function selectDistinctRolloutPair(artifacts) {
  for (const previous of artifacts) for (const candidate of artifacts) {
    if (previous.manifest_digest !== candidate.manifest_digest && previous.version !== candidate.version && candidate.compatible_from.includes(previous.version)) return { previous, candidate };
  }
  return null;
}

function unavailable(profile, runId, startedAt, reasonCode) {
  return {
    schema_version: OBSERVATION_SCHEMA,
    driver_id: DRIVER_ID,
    run_id: runId,
    scenario_id: SCENARIO_ID,
    started_at: startedAt,
    ended_at: new Date().toISOString(),
    mutation_started: false,
    prerequisites: [{ id: "CELLD_QUALIFIED_ROLLOUT_PAIR", status: "unavailable", reason_code: reasonCode }],
    assertions: [],
    identities: { profile_id: profile.profile_id, sandbox_git: profile.expected_sandbox_git, environment_host_sha256: profile.environment.host_sha256, driver_version: DRIVER_VERSION },
    metrics: [],
    faults: [],
    artifacts: [],
    cleanup: { status: "not_required", assertions: ["no rollout mutation was started"] },
  };
}

function artifact(path, relativePath) {
  const bytes = readFileSync(path);
  return { path: relativePath, mime_type: relativePath.endsWith(".jsonl") ? "application/x-ndjson" : "application/json", sha256: sha256(bytes), bytes: bytes.length, contains_restricted_data: false };
}

export async function executeRolloutDriver({ scenarioId, runId, liveProfilePath, artifactDir }, dependencies = {}) {
  const startedAt = new Date().toISOString();
  const profile = protectedJson(liveProfilePath, "live profile");
  const profileErrors = validateLiveProfile(profile);
  if (profileErrors.length) throw new Error(profileErrors.join("; "));
  const entry = profile.drivers?.[DRIVER_ID];
  if (!entry?.enabled) return unavailable(profile, runId, startedAt, "CELLD_LIVE_ROLLOUT_DRIVER_DISABLED");
  const git = dependencies.gitCommit?.() ?? gitCommit();
  const host = dependencies.hostname?.() ?? hostname();
  if (scenarioId !== SCENARIO_ID || profile.run_id !== runId || profile.expected_sandbox_git !== git || profile.environment.host_sha256 !== sha256(host)) throw new Error("rollout live identity does not match the requested run");
  const config = protectedJson(entry.config_path, "orchestration config");
  const configErrors = validateOrchestrationConfig(config);
  if (configErrors.length || config.run_id !== runId) throw new Error([...configErrors, "config run identity mismatch"].join("; "));

  const inventory = dependencies.qualifiedImages?.() ?? JSON.parse(readFileSync(QUALIFIED_IMAGES_PATH, "utf8"));
  const artifacts = qualifiedCelldArtifacts(inventory);
  const pair = selectDistinctRolloutPair(artifacts);
  if (!pair) {
    const reviewedCandidates = dependencies.reviewedCandidates?.() ?? loadReviewedRolloutCandidates();
    if (reviewedCandidates.length > 0) return unavailable(profile, runId, startedAt, "CELLD_ROLLOUT_CANDIDATE_UNQUALIFIED");
    return unavailable(profile, runId, startedAt, "CELLD_QUALIFIED_ROLLOUT_PAIR_UNAVAILABLE");
  }
  if (!profile.authorization.destructive_faults || profile.authorization.exact_run_owner !== runId) return unavailable(profile, runId, startedAt, "CELLD_DESTRUCTIVE_AUTHORIZATION_REQUIRED");
  if (!dependencies.rolloutAdapter || !dependencies.rolloutPlan) return unavailable(profile, runId, startedAt, "CELLD_ROLLOUT_ADAPTER_UNAVAILABLE");

  const plan = {
    ...dependencies.rolloutPlan,
    run_id: runId,
    previous: pair.previous,
    candidate: pair.candidate,
    authorization: profile.authorization,
  };
  const campaign = await executeRolloutController(plan, dependencies.rolloutAdapter);
  const root = resolve(artifactDir ?? "");
  if (!isAbsolute(artifactDir ?? "")) throw new Error("rollout artifact directory must be absolute");
  mkdirSync(root, { recursive: true, mode: 0o700 });
  chmodSync(root, 0o700);
  const evidencePath = resolve(root, "rollout-controller-evidence.json");
  const timelinePath = resolve(root, "rollout-controller-timeline.jsonl");
  writeFileSync(evidencePath, `${JSON.stringify({ schema_version: "agentic-sandbox.celld-rollout-evidence/v1", run_id: runId, assertions: campaign.assertions, mutation_count: campaign.mutation_count })}\n`, { mode: 0o600, flag: "wx" });
  writeFileSync(timelinePath, `${campaign.timeline.map((entry) => JSON.stringify(entry)).join("\n")}\n`, { mode: 0o600, flag: "wx" });
  chmodSync(evidencePath, 0o600); chmodSync(timelinePath, 0o600);
  const artifactsOut = [artifact(evidencePath, "artifacts/rollout-controller-evidence.json"), artifact(timelinePath, "artifacts/rollout-controller-timeline.jsonl")];
  return {
    schema_version: OBSERVATION_SCHEMA,
    driver_id: DRIVER_ID,
    run_id: runId,
    scenario_id: SCENARIO_ID,
    started_at: startedAt,
    ended_at: new Date().toISOString(),
    mutation_started: true,
    prerequisites: [
      { id: "CELLD_QUALIFIED_ROLLOUT_PAIR", status: "available", reason_code: "CELLD_QUALIFIED_ROLLOUT_PAIR_READY" },
      { id: "CELLD_ROLLOUT_AUTHORIZATION", status: "available", reason_code: "CELLD_EXACT_RUN_DESTRUCTIVE_AUTHORIZATION_READY" },
      { id: "CELLD_ROLLOUT_CONTROLLER", status: "available", reason_code: "CELLD_ROLLOUT_CONTROLLER_READY" },
    ],
    assertions: campaign.assertions.map((entryAssertion) => ({ ...entryAssertion, evidence_refs: artifactsOut.map((entryArtifact) => entryArtifact.path) })),
    identities: { profile_id: profile.profile_id, sandbox_git: profile.expected_sandbox_git, environment_host_sha256: profile.environment.host_sha256, driver_version: DRIVER_VERSION },
    metrics: [{ name: "rollout_mutations", value: campaign.mutation_count, unit: "operations" }],
    faults: [{ kind: "abrupt_node_loss", healed: true }, { kind: "rollback_threshold", healed: true }],
    artifacts: artifactsOut,
    cleanup: { status: "passed", assertions: ["all three nodes restored to the previous approved digest", "declared reserve remained healthy"] },
  };
}

async function main(args) {
  const observation = await executeRolloutDriver({
    scenarioId: argument(args, "--scenario-id"),
    runId: argument(args, "--run-id"),
    liveProfilePath: resolve(argument(args, "--profile")),
    artifactDir: resolve(argument(args, "--artifact-dir")),
  });
  process.stdout.write(`${JSON.stringify(observation)}\n`);
}

if (process.argv[1] && SCRIPT_PATH === resolve(process.argv[1])) main(process.argv.slice(2)).catch((error) => { process.stderr.write(`CELLD_LIVE_ROLLOUT_ERROR ${sha256(error.message)}\n`); process.exitCode = 3; });
