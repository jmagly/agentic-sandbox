#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, lstatSync, readFileSync } from "node:fs";
import { hostname } from "node:os";
import { dirname, isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { LIVE_OBSERVATION_SCHEMA, validateLiveProfile } from "./celld-uat-live-protocol.mjs";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
const DRIVER_ID = "celld-live-credential-provenance";
const DRIVER_VERSION = "celld-live-credential-provenance/v1";
const SCENARIO_ID = "UAT-CELLD-013";
const RUN_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const REASON_CODE = /^[A-Z0-9][A-Z0-9_.-]+$/;

export const CREDENTIAL_PROVENANCE_PROFILE_SCHEMA = "agentic-sandbox.celld-live-credential-provenance/v1";

export const CREDENTIAL_PROVENANCE_PREREQUISITES = Object.freeze([
  Object.freeze({ id: "CELLD_CREDENTIAL_PROVENANCE_AUTHORIZATION", status: "unavailable", reason_code: "CELLD_CREDENTIAL_PROVENANCE_AUTHORIZATION_REQUIRED" }),
  Object.freeze({ id: "CELLD_STORAGE_QUALIFICATION", status: "unavailable", reason_code: "CELLD_STORAGE_QUALIFICATION_UNAVAILABLE" }),
  Object.freeze({ id: "CELLD_FLEET_QUALIFICATION", status: "unavailable", reason_code: "CELLD_FLEET_QUALIFICATION_UNAVAILABLE" }),
  Object.freeze({ id: "CELLD_NETWORK_AUTH_QUALIFICATION", status: "unavailable", reason_code: "CELLD_NETWORK_AUTH_QUALIFICATION_UNAVAILABLE" }),
  Object.freeze({ id: "CELLD_BROKER_INTEGRATED_FLEET", status: "unavailable", reason_code: "CELLD_BROKER_INTEGRATED_FLEET_UNAVAILABLE" }),
  Object.freeze({ id: "CELLD_S3_CREDENTIAL_ROTATION_CONTROL", status: "unavailable", reason_code: "CELLD_S3_CREDENTIAL_ROTATION_CONTROL_UNAVAILABLE" }),
  Object.freeze({ id: "CELLD_REQUEST_HMAC_ROTATION_CONTROL", status: "unavailable", reason_code: "CELLD_REQUEST_HMAC_ROTATION_CONTROL_UNAVAILABLE" }),
  Object.freeze({ id: "CELLD_MTLS_IDENTITY_ROTATION_CONTROL", status: "unavailable", reason_code: "CELLD_MTLS_IDENTITY_ROTATION_CONTROL_UNAVAILABLE" }),
  Object.freeze({ id: "CELLD_PEER_SECRET_ROTATION_CONTROL", status: "unavailable", reason_code: "CELLD_PEER_SECRET_ROTATION_CONTROL_UNAVAILABLE" }),
  Object.freeze({ id: "CELLD_FIXTURE_ADMIN_LIFECYCLE_CONTROL", status: "unavailable", reason_code: "CELLD_FIXTURE_ADMIN_LIFECYCLE_CONTROL_UNAVAILABLE" }),
  Object.freeze({ id: "CELLD_SECRET_SCAN_SURFACES", status: "unavailable", reason_code: "CELLD_SECRET_SCAN_SURFACES_UNAVAILABLE" }),
  Object.freeze({ id: "CELLD_SIGNED_ARTIFACT_VERIFIER", status: "unavailable", reason_code: "CELLD_SIGNED_ARTIFACT_VERIFIER_UNAVAILABLE" }),
  Object.freeze({ id: "CELLD_SCOPED_OBJECT_STORE_IDENTITIES", status: "unavailable", reason_code: "CELLD_SCOPED_OBJECT_STORE_IDENTITIES_UNAVAILABLE" }),
  Object.freeze({ id: "CELLD_SUPPORT_BUNDLE_EXPORTER", status: "unavailable", reason_code: "CELLD_SUPPORT_BUNDLE_EXPORTER_UNAVAILABLE" }),
]);

function sha256(value) { return createHash("sha256").update(value).digest("hex"); }

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

function gitCommit() {
  const result = spawnSync("git", ["rev-parse", "HEAD"], { cwd: REPO_ROOT, encoding: "utf8", shell: false });
  if (result.error || result.status !== 0 || !/^[0-9a-f]{40}\n?$/.test(result.stdout ?? "")) throw new Error("exact Git commit is unavailable");
  return result.stdout.trim();
}

function ownKeys(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? Object.keys(value) : [];
}

export function validateCredentialProvenanceProfile(profile) {
  const errors = [];
  if (!profile || typeof profile !== "object" || Array.isArray(profile)) return ["credential provenance profile must be an object"];
  const allowed = new Set(["schema_version", "run_id", "mode"]);
  for (const key of ownKeys(profile)) if (!allowed.has(key)) errors.push(`credential provenance profile.${key} is not allowed`);
  if (profile.schema_version !== CREDENTIAL_PROVENANCE_PROFILE_SCHEMA) errors.push(`credential provenance profile.schema_version must be ${CREDENTIAL_PROVENANCE_PROFILE_SCHEMA}`);
  if (!RUN_ID.test(profile.run_id ?? "")) errors.push("credential provenance profile.run_id is invalid");
  if (profile.mode !== "prerequisite-assessment") errors.push("credential provenance profile.mode must be prerequisite-assessment");
  return errors;
}

function validatePrerequisites(prerequisites) {
  if (!Array.isArray(prerequisites) || prerequisites.length === 0) throw new Error("credential provenance prerequisites must be a non-empty array");
  const ids = new Set();
  for (const prerequisite of prerequisites) {
    if (!prerequisite || typeof prerequisite !== "object" || Array.isArray(prerequisite)) throw new Error("credential provenance prerequisite must be an object");
    if (Object.keys(prerequisite).some((key) => !["id", "status", "reason_code"].includes(key))) throw new Error("credential provenance prerequisite contains an unknown field");
    if (!REASON_CODE.test(prerequisite.id ?? "") || ids.has(prerequisite.id)) throw new Error("credential provenance prerequisite id is invalid or duplicated");
    if (!["available", "unavailable"].includes(prerequisite.status)) throw new Error(`credential provenance prerequisite ${prerequisite.id} has an invalid status`);
    if (!REASON_CODE.test(prerequisite.reason_code ?? "")) throw new Error(`credential provenance prerequisite ${prerequisite.id} has an invalid reason code`);
    ids.add(prerequisite.id);
  }
  return prerequisites.map((item) => ({ ...item }));
}

function unavailable(profile, runId, startedAt, prerequisites, cleanupAssertion) {
  return {
    schema_version: LIVE_OBSERVATION_SCHEMA,
    driver_id: DRIVER_ID,
    run_id: runId,
    scenario_id: SCENARIO_ID,
    started_at: startedAt,
    ended_at: new Date().toISOString(),
    mutation_started: false,
    prerequisites,
    assertions: [],
    identities: {
      profile_id: profile.profile_id,
      sandbox_git: profile.expected_sandbox_git,
      environment_host_sha256: profile.environment.host_sha256,
      driver_version: DRIVER_VERSION,
    },
    metrics: [],
    faults: [],
    artifacts: [],
    cleanup: { status: "not_required", assertions: [cleanupAssertion] },
  };
}

export function executeCredentialProvenanceDriver({ scenarioId, runId, liveProfilePath }, dependencies = {}) {
  const startedAt = new Date().toISOString();
  const profile = protectedJson(liveProfilePath, "live profile");
  const profileErrors = validateLiveProfile(profile);
  if (profileErrors.length) throw new Error(profileErrors.join("; "));
  const entry = profile.drivers?.[DRIVER_ID];
  if (!entry?.enabled) {
    return unavailable(profile, runId, startedAt, [{ id: "CELLD_LIVE_CREDENTIAL_PROVENANCE", status: "unavailable", reason_code: "CELLD_LIVE_CREDENTIAL_PROVENANCE_DRIVER_DISABLED" }], "no credential or provider mutation was started");
  }

  const git = dependencies.gitCommit?.() ?? gitCommit();
  const host = dependencies.hostname?.() ?? hostname();
  if (scenarioId !== SCENARIO_ID || profile.run_id !== runId || profile.expected_sandbox_git !== git || profile.environment.host_sha256 !== sha256(host)) throw new Error("credential provenance live identity does not match the requested run");

  const campaignProfile = protectedJson(entry.config_path, "credential provenance profile");
  const campaignErrors = validateCredentialProvenanceProfile(campaignProfile);
  if (campaignErrors.length || campaignProfile.run_id !== runId) throw new Error([...campaignErrors, "credential provenance profile run identity mismatch"].join("; "));

  const prerequisites = validatePrerequisites(dependencies.prerequisites?.() ?? CREDENTIAL_PROVENANCE_PREREQUISITES);
  if (prerequisites.some((item) => item.status === "unavailable")) {
    return unavailable(profile, runId, startedAt, prerequisites, "no credential, provider, deployment, or provenance mutation was started");
  }

  throw new Error("credential provenance mutation campaign is not implemented; refusing to continue");
}

async function main(args) {
  const observation = executeCredentialProvenanceDriver({
    scenarioId: argument(args, "--scenario-id"),
    runId: argument(args, "--run-id"),
    liveProfilePath: resolve(argument(args, "--profile")),
  });
  process.stdout.write(`${JSON.stringify(observation)}\n`);
}

if (process.argv[1] && SCRIPT_PATH === resolve(process.argv[1])) main(process.argv.slice(2)).catch((error) => {
  process.stderr.write(`CELLD_LIVE_CREDENTIAL_PROVENANCE_ERROR ${sha256(error.message)}\n`);
  process.exitCode = 3;
});
