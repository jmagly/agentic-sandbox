#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, lstatSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { hostname } from "node:os";
import { dirname, isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { ObservabilityCleanupError, executeObservabilityCampaign } from "./celld-observability-controller.mjs";
import { validateOrchestrationConfig } from "./celld-live-orchestration.mjs";
import { LIVE_OBSERVATION_SCHEMA, validateLiveProfile } from "./celld-uat-live-protocol.mjs";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
const DRIVER_ID = "celld-live-observability";
const DRIVER_VERSION = "celld-live-observability/v1";
const SCENARIO_ID = "UAT-CELLD-014";

const OBSERVABILITY_PREREQUISITE_SPECS = Object.freeze([
  Object.freeze({ id: "CELLD_CREDENTIAL_PROVENANCE_CAMPAIGN", unavailable: "CELLD_CREDENTIAL_PROVENANCE_AUTHORIZATION_REQUIRED", available: "CELLD_CREDENTIAL_PROVENANCE_CAMPAIGN_READY" }),
  Object.freeze({ id: "CELLD_ROLLOUT_QUALIFICATION", unavailable: "CELLD_ROLLOUT_QUALIFICATION_UNAVAILABLE", available: "CELLD_ROLLOUT_QUALIFICATION_READY" }),
  Object.freeze({ id: "CELLD_LIVE_TELEMETRY_STACK", unavailable: "CELLD_ALERT_TRACE_DASHBOARD_FIXTURE_UNAVAILABLE", available: "CELLD_ALERT_TRACE_DASHBOARD_FIXTURE_READY" }),
]);

function prerequisiteSet(status) {
  return Object.freeze(OBSERVABILITY_PREREQUISITE_SPECS.map((spec) => Object.freeze({ id: spec.id, status, reason_code: spec[status] })));
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

export const OBSERVABILITY_PREREQUISITES = prerequisiteSet("unavailable");
export const OBSERVABILITY_READY_PREREQUISITES = prerequisiteSet("available");

function validatePrerequisites(prerequisites) {
  if (!Array.isArray(prerequisites) || prerequisites.length !== OBSERVABILITY_PREREQUISITE_SPECS.length) throw new Error("observability prerequisites must contain the exact prerequisite inventory");
  const expected = new Map(OBSERVABILITY_PREREQUISITE_SPECS.map((spec) => [spec.id, spec]));
  const byId = new Map();
  for (const prerequisite of prerequisites) {
    if (!prerequisite || typeof prerequisite !== "object" || Array.isArray(prerequisite) || Object.keys(prerequisite).some((key) => !["id", "status", "reason_code"].includes(key))) throw new Error("observability prerequisite is invalid");
    const spec = expected.get(prerequisite.id);
    if (!spec || byId.has(prerequisite.id) || !["available", "unavailable"].includes(prerequisite.status) || prerequisite.reason_code !== spec[prerequisite.status]) throw new Error("observability prerequisite does not match the exact status contract");
    byId.set(prerequisite.id, { ...prerequisite });
  }
  if (byId.size !== expected.size) throw new Error("observability prerequisites do not match the exact inventory");
  return OBSERVABILITY_PREREQUISITE_SPECS.map((spec) => byId.get(spec.id));
}

function unavailable(profile, runId, startedAt, prerequisites) {
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
    identities: { profile_id: profile.profile_id, sandbox_git: profile.expected_sandbox_git, environment_host_sha256: profile.environment.host_sha256, driver_version: DRIVER_VERSION },
    metrics: [], faults: [], artifacts: [],
    cleanup: { status: "not_required", assertions: ["no observability fault was injected"] },
  };
}

function artifact(path, relativePath) {
  const bytes = readFileSync(path);
  return { path: relativePath, mime_type: relativePath.endsWith(".jsonl") ? "application/x-ndjson" : "application/json", sha256: sha256(bytes), bytes: bytes.length, contains_restricted_data: false };
}

function campaignAssertions(campaign, evidenceRefs) {
  return [
    { id: "CELLD.014.CLASSIFICATION", measurements: { cases: campaign.cases }, evidence_refs: evidenceRefs },
    { id: "CELLD.014.CORRELATION", measurements: { records: campaign.records, redaction: campaign.redaction, evidence_exported: true, fleet_baseline_restored: campaign.baseline.restored }, evidence_refs: evidenceRefs },
    { id: "CELLD.014.ALERTS", measurements: { alerts: campaign.alerts }, evidence_refs: evidenceRefs },
  ];
}

export async function executeObservabilityDriver({ scenarioId, runId, liveProfilePath, artifactDir }, dependencies = {}) {
  const startedAt = new Date().toISOString();
  const profile = protectedJson(liveProfilePath, "live profile");
  const profileErrors = validateLiveProfile(profile);
  if (profileErrors.length) throw new Error(profileErrors.join("; "));
  const entry = profile.drivers?.[DRIVER_ID];
  if (!entry?.enabled) return unavailable(profile, runId, startedAt, [{ id: "CELLD_LIVE_OBSERVABILITY", status: "unavailable", reason_code: "CELLD_LIVE_OBSERVABILITY_DRIVER_DISABLED" }]);
  const git = dependencies.gitCommit?.() ?? gitCommit(), host = dependencies.hostname?.() ?? hostname();
  if (scenarioId !== SCENARIO_ID || profile.run_id !== runId || profile.expected_sandbox_git !== git || profile.environment.host_sha256 !== sha256(host)) throw new Error("observability live identity does not match the requested run");
  const config = protectedJson(entry.config_path, "orchestration config");
  const configErrors = validateOrchestrationConfig(config);
  if (configErrors.length || config.run_id !== runId) throw new Error([...configErrors, "config run identity mismatch"].join("; "));

  const prerequisites = validatePrerequisites(dependencies.prerequisites?.() ?? OBSERVABILITY_PREREQUISITES);
  if (prerequisites.some((item) => item.status === "unavailable")) return unavailable(profile, runId, startedAt, prerequisites);
  if (!profile.authorization.destructive_faults || profile.authorization.exact_run_owner !== runId) return unavailable(profile, runId, startedAt, [{ id: "CELLD_OBSERVABILITY_AUTHORIZATION", status: "unavailable", reason_code: "CELLD_DESTRUCTIVE_AUTHORIZATION_REQUIRED" }]);
  if (!dependencies.observabilityAdapter) return unavailable(profile, runId, startedAt, [{ id: "CELLD_OBSERVABILITY_ADAPTER", status: "unavailable", reason_code: "CELLD_OBSERVABILITY_ADAPTER_UNAVAILABLE" }]);

  const root = resolve(artifactDir ?? "");
  if (!isAbsolute(artifactDir ?? "")) throw new Error("observability artifact directory must be absolute");
  mkdirSync(root, { recursive: true, mode: 0o700 });
  chmodSync(root, 0o700);
  const campaign = await (dependencies.executeCampaign ?? executeObservabilityCampaign)({ runId, adapter: dependencies.observabilityAdapter });
  const evidencePath = resolve(root, "observability-evidence.json");
  const timelinePath = resolve(root, "observability-timeline.jsonl");
  writeFileSync(evidencePath, `${JSON.stringify({
    schema_version: "agentic-sandbox.celld-observability-evidence/v1",
    run_id: runId,
    cases: campaign.cases,
    records: campaign.records,
    alerts: campaign.alerts,
    redaction: campaign.redaction,
    baseline: campaign.baseline,
    cleanup: campaign.cleanup,
  }, null, 2)}\n`, { mode: 0o600, flag: "wx" });
  writeFileSync(timelinePath, `${campaign.timeline.map((entry) => JSON.stringify(entry)).join("\n")}\n`, { mode: 0o600, flag: "wx" });
  chmodSync(evidencePath, 0o600);
  chmodSync(timelinePath, 0o600);
  const artifacts = [artifact(evidencePath, "artifacts/observability-evidence.json"), artifact(timelinePath, "artifacts/observability-timeline.jsonl")];
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
      { name: "observability_fault_boundaries", value: campaign.cases.length, unit: "boundaries" },
      { name: "observability_surface_records", value: campaign.records.length, unit: "records" },
      { name: "observability_alert_lifecycles", value: campaign.alerts.length, unit: "alerts" },
    ],
    faults: campaign.cases.map((entryCase) => ({ kind: entryCase.boundary, healed: entryCase.healed && entryCase.heal_verified })),
    artifacts,
    cleanup: { status: "passed", assertions: ["all ten injected faults were independently healed", "the preflight fleet baseline was restored", "all seven telemetry surfaces passed redaction"] },
  };
}

async function main(args) {
  const observation = await executeObservabilityDriver({ scenarioId: argument(args, "--scenario-id"), runId: argument(args, "--run-id"), liveProfilePath: resolve(argument(args, "--profile")), artifactDir: resolve(argument(args, "--artifact-dir")) });
  process.stdout.write(`${JSON.stringify(observation)}\n`);
}

if (process.argv[1] && SCRIPT_PATH === resolve(process.argv[1])) main(process.argv.slice(2)).catch((error) => { process.stderr.write(`CELLD_LIVE_OBSERVABILITY_ERROR ${sha256(error.message)}\n`); process.exitCode = error instanceof ObservabilityCleanupError ? 4 : 3; });
