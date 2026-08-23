#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, lstatSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { hostname } from "node:os";
import { dirname, isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { RecoveryCleanupError, executeRecoveryCampaign } from "./celld-recovery-controller.mjs";
import { validateOrchestrationConfig } from "./celld-live-orchestration.mjs";
import { LIVE_OBSERVATION_SCHEMA, validateLiveProfile } from "./celld-uat-live-protocol.mjs";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
const DRIVER_ID = "celld-live-recovery";
const DRIVER_VERSION = "celld-live-recovery/v1";
const SCENARIO_ID = "UAT-CELLD-015";

const RECOVERY_PREREQUISITE_SPECS = Object.freeze([
  Object.freeze({ id: "CELLD_CREDENTIAL_PROVENANCE_CAMPAIGN", unavailable: "CELLD_CREDENTIAL_PROVENANCE_AUTHORIZATION_REQUIRED", available: "CELLD_CREDENTIAL_PROVENANCE_CAMPAIGN_READY" }),
  Object.freeze({ id: "CELLD_OBSERVABILITY_QUALIFICATION", unavailable: "CELLD_OBSERVABILITY_QUALIFICATION_UNAVAILABLE", available: "CELLD_OBSERVABILITY_QUALIFICATION_READY" }),
  Object.freeze({ id: "CELLD_VERSIONED_SNAPSHOT_FIXTURE", unavailable: "CELLD_VERSIONED_SNAPSHOT_RESTORE_UNAVAILABLE", available: "CELLD_VERSIONED_SNAPSHOT_RESTORE_READY" }),
  Object.freeze({ id: "CELLD_INDEPENDENT_EVIDENCE_STORE", unavailable: "CELLD_INDEPENDENT_EVIDENCE_STORE_UNAVAILABLE", available: "CELLD_INDEPENDENT_EVIDENCE_STORE_READY" }),
]);

function prerequisiteSet(status) {
  return Object.freeze(RECOVERY_PREREQUISITE_SPECS.map((spec) => Object.freeze({ id: spec.id, status, reason_code: spec[status] })));
}

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

export const RECOVERY_PREREQUISITES = prerequisiteSet("unavailable");
export const RECOVERY_READY_PREREQUISITES = prerequisiteSet("available");

function validatePrerequisites(prerequisites) {
  if (!Array.isArray(prerequisites) || prerequisites.length !== RECOVERY_PREREQUISITE_SPECS.length) throw new Error("recovery prerequisites must contain the exact prerequisite inventory");
  const expected = new Map(RECOVERY_PREREQUISITE_SPECS.map((spec) => [spec.id, spec]));
  const byId = new Map();
  for (const prerequisite of prerequisites) {
    if (!prerequisite || typeof prerequisite !== "object" || Array.isArray(prerequisite) || Object.keys(prerequisite).some((key) => !["id", "status", "reason_code"].includes(key))) throw new Error("recovery prerequisite is invalid");
    const spec = expected.get(prerequisite.id);
    if (!spec || byId.has(prerequisite.id) || !["available", "unavailable"].includes(prerequisite.status) || prerequisite.reason_code !== spec[prerequisite.status]) throw new Error("recovery prerequisite does not match the exact status contract");
    byId.set(prerequisite.id, { ...prerequisite });
  }
  if (byId.size !== expected.size) throw new Error("recovery prerequisites do not match the exact inventory");
  return RECOVERY_PREREQUISITE_SPECS.map((spec) => byId.get(spec.id));
}

function unavailable(profile, runId, startedAt, prerequisites) {
  return {
    schema_version: LIVE_OBSERVATION_SCHEMA, driver_id: DRIVER_ID, run_id: runId, scenario_id: SCENARIO_ID,
    started_at: startedAt, ended_at: new Date().toISOString(), mutation_started: false, prerequisites, assertions: [],
    identities: { profile_id: profile.profile_id, sandbox_git: profile.expected_sandbox_git, environment_host_sha256: profile.environment.host_sha256, driver_version: DRIVER_VERSION },
    metrics: [], faults: [], artifacts: [], cleanup: { status: "not_required", assertions: ["no snapshot or restore mutation was started"] },
  };
}

function artifact(path, relativePath) {
  const bytes = readFileSync(path);
  return { path: relativePath, mime_type: relativePath.endsWith(".jsonl") ? "application/x-ndjson" : "application/json", sha256: sha256(bytes), bytes: bytes.length, contains_restricted_data: false };
}

function campaignAssertions(campaign, evidenceRefs) {
  return [
    { id: "CELLD.015.OBJECTIVES", measurements: { restores: campaign.restores }, evidence_refs: evidenceRefs },
    { id: "CELLD.015.IDEMPOTENT", measurements: { runbooks: campaign.runbooks }, evidence_refs: evidenceRefs },
    { id: "CELLD.015.EVIDENCE", measurements: campaign.evidence, evidence_refs: evidenceRefs },
  ];
}

export async function executeRecoveryDriver({ scenarioId, runId, liveProfilePath, artifactDir }, dependencies = {}) {
  const startedAt = new Date().toISOString();
  const profile = protectedJson(liveProfilePath, "live profile");
  const profileErrors = validateLiveProfile(profile);
  if (profileErrors.length) throw new Error(profileErrors.join("; "));
  const entry = profile.drivers?.[DRIVER_ID];
  if (!entry?.enabled) return unavailable(profile, runId, startedAt, [{ id: "CELLD_LIVE_RECOVERY", status: "unavailable", reason_code: "CELLD_LIVE_RECOVERY_DRIVER_DISABLED" }]);
  const git = dependencies.gitCommit?.() ?? gitCommit(), host = dependencies.hostname?.() ?? hostname();
  if (scenarioId !== SCENARIO_ID || profile.run_id !== runId || profile.expected_sandbox_git !== git || profile.environment.host_sha256 !== sha256(host)) throw new Error("recovery live identity does not match the requested run");
  const config = protectedJson(entry.config_path, "orchestration config");
  const configErrors = validateOrchestrationConfig(config);
  if (configErrors.length || config.run_id !== runId) throw new Error([...configErrors, "config run identity mismatch"].join("; "));

  const prerequisites = validatePrerequisites(dependencies.prerequisites?.() ?? RECOVERY_PREREQUISITES);
  if (prerequisites.some((item) => item.status === "unavailable")) return unavailable(profile, runId, startedAt, prerequisites);
  if (!profile.authorization.destructive_faults || profile.authorization.exact_run_owner !== runId) return unavailable(profile, runId, startedAt, [{ id: "CELLD_RECOVERY_AUTHORIZATION", status: "unavailable", reason_code: "CELLD_DESTRUCTIVE_AUTHORIZATION_REQUIRED" }]);
  if (!dependencies.recoveryAdapter) return unavailable(profile, runId, startedAt, [{ id: "CELLD_RECOVERY_ADAPTER", status: "unavailable", reason_code: "CELLD_RECOVERY_ADAPTER_UNAVAILABLE" }]);

  const root = resolve(artifactDir ?? "");
  if (!isAbsolute(artifactDir ?? "")) throw new Error("recovery artifact directory must be absolute");
  mkdirSync(root, { recursive: true, mode: 0o700 });
  chmodSync(root, 0o700);
  const campaign = await (dependencies.executeCampaign ?? executeRecoveryCampaign)({ runId, adapter: dependencies.recoveryAdapter });
  const evidencePath = resolve(root, "recovery-evidence.json");
  const timelinePath = resolve(root, "recovery-timeline.jsonl");
  writeFileSync(evidencePath, `${JSON.stringify({
    schema_version: "agentic-sandbox.celld-recovery-evidence/v1",
    run_id: runId,
    restores: campaign.restores,
    runbooks: campaign.runbooks,
    evidence: campaign.evidence,
    baseline: campaign.baseline,
    cleanup: campaign.cleanup,
  }, null, 2)}\n`, { mode: 0o600, flag: "wx" });
  writeFileSync(timelinePath, `${campaign.timeline.map((entry) => JSON.stringify(entry)).join("\n")}\n`, { mode: 0o600, flag: "wx" });
  chmodSync(evidencePath, 0o600);
  chmodSync(timelinePath, 0o600);
  const artifacts = [artifact(evidencePath, "artifacts/recovery-evidence.json"), artifact(timelinePath, "artifacts/recovery-timeline.jsonl")];
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
    identities: { profile_id: profile.profile_id, sandbox_git: profile.expected_sandbox_git, environment_host_sha256: profile.environment.host_sha256, driver_version: DRIVER_VERSION },
    metrics: [
      { name: "recovery_restore_exercises", value: campaign.restores.length, unit: "restores" },
      { name: "recovery_runbook_executions", value: campaign.runbooks.reduce((sum, runbook) => sum + runbook.executions.length, 0), unit: "executions" },
      { name: "external_recovery_evidence", value: campaign.evidence.artifacts.length, unit: "artifacts" },
    ],
    faults: [{ kind: "affected_fleet_loss", healed: true }, ...campaign.runbooks.map((runbook) => ({ kind: runbook.runbook, healed: runbook.healed }))],
    artifacts,
    cleanup: { status: "passed", assertions: ["source writers and restore authority returned to baseline", "the affected fleet was restored", "isolated restore and snapshot fixtures were removed", "external recovery evidence remains retained"] },
  };
}

async function main(args) {
  const observation = await executeRecoveryDriver({ scenarioId: argument(args, "--scenario-id"), runId: argument(args, "--run-id"), liveProfilePath: resolve(argument(args, "--profile")), artifactDir: resolve(argument(args, "--artifact-dir")) });
  process.stdout.write(`${JSON.stringify(observation)}\n`);
}

if (process.argv[1] && SCRIPT_PATH === resolve(process.argv[1])) main(process.argv.slice(2)).catch((error) => { process.stderr.write(`CELLD_LIVE_RECOVERY_ERROR ${sha256(error.message)}\n`); process.exitCode = error instanceof RecoveryCleanupError ? 4 : 3; });
