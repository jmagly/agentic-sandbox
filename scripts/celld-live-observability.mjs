#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, lstatSync, readFileSync } from "node:fs";
import { hostname } from "node:os";
import { dirname, isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { validateOrchestrationConfig } from "./celld-live-orchestration.mjs";
import { validateLiveProfile } from "./celld-uat-live-protocol.mjs";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
const DRIVER_ID = "celld-live-observability";
const DRIVER_VERSION = "celld-live-observability/v1";
const SCENARIO_ID = "UAT-CELLD-014";
const OBSERVATION_SCHEMA = "agentic-sandbox.celld-live-observation/v1";

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

export const OBSERVABILITY_PREREQUISITES = Object.freeze([
  Object.freeze({ id: "CELLD_CREDENTIAL_PROVENANCE_CAMPAIGN", status: "unavailable", reason_code: "CELLD_CREDENTIAL_PROVENANCE_AUTHORIZATION_REQUIRED" }),
  Object.freeze({ id: "CELLD_ROLLOUT_QUALIFICATION", status: "unavailable", reason_code: "CELLD_ROLLOUT_QUALIFICATION_UNAVAILABLE" }),
  Object.freeze({ id: "CELLD_LIVE_TELEMETRY_STACK", status: "unavailable", reason_code: "CELLD_ALERT_TRACE_DASHBOARD_FIXTURE_UNAVAILABLE" }),
]);

function unavailable(profile, runId, startedAt, prerequisites) {
  return {
    schema_version: OBSERVATION_SCHEMA,
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

export function executeObservabilityDriver({ scenarioId, runId, liveProfilePath }, dependencies = {}) {
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
  return unavailable(profile, runId, startedAt, dependencies.prerequisites?.() ?? OBSERVABILITY_PREREQUISITES);
}

async function main(args) {
  const observation = executeObservabilityDriver({ scenarioId: argument(args, "--scenario-id"), runId: argument(args, "--run-id"), liveProfilePath: resolve(argument(args, "--profile")) });
  process.stdout.write(`${JSON.stringify(observation)}\n`);
}

if (process.argv[1] && SCRIPT_PATH === resolve(process.argv[1])) main(process.argv.slice(2)).catch((error) => { process.stderr.write(`CELLD_LIVE_OBSERVABILITY_ERROR ${sha256(error.message)}\n`); process.exitCode = 3; });
