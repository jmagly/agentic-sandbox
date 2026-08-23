#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, lstatSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { resolveQualificationLane } from "./celld-qualification-lanes.mjs";
import { CANDIDATE_LIVE_EVALUATORS, SAFE_LIVE_EVALUATORS } from "./celld-live-evaluators.mjs";
import { LIVE_DRIVERS, validateLiveProfile } from "./celld-uat-live-protocol.mjs";
import { DEFAULT_CATALOG, EXECUTORS, validateCatalog } from "./run-celld-uat.mjs";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
const GIT_COMMIT = /^[0-9a-f]{40}$/;

export const QUALIFICATION_DRIVER_IDS = Object.freeze({
  orchestration: "celld-live-orchestration",
  worker: "celld-live-worker",
  networkAuth: "celld-live-network-auth",
  credentialProvenance: "celld-live-credential-provenance",
  rollout: "celld-live-rollout",
  observability: "celld-live-observability",
  recovery: "celld-live-recovery",
  storageTopology: "celld-live-storage-topology",
});

export const QUALIFICATION_DEPENDENCY_SOURCES = Object.freeze({
  "#763": Object.freeze([".gitea/workflows/celld-qualification.yml", "scripts/celld-uat-live-protocol.mjs", "tests/celld/uat/live-observation-v1.schema.json"]),
  "#764": Object.freeze(["scripts/celld-live-orchestration.mjs", "tests/celld/uat/crash-phase-evidence-v1.schema.json"]),
  "#765": Object.freeze(["scripts/celld-live-network-auth.mjs", "tests/celld/uat/network-auth-evidence-v1.schema.json"]),
  "#766": Object.freeze(["scripts/celld-live-credential-provenance.mjs", "scripts/celld-credential-provenance-controller.mjs", "tests/celld/uat/credential-provenance-evidence-v1.schema.json"]),
  "#767": Object.freeze(["scripts/celld-live-worker.mjs", "scripts/celld-fleet-fixture.mjs", "tests/celld/uat/worker-evidence-v1.schema.json"]),
  "#768": Object.freeze(["scripts/celld-live-rollout.mjs", "scripts/celld-rollout-controller.mjs", "tests/celld/uat/rollout-controller.test.mjs"]),
  "#769": Object.freeze(["scripts/celld-live-observability.mjs", "scripts/celld-observability-controller.mjs", "tests/celld/uat/observability-evidence-v1.schema.json"]),
  "#770": Object.freeze(["scripts/celld-live-recovery.mjs", "scripts/celld-recovery-controller.mjs", "tests/celld/uat/recovery-evidence-v1.schema.json"]),
  "#771": Object.freeze(["scripts/celld-live-offline-migration.mjs", "scripts/celld-offline-migration.mjs", "tests/celld/uat/offline-migration-evidence-v1.schema.json"]),
});

function check(id, passed, evidence) {
  return { id, status: passed ? "PASS" : "FAIL", evidence };
}

function ownFileStatus(repoRoot, path) {
  const target = resolve(repoRoot, path);
  if (!existsSync(target)) return { exists: false, regular_file: false, symlink: false };
  const metadata = lstatSync(target);
  return { exists: true, regular_file: metadata.isFile(), symlink: metadata.isSymbolicLink() };
}

function workflowChecks(workflow) {
  const specs = [
    ["workflow.job_timeout", /timeout-minutes:\s*480/, "480-minute job ceiling"],
    ["workflow.shell_timeout", /timeout\s+--signal=TERM\s+--kill-after=180s\s+420m\s+\\\s*\n\s*node scripts\/run-celld-uat\.mjs/, "420-minute shell ceiling"],
    ["workflow.serial_host", /group:\s*agentic-sandbox-celld-qualification-titan[\s\S]*cancel-in-progress:\s*false/, "non-cancelling Titan reservation"],
    ["workflow.serial_vm", /group:\s*agentic-sandbox-vm-e2e[\s\S]*cancel-in-progress:\s*false/, "non-cancelling VM reservation"],
    ["workflow.start_janitor_preview", /name:\s*Preview stale disposable E2E cleanup/, "start janitor preview"],
    ["workflow.start_janitor", /name:\s*Reap stale disposable E2E resources/, "start janitor"],
    ["workflow.end_janitor_preview", /name:\s*Preview final disposable E2E cleanup/, "end janitor preview"],
    ["workflow.end_janitor", /name:\s*Reap qualification E2E resources/, "end janitor"],
    ["workflow.postflight", /name:\s*Verify post-run resource and capacity baseline[\s\S]*celld-titan-postflight\.mjs/, "postflight baseline comparison"],
    ["workflow.always_upload", /name:\s*Upload qualification evidence\s*\n\s*if:\s*always\(\)/, "all-outcome evidence upload"],
    ["workflow.destructive_owner", /exact_run_owner:\s*\$run_id/, "exact run owner in authorization"],
    ["workflow.readiness_gate", /name:\s*Validate exact-head qualification readiness[\s\S]*celld-qualification-readiness\.mjs/, "exact-head readiness step"],
    ["workflow.profile_readiness_gate", /name:\s*Validate generated live profile readiness[\s\S]*--profile/, "generated-profile readiness step"],
  ];
  return specs.map(([id, pattern, evidence]) => check(id, pattern.test(workflow), evidence));
}

function exactAssertionOwners(scenario) {
  const owners = new Map(scenario.assertions.map((assertion) => [assertion.id, []]));
  for (const id of scenario.execution?.supporting_covers_assertions ?? []) owners.get(id)?.push(`executor:${scenario.execution.supporting_executor_id}`);
  for (const driver of scenario.execution?.live_drivers ?? []) for (const id of driver.covers_assertions ?? []) owners.get(id)?.push(`driver:${driver.id}`);
  return owners;
}

export function evaluateQualificationReadiness({
  catalog,
  laneName,
  expectedGit,
  actualGit,
  gitClean = true,
  workflowSource,
  repoRoot = REPO_ROOT,
  profile = null,
  fileStatus = (path) => ownFileStatus(repoRoot, path),
}) {
  const checks = [];
  let lane;
  try { lane = resolveQualificationLane(laneName); }
  catch (error) {
    return { schema_version: "agentic-sandbox.celld-qualification-readiness/v1", ready: false, lane: laneName, git: { expected: expectedGit, actual: actualGit }, checks: [check("lane.valid", false, error.message)], scenarios: [], assertions: [], drivers: [], dependencies: [] };
  }
  checks.push(check("git.expected", GIT_COMMIT.test(expectedGit ?? ""), expectedGit));
  checks.push(check("git.actual", GIT_COMMIT.test(actualGit ?? ""), actualGit));
  checks.push(check("git.exact_head", expectedGit === actualGit, `${actualGit} == ${expectedGit}`));
  checks.push(check("git.clean", gitClean === true, gitClean ? "checkout clean" : "checkout contains uncommitted changes"));

  const catalogErrors = validateCatalog(catalog, new Set(Object.keys(EXECUTORS)), new Set(Object.keys(LIVE_DRIVERS)));
  checks.push(check("catalog.valid", catalogErrors.length === 0, catalogErrors.length === 0 ? "catalog contract valid" : catalogErrors));
  const byId = new Map((catalog.scenarios ?? []).map((scenario) => [scenario.id, scenario]));
  const scenarios = lane.selectedIds.map((id) => byId.get(id)).filter(Boolean);
  checks.push(check("catalog.selection", scenarios.length === lane.selectedIds.length, `${scenarios.length}/${lane.selectedIds.length} selected scenarios found`));
  if (lane.name === "complete") {
    checks.push(check("catalog.complete_scenarios", scenarios.length === 13, `${scenarios.length}/13 scenarios`));
  }

  const assertionRecords = [];
  const liveAssertionIds = new Set();
  for (const scenario of scenarios) {
    const owners = exactAssertionOwners(scenario);
    for (const assertion of scenario.assertions) {
      const assigned = owners.get(assertion.id) ?? [];
      const live = assigned.find((owner) => owner.startsWith("driver:"));
      if (live) liveAssertionIds.add(assertion.id);
      assertionRecords.push({ scenario_id: scenario.id, assertion_id: assertion.id, owners: assigned, owner_count: assigned.length, formula: live ? (SAFE_LIVE_EVALUATORS[assertion.id] ? "trusted" : CANDIDATE_LIVE_EVALUATORS[assertion.id] ? "candidate_withheld" : "missing") : "supporting_executor" });
    }
  }
  checks.push(check("assertions.one_owner", assertionRecords.every((record) => record.owner_count === 1), `${assertionRecords.filter((record) => record.owner_count === 1).length}/${assertionRecords.length} assertions have one owner`));
  checks.push(check("assertions.live_formulas", assertionRecords.filter((record) => record.owners[0]?.startsWith("driver:")).every((record) => record.formula !== "missing"), `${liveAssertionIds.size} live assertions checked`));
  if (lane.name === "complete") checks.push(check("assertions.complete_count", assertionRecords.length === 44, `${assertionRecords.length}/44 assertions`));

  const driverRecords = [];
  for (const [key, enabled] of Object.entries(lane.drivers)) {
    if (!enabled) continue;
    const id = QUALIFICATION_DRIVER_IDS[key];
    const definition = LIVE_DRIVERS[id];
    const path = definition?.args?.[0];
    const status = path ? fileStatus(path) : { exists: false, regular_file: false, symlink: false };
    driverRecords.push({ key, id, path: path ?? null, installed: Boolean(definition) && status.exists && status.regular_file && !status.symlink });
  }
  checks.push(check("drivers.installed", driverRecords.every((record) => record.installed), `${driverRecords.filter((record) => record.installed).length}/${driverRecords.length} fixed drivers installed`));
  if (lane.name === "complete") checks.push(check("drivers.complete_count", driverRecords.length === Object.keys(QUALIFICATION_DRIVER_IDS).length, `${driverRecords.length}/${Object.keys(QUALIFICATION_DRIVER_IDS).length} complete-lane drivers`));

  const dependencies = Object.entries(QUALIFICATION_DEPENDENCY_SOURCES).map(([issue, paths]) => {
    const sources = paths.map((path) => ({ path, ...fileStatus(path) }));
    return { issue, exact_git: actualGit, sources, acceptable: sources.every((source) => source.exists && source.regular_file && !source.symlink) };
  });
  checks.push(check("dependencies.exact_head_sources", dependencies.every((dependency) => dependency.acceptable), `${dependencies.filter((dependency) => dependency.acceptable).length}/${dependencies.length} dependency source sets at ${actualGit}`));
  checks.push(...workflowChecks(workflowSource));

  const profileRecords = [];
  if (profile !== null) {
    const profileErrors = validateLiveProfile(profile);
    checks.push(check("profile.valid", profileErrors.length === 0, profileErrors.length === 0 ? "live profile contract valid" : profileErrors));
    checks.push(check("profile.exact_git", profile.expected_sandbox_git === actualGit, `${profile.expected_sandbox_git} == ${actualGit}`));
    for (const record of driverRecords) {
      const entry = profile.drivers?.[record.id];
      profileRecords.push({ id: record.id, present: Boolean(entry), enabled: entry?.enabled === true, config_path: entry?.config_path ?? null });
    }
    checks.push(check("profile.required_drivers_enabled", profileRecords.every((record) => record.present && record.enabled && isAbsolute(record.config_path ?? "")), `${profileRecords.filter((record) => record.present && record.enabled && isAbsolute(record.config_path ?? "")).length}/${profileRecords.length} required profile drivers enabled`));
  }

  return {
    schema_version: "agentic-sandbox.celld-qualification-readiness/v1",
    ready: checks.every((entry) => entry.status === "PASS"),
    lane: lane.name,
    git: { expected: expectedGit, actual: actualGit },
    checks,
    scenarios: scenarios.map((scenario) => scenario.id),
    assertions: assertionRecords,
    drivers: driverRecords,
    dependencies,
    profiles: profileRecords,
  };
}

function argument(args, name, required = true) {
  const index = args.indexOf(name);
  if (index < 0 || !args[index + 1]) {
    if (required) throw new Error(`${name} is required`);
    return null;
  }
  return args[index + 1];
}

function protectedJson(path, description) {
  const target = resolve(path);
  if (!existsSync(target)) throw new Error(`${description} is missing`);
  const metadata = lstatSync(target);
  if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) throw new Error(`${description} must be a protected regular non-symlink file`);
  return JSON.parse(readFileSync(target, "utf8"));
}

function gitState() {
  const commit = spawnSync("git", ["rev-parse", "HEAD"], { cwd: REPO_ROOT, encoding: "utf8", shell: false });
  const status = spawnSync("git", ["status", "--porcelain"], { cwd: REPO_ROOT, encoding: "utf8", shell: false });
  if (commit.error || commit.status !== 0 || !GIT_COMMIT.test((commit.stdout ?? "").trim()) || status.error || status.status !== 0) throw new Error("exact Git state is unavailable");
  return { commit: commit.stdout.trim(), clean: (status.stdout ?? "") === "" };
}

function main(args) {
  const laneName = argument(args, "--lane");
  const expectedGit = argument(args, "--expected-git");
  const output = resolve(argument(args, "--output"));
  const profilePath = argument(args, "--profile", false);
  const catalog = JSON.parse(readFileSync(DEFAULT_CATALOG, "utf8"));
  const workflowSource = readFileSync(resolve(REPO_ROOT, ".gitea/workflows/celld-qualification.yml"), "utf8");
  const profile = profilePath === null ? null : protectedJson(profilePath, "live profile");
  const repository = gitState();
  const result = evaluateQualificationReadiness({ catalog, laneName, expectedGit, actualGit: repository.commit, gitClean: repository.clean, workflowSource, profile });
  mkdirSync(dirname(output), { recursive: true, mode: 0o700 });
  writeFileSync(output, `${JSON.stringify({ ...result, generated_at: new Date().toISOString() }, null, 2)}\n`, { mode: 0o600, flag: "wx" });
  chmodSync(output, 0o600);
  process.stdout.write(`${JSON.stringify({ ready: result.ready, lane: result.lane, git: result.git, checks: result.checks.length, failed: result.checks.filter((entry) => entry.status === "FAIL").map((entry) => entry.id) })}\n`);
  if (!result.ready) process.exitCode = 3;
}

if (process.argv[1] && SCRIPT_PATH === resolve(process.argv[1])) {
  try { main(process.argv.slice(2)); }
  catch (error) {
    process.stderr.write(`CELLD_QUALIFICATION_READINESS_ERROR ${error.message}\n`);
    process.exitCode = 3;
  }
}
