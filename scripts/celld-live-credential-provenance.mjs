#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, lstatSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { hostname } from "node:os";
import { dirname, isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { LIVE_OBSERVATION_SCHEMA, validateLiveProfile } from "./celld-uat-live-protocol.mjs";
import { CredentialCampaignCleanupError, executeCredentialProvenanceCampaign } from "./celld-credential-provenance-controller.mjs";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
const DRIVER_ID = "celld-live-credential-provenance";
const DRIVER_VERSION = "celld-live-credential-provenance/v1";
const SCENARIO_ID = "UAT-CELLD-013";
const RUN_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const REASON_CODE = /^[A-Z0-9][A-Z0-9_.-]+$/;

export const CREDENTIAL_PROVENANCE_PROFILE_SCHEMA = "agentic-sandbox.celld-live-credential-provenance/v1";

const CREDENTIAL_PROVENANCE_PREREQUISITE_SPECS = Object.freeze([
  Object.freeze({ id: "CELLD_CREDENTIAL_PROVENANCE_AUTHORIZATION", unavailable: "CELLD_CREDENTIAL_PROVENANCE_AUTHORIZATION_REQUIRED", available: "CELLD_CREDENTIAL_PROVENANCE_AUTHORIZATION_READY" }),
  Object.freeze({ id: "CELLD_STORAGE_QUALIFICATION", unavailable: "CELLD_STORAGE_QUALIFICATION_UNAVAILABLE", available: "CELLD_STORAGE_QUALIFICATION_READY" }),
  Object.freeze({ id: "CELLD_FLEET_QUALIFICATION", unavailable: "CELLD_FLEET_QUALIFICATION_UNAVAILABLE", available: "CELLD_FLEET_QUALIFICATION_READY" }),
  Object.freeze({ id: "CELLD_NETWORK_AUTH_QUALIFICATION", unavailable: "CELLD_NETWORK_AUTH_QUALIFICATION_UNAVAILABLE", available: "CELLD_NETWORK_AUTH_QUALIFICATION_READY" }),
  Object.freeze({ id: "CELLD_BROKER_INTEGRATED_FLEET", unavailable: "CELLD_BROKER_INTEGRATED_FLEET_UNAVAILABLE", available: "CELLD_BROKER_INTEGRATED_FLEET_READY" }),
  Object.freeze({ id: "CELLD_S3_CREDENTIAL_ROTATION_CONTROL", unavailable: "CELLD_S3_CREDENTIAL_ROTATION_CONTROL_UNAVAILABLE", available: "CELLD_S3_CREDENTIAL_ROTATION_CONTROL_READY" }),
  Object.freeze({ id: "CELLD_REQUEST_HMAC_ROTATION_CONTROL", unavailable: "CELLD_REQUEST_HMAC_ROTATION_CONTROL_UNAVAILABLE", available: "CELLD_REQUEST_HMAC_ROTATION_CONTROL_READY" }),
  Object.freeze({ id: "CELLD_MTLS_IDENTITY_ROTATION_CONTROL", unavailable: "CELLD_MTLS_IDENTITY_ROTATION_CONTROL_UNAVAILABLE", available: "CELLD_MTLS_IDENTITY_ROTATION_CONTROL_READY" }),
  Object.freeze({ id: "CELLD_PEER_SECRET_ROTATION_CONTROL", unavailable: "CELLD_PEER_SECRET_ROTATION_CONTROL_UNAVAILABLE", available: "CELLD_PEER_SECRET_ROTATION_CONTROL_READY" }),
  Object.freeze({ id: "CELLD_FIXTURE_ADMIN_LIFECYCLE_CONTROL", unavailable: "CELLD_FIXTURE_ADMIN_LIFECYCLE_CONTROL_UNAVAILABLE", available: "CELLD_FIXTURE_ADMIN_LIFECYCLE_CONTROL_READY" }),
  Object.freeze({ id: "CELLD_SECRET_SCAN_SURFACES", unavailable: "CELLD_SECRET_SCAN_SURFACES_UNAVAILABLE", available: "CELLD_SECRET_SCAN_SURFACES_READY" }),
  Object.freeze({ id: "CELLD_SIGNED_ARTIFACT_VERIFIER", unavailable: "CELLD_SIGNED_ARTIFACT_VERIFIER_UNAVAILABLE", available: "CELLD_SIGNED_ARTIFACT_VERIFIER_READY" }),
  Object.freeze({ id: "CELLD_SCOPED_OBJECT_STORE_IDENTITIES", unavailable: "CELLD_SCOPED_OBJECT_STORE_IDENTITIES_UNAVAILABLE", available: "CELLD_SCOPED_OBJECT_STORE_IDENTITIES_READY" }),
  Object.freeze({ id: "CELLD_SUPPORT_BUNDLE_EXPORTER", unavailable: "CELLD_SUPPORT_BUNDLE_EXPORTER_UNAVAILABLE", available: "CELLD_SUPPORT_BUNDLE_EXPORTER_READY" }),
]);

function prerequisiteSet(status) {
  return Object.freeze(CREDENTIAL_PROVENANCE_PREREQUISITE_SPECS.map((spec) => Object.freeze({
    id: spec.id,
    status,
    reason_code: spec[status],
  })));
}

export const CREDENTIAL_PROVENANCE_PREREQUISITES = prerequisiteSet("unavailable");
export const CREDENTIAL_PROVENANCE_READY_PREREQUISITES = prerequisiteSet("available");

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
  if (!Array.isArray(prerequisites) || prerequisites.length !== CREDENTIAL_PROVENANCE_PREREQUISITE_SPECS.length) throw new Error("credential provenance prerequisites must contain the exact prerequisite inventory");
  const expected = new Map(CREDENTIAL_PROVENANCE_PREREQUISITE_SPECS.map((spec) => [spec.id, spec]));
  const ids = new Set();
  for (const prerequisite of prerequisites) {
    if (!prerequisite || typeof prerequisite !== "object" || Array.isArray(prerequisite)) throw new Error("credential provenance prerequisite must be an object");
    if (Object.keys(prerequisite).some((key) => !["id", "status", "reason_code"].includes(key))) throw new Error("credential provenance prerequisite contains an unknown field");
    if (!REASON_CODE.test(prerequisite.id ?? "") || ids.has(prerequisite.id)) throw new Error("credential provenance prerequisite id is invalid or duplicated");
    if (!["available", "unavailable"].includes(prerequisite.status)) throw new Error(`credential provenance prerequisite ${prerequisite.id} has an invalid status`);
    if (!REASON_CODE.test(prerequisite.reason_code ?? "")) throw new Error(`credential provenance prerequisite ${prerequisite.id} has an invalid reason code`);
    const spec = expected.get(prerequisite.id);
    if (!spec) throw new Error(`credential provenance prerequisite ${prerequisite.id} is not in the exact inventory`);
    if (prerequisite.reason_code !== spec[prerequisite.status]) throw new Error(`credential provenance prerequisite ${prerequisite.id} reason code does not match status`);
    ids.add(prerequisite.id);
  }
  if (ids.size !== expected.size || [...expected.keys()].some((id) => !ids.has(id))) throw new Error("credential provenance prerequisites do not match the exact inventory");
  const byId = new Map(prerequisites.map((item) => [item.id, item]));
  return CREDENTIAL_PROVENANCE_PREREQUISITE_SPECS.map((spec) => ({ ...byId.get(spec.id) }));
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

function artifact(path, relativePath) {
  const bytes = readFileSync(path);
  return {
    path: relativePath,
    mime_type: relativePath.endsWith(".jsonl") ? "application/x-ndjson" : "application/json",
    sha256: sha256(bytes),
    bytes: bytes.length,
    contains_restricted_data: false,
  };
}

function campaignAssertions(campaign, evidenceRefs) {
  return [
    {
      id: "CELLD.013.NO_LEAK",
      measurements: {
        protected_credentials: campaign.protected_credentials.map((entry) => ({ ...entry, revoked_or_removed: campaign.cleanup.all_disposable_secrets_removed })),
        scans: campaign.scans,
        unprotected_secret_files: campaign.cleanup.unprotected_secret_files,
        evidence_secret_findings: campaign.cleanup.evidence_secret_findings,
        all_disposable_secrets_removed: campaign.cleanup.all_disposable_secrets_removed,
      },
      evidence_refs: evidenceRefs,
    },
    {
      id: "CELLD.013.SCOPE",
      measurements: {
        lifecycles: campaign.lifecycles,
        scope_mode: "per_fleet_bucket",
        shared_prefix_claimed: false,
        source_bucket_sha256: campaign.scope.source_bucket_sha256,
        other_fleet_bucket_count: campaign.scope.other_bucket_sha256.length,
        cross_scope_cases: campaign.scope.cases,
        ...campaign.hmac,
      },
      evidence_refs: evidenceRefs,
    },
    {
      id: "CELLD.013.PROVENANCE",
      measurements: { cases: campaign.provenance, ...campaign.pins },
      evidence_refs: evidenceRefs,
    },
  ];
}

export async function executeCredentialProvenanceDriver({ scenarioId, runId, liveProfilePath, artifactDir }, dependencies = {}) {
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

  if (!profile.authorization.destructive_faults || profile.authorization.exact_run_owner !== runId) {
    return unavailable(profile, runId, startedAt, [{ id: "CELLD_CREDENTIAL_PROVENANCE_AUTHORIZATION", status: "unavailable", reason_code: "CELLD_CREDENTIAL_PROVENANCE_AUTHORIZATION_REQUIRED" }], "no credential, provider, deployment, or provenance mutation was started");
  }
  if (!dependencies.campaignAdapter) {
    return unavailable(profile, runId, startedAt, [{ id: "CELLD_CREDENTIAL_PROVENANCE_ADAPTER", status: "unavailable", reason_code: "CELLD_CREDENTIAL_PROVENANCE_ADAPTER_UNAVAILABLE" }], "no credential, provider, deployment, or provenance mutation was started");
  }

  const root = resolve(artifactDir ?? "");
  if (!isAbsolute(artifactDir ?? "")) throw new Error("credential provenance artifact directory must be absolute");
  mkdirSync(root, { recursive: true, mode: 0o700 });
  chmodSync(root, 0o700);
  const campaign = await (dependencies.executeCampaign ?? executeCredentialProvenanceCampaign)({ runId, adapter: dependencies.campaignAdapter });
  const evidencePath = resolve(root, "credential-provenance-evidence.json");
  const timelinePath = resolve(root, "credential-provenance-timeline.jsonl");
  writeFileSync(evidencePath, `${JSON.stringify({
    schema_version: "agentic-sandbox.celld-credential-provenance-evidence/v1",
    run_id: runId,
    protected_credentials: campaign.protected_credentials,
    lifecycles: campaign.lifecycles,
    scope: campaign.scope,
    hmac: campaign.hmac,
    provenance: campaign.provenance,
    scans: campaign.scans,
    pins: campaign.pins,
    expected_inventory_sha256: campaign.expected_inventory_sha256,
    cleanup: campaign.cleanup,
  }, null, 2)}\n`, { mode: 0o600, flag: "wx" });
  writeFileSync(timelinePath, `${campaign.timeline.map((entry) => JSON.stringify(entry)).join("\n")}\n`, { mode: 0o600, flag: "wx" });
  chmodSync(evidencePath, 0o600);
  chmodSync(timelinePath, 0o600);
  const artifacts = [
    artifact(evidencePath, "artifacts/credential-provenance-evidence.json"),
    artifact(timelinePath, "artifacts/credential-provenance-timeline.jsonl"),
  ];
  const evidenceRefs = artifacts.map((entryArtifact) => entryArtifact.path);
  return {
    schema_version: LIVE_OBSERVATION_SCHEMA,
    driver_id: DRIVER_ID,
    run_id: runId,
    scenario_id: SCENARIO_ID,
    started_at: startedAt,
    ended_at: new Date().toISOString(),
    mutation_started: campaign.mutation_started,
    prerequisites,
    assertions: campaignAssertions(campaign, evidenceRefs),
    identities: {
      profile_id: profile.profile_id,
      sandbox_git: profile.expected_sandbox_git,
      environment_host_sha256: profile.environment.host_sha256,
      driver_version: DRIVER_VERSION,
    },
    metrics: [
      { name: "credential_lifecycles", value: campaign.lifecycles.length, unit: "domains" },
      { name: "cross_scope_denials", value: campaign.scope.cases.reduce((sum, entryCase) => sum + entryCase.denied, 0), unit: "requests" },
      { name: "provenance_mismatch_cases", value: campaign.provenance.length, unit: "cases" },
    ],
    faults: [
      { kind: "credential_revocation", healed: true },
      { kind: "failed_hmac_canary", healed: true },
      { kind: "provenance_mismatch", healed: true },
    ],
    artifacts,
    cleanup: { status: "passed", assertions: ["all disposable credentials and campaign resources were removed", "all required scan surfaces contained zero credential canaries"] },
  };
}

async function main(args) {
  const observation = await executeCredentialProvenanceDriver({
    scenarioId: argument(args, "--scenario-id"),
    runId: argument(args, "--run-id"),
    liveProfilePath: resolve(argument(args, "--profile")),
    artifactDir: resolve(argument(args, "--artifact-dir")),
  });
  process.stdout.write(`${JSON.stringify(observation)}\n`);
}

if (process.argv[1] && SCRIPT_PATH === resolve(process.argv[1])) main(process.argv.slice(2)).catch((error) => {
  process.stderr.write(`CELLD_LIVE_CREDENTIAL_PROVENANCE_ERROR ${sha256(error.message)}\n`);
  process.exitCode = error instanceof CredentialCampaignCleanupError ? 4 : 3;
});
