#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { arch, hostname, platform, release } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  LIVE_DRIVERS,
  evaluateLiveObservation,
  runSafeLiveDriver,
  validateLiveProfile,
} from "./celld-uat-live-protocol.mjs";
import { SAFE_LIVE_EVALUATORS } from "./celld-live-evaluators.mjs";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
export const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
export const DEFAULT_CATALOG = join(REPO_ROOT, "tests/celld/uat/scenarios.json");
export const EVIDENCE_SCHEMA = "agentic-sandbox.celld-uat-evidence/v1";
export const LIVE_EVALUATORS = SAFE_LIVE_EVALUATORS;

export const EXECUTORS = Object.freeze({
  "celld-disabled-regression": Object.freeze({
    program: process.execPath,
    args: ["scripts/celld-disabled-uat.mjs"],
    timeout_ms: 50 * 60 * 1000,
  }),
  "celld-contract-static": Object.freeze({
    program: process.execPath,
    args: ["scripts/celld-uat-contract-check.mjs"],
    timeout_ms: 30_000,
  }),
  "celld-qualification-deterministic": Object.freeze({
    program: "make",
    args: ["test-celld"],
    timeout_ms: 600_000,
  }),
  "celld-storage-deterministic": Object.freeze({
    program: process.execPath,
    args: ["--test", "tests/celld/uat/storage-qualifier.test.mjs"],
    timeout_ms: 30_000,
  }),
});

const LANES = new Set(["orchestration", "worker", "fleet", "security", "operations", "cross-cutting"]);
const TRIGGERS = new Set(["automated", "operator_soak", "operator_human"]);
const STATUS_ORDER = Object.freeze({ PASS: 0, NOT_RUN: 1, FAIL: 2, ERROR: 3 });
const SECRET_PATTERNS = [
  /(authorization\s*[:=]\s*)(?:bearer\s+)?([^\s,;]+)/gi,
  /((?:api[-_]?key|password|secret|token|credential)\s*[:=]\s*)([^\s,;]+)/gi,
  /(bearer\s+)[A-Za-z0-9._~+\/-]+=*/gi,
  /\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b/g,
];

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

export function redact(value) {
  let safe = String(value ?? "");
  for (const pattern of SECRET_PATTERNS) {
    safe = safe.replace(pattern, (_match, prefix = "") => `${prefix}[REDACTED]`);
  }
  return safe;
}

function requireString(errors, object, key, context) {
  if (typeof object?.[key] !== "string" || object[key].trim() === "") {
    errors.push(`${context}.${key} must be a non-empty string`);
  }
}

function requireStringArray(errors, object, key, context) {
  if (!Array.isArray(object?.[key]) || object[key].length === 0 || object[key].some((value) => typeof value !== "string" || value.trim() === "")) {
    errors.push(`${context}.${key} must be a non-empty string array`);
  }
}

export function validateCatalog(catalog, executorIds = new Set(Object.keys(EXECUTORS)), liveDriverIds = new Set(Object.keys(LIVE_DRIVERS))) {
  const errors = [];
  if (!catalog || typeof catalog !== "object" || Array.isArray(catalog)) return ["catalog must be an object"];
  if (catalog.catalog_version !== "1") errors.push("catalog_version must be '1'");
  if (catalog.evidence_schema !== EVIDENCE_SCHEMA) errors.push(`evidence_schema must be '${EVIDENCE_SCHEMA}'`);
  if (catalog.policy?.not_run_is_pass !== false) errors.push("policy.not_run_is_pass must be false");
  if (catalog.policy?.mandatory_scenario_pass_rate !== 1) errors.push("policy.mandatory_scenario_pass_rate must be 1");
  if (catalog.policy?.lane_verdicts_are_independent !== true) errors.push("policy.lane_verdicts_are_independent must be true");
  if (!Array.isArray(catalog.automatic_no_go) || catalog.automatic_no_go.length === 0) {
    errors.push("automatic_no_go must be a non-empty array");
  } else {
    for (const [index, rule] of catalog.automatic_no_go.entries()) {
      if (!/^CELLD\.NO_GO\.[A-Z0-9_.-]+$/.test(rule?.id ?? "") || typeof rule?.description !== "string" || rule.description.trim() === "") {
        errors.push(`automatic_no_go[${index}] requires a stable CELLD.NO_GO.* id and description`);
      }
    }
  }
  if (!Array.isArray(catalog.scenarios) || catalog.scenarios.length === 0) return [...errors, "scenarios must be a non-empty array"];

  const scenarioIds = new Set();
  const assertionIds = new Set();
  const coveredIssues = new Set();
  for (const [index, scenario] of catalog.scenarios.entries()) {
    const context = `scenarios[${index}]`;
    for (const key of ["id", "title", "lane", "description"]) requireString(errors, scenario, key, context);
    for (const key of ["issues", "personas", "tags", "given", "when", "then", "prerequisites", "artifacts", "cleanup"]) requireStringArray(errors, scenario, key, context);
    if (!/^UAT-CELLD-\d{3}$/.test(scenario.id ?? "")) errors.push(`${context}.id must match UAT-CELLD-NNN`);
    if (scenarioIds.has(scenario.id)) errors.push(`${context}.id is duplicated: ${scenario.id}`);
    scenarioIds.add(scenario.id);
    if (!LANES.has(scenario.lane)) errors.push(`${context}.lane is unsupported: ${scenario.lane}`);
    if (!TRIGGERS.has(scenario.trigger)) errors.push(`${context}.trigger is unsupported: ${scenario.trigger}`);
    for (const issue of scenario.issues ?? []) {
      if (!/^#(?:747|748|749|750|751|752|753|754)$/.test(issue)) errors.push(`${context}.issues contains unsupported issue ${issue}`);
      coveredIssues.add(issue);
    }
    if (!Array.isArray(scenario.assertions) || scenario.assertions.length === 0) {
      errors.push(`${context}.assertions must be a non-empty array`);
    } else {
      for (const [assertionIndex, assertion] of scenario.assertions.entries()) {
        const assertionContext = `${context}.assertions[${assertionIndex}]`;
        for (const key of ["id", "expected"]) requireString(errors, assertion, key, assertionContext);
        if (!/^CELLD\.[A-Z0-9_.-]+$/.test(assertion.id ?? "")) errors.push(`${assertionContext}.id must be stable CELLD.* notation`);
        if (assertionIds.has(assertion.id)) errors.push(`${assertionContext}.id is duplicated: ${assertion.id}`);
        assertionIds.add(assertion.id);
        if (typeof assertion.hard_gate !== "boolean") errors.push(`${assertionContext}.hard_gate must be boolean`);
      }
    }
    const execution = scenario.execution;
    if (!execution || !["deterministic", "live"].includes(execution.mode)) {
      errors.push(`${context}.execution.mode must be deterministic or live`);
      continue;
    }
    if (execution.mode === "live") {
      for (const key of Object.keys(execution)) if (!["mode", "supporting_executor_id", "supporting_covers_assertions", "live_drivers", "live_prerequisites"].includes(key)) errors.push(`${context}.execution.${key} is not allowed for live scenarios`);
      if (execution.executor_id !== undefined) errors.push(`${context}.execution live scenarios cannot name an executor`);
      requireStringArray(errors, execution, "live_prerequisites", `${context}.execution`);
      const hasSupportingExecutor = execution.supporting_executor_id !== undefined;
      const hasSupportingCoverage = execution.supporting_covers_assertions !== undefined;
      if (hasSupportingExecutor !== hasSupportingCoverage) {
        errors.push(`${context}.execution supporting_executor_id and supporting_covers_assertions must be provided together`);
      }
      if (hasSupportingExecutor) {
        requireString(errors, execution, "supporting_executor_id", `${context}.execution`);
        if (!executorIds.has(execution.supporting_executor_id)) {
          errors.push(`${context}.execution.supporting_executor_id is not allowlisted: ${execution.supporting_executor_id}`);
        }
        requireStringArray(errors, execution, "supporting_covers_assertions", `${context}.execution`);
        for (const assertionId of execution.supporting_covers_assertions ?? []) {
          if (!(scenario.assertions ?? []).some((assertion) => assertion.id === assertionId)) {
            errors.push(`${context}.execution supporting executor covers unknown assertion ${assertionId}`);
          }
        }
      }
      const liveDrivers = execution.live_drivers;
      const requiresDriver = scenario.trigger === "automated" && /^UAT-CELLD-0(?:0[3-9]|1[0-5])$/.test(scenario.id ?? "");
      if (requiresDriver && (!Array.isArray(liveDrivers) || liveDrivers.length === 0)) {
        errors.push(`${context}.execution.live_drivers must assign automated live assertions`);
      }
      const assigned = new Set(execution.supporting_covers_assertions ?? []);
      for (const [driverIndex, driver] of (Array.isArray(liveDrivers) ? liveDrivers : []).entries()) {
        const driverContext = `${context}.execution.live_drivers[${driverIndex}]`;
        for (const key of Object.keys(driver ?? {})) if (!["id", "covers_assertions"].includes(key)) errors.push(`${driverContext}.${key} is not allowed`);
        requireString(errors, driver, "id", driverContext);
        if (!liveDriverIds.has(driver?.id)) errors.push(`${driverContext}.id is not allowlisted: ${driver?.id}`);
        requireStringArray(errors, driver, "covers_assertions", driverContext);
        for (const assertionId of driver?.covers_assertions ?? []) {
          if (!(scenario.assertions ?? []).some((assertion) => assertion.id === assertionId)) errors.push(`${driverContext} covers unknown assertion ${assertionId}`);
          if (assigned.has(assertionId)) errors.push(`${driverContext} overlaps assertion ${assertionId}`);
          assigned.add(assertionId);
        }
      }
      if (requiresDriver) {
        for (const assertion of scenario.assertions ?? []) if (!assigned.has(assertion.id)) errors.push(`${context}.execution leaves assertion unassigned: ${assertion.id}`);
      }
    } else {
      for (const key of Object.keys(execution)) if (!["mode", "executor_id", "covers_assertions"].includes(key)) errors.push(`${context}.execution.${key} is not allowed for deterministic scenarios`);
      requireString(errors, execution, "executor_id", `${context}.execution`);
      if (!executorIds.has(execution.executor_id)) errors.push(`${context}.execution.executor_id is not allowlisted: ${execution.executor_id}`);
      requireStringArray(errors, execution, "covers_assertions", `${context}.execution`);
      for (const assertionId of execution.covers_assertions ?? []) {
        if (!(scenario.assertions ?? []).some((assertion) => assertion.id === assertionId)) errors.push(`${context}.execution covers unknown assertion ${assertionId}`);
      }
    }
  }
  for (let issue = 747; issue <= 754; issue += 1) {
    if (!coveredIssues.has(`#${issue}`)) errors.push(`catalog does not cover issue #${issue}`);
  }
  return errors;
}

export function selectScenarios(scenarios, { ids = [], tags = [], triggers = [] } = {}) {
  const requestedIds = new Set(ids);
  const requestedTags = new Set(tags);
  const requestedTriggers = new Set(triggers);
  const known = new Set(scenarios.map((scenario) => scenario.id));
  const unknown = [...requestedIds].filter((id) => !known.has(id));
  if (unknown.length > 0) throw new Error(`unknown scenario id(s): ${unknown.join(", ")}`);
  return scenarios.filter((scenario) => {
    const idMatch = requestedIds.size === 0 || requestedIds.has(scenario.id);
    const tagMatch = requestedTags.size === 0 || scenario.tags.some((tag) => requestedTags.has(tag));
    const triggerMatch = requestedTriggers.size === 0 || requestedTriggers.has(scenario.trigger);
    return idMatch && tagMatch && triggerMatch;
  });
}

export function runSafeExecutor(definition) {
  const started = new Date();
  const outcome = spawnSync(definition.program, definition.args, {
    cwd: REPO_ROOT,
    encoding: "utf8",
    env: { ...process.env, AGENTIC_CELLD_ENABLED: "false" },
    shell: false,
    timeout: definition.timeout_ms,
    maxBuffer: 8 * 1024 * 1024,
  });
  const ended = new Date();
  const stdout = redact(outcome.stdout ?? "");
  const stderr = redact(outcome.stderr ?? "");
  let kind = "pass";
  let reason = "deterministic command passed";
  if (outcome.error?.code === "ENOENT") {
    kind = "not_run";
    reason = `required tool not found: ${definition.program}`;
  } else if (outcome.error) {
    kind = "fail";
    reason = redact(outcome.error.message);
  } else if (outcome.status !== 0) {
    kind = "fail";
    reason = `deterministic command exited ${outcome.status}`;
  }
  return {
    kind,
    reason,
    cleanup_status: "not_required",
    command: {
      argv_redacted: [definition.program, ...definition.args].map(redact),
      shell: false,
      started_at: started.toISOString(),
      ended_at: ended.toISOString(),
      exit_code: outcome.status,
      signal: outcome.signal,
      stdout_sha256: sha256(stdout),
      stderr_sha256: sha256(stderr),
      stdout_preview: stdout.slice(0, 4096),
      stderr_preview: stderr.slice(0, 4096),
      stdout_tail: stdout.slice(-4096),
      stderr_tail: stderr.slice(-4096),
      redacted: true,
    },
  };
}

function scenarioStatus(assertions, cleanupStatus) {
  if (cleanupStatus === "failed") return "ERROR";
  return assertions.reduce((status, assertion) => STATUS_ORDER[assertion.status] > STATUS_ORDER[status] ? assertion.status : status, "PASS");
}

function readIdentity(path, key) {
  try { return JSON.parse(readFileSync(join(REPO_ROOT, path), "utf8"))[key] ?? null; } catch { return null; }
}

function gitCommit() {
  const result = spawnSync("git", ["rev-parse", "HEAD"], { cwd: REPO_ROOT, encoding: "utf8", shell: false });
  return result.status === 0 ? result.stdout.trim() : null;
}

export async function runUat(catalog, scenarios, options = {}) {
  const runId = options.runId ?? randomUUID();
  const execute = options.execute ?? runSafeExecutor;
  const executeLive = options.executeLive ?? ((definition, context) => runSafeLiveDriver(definition, context, redact));
  const evaluators = options.evaluators ?? LIVE_EVALUATORS;
  const liveProfile = options.liveProfile ?? null;
  const outputDir = options.outputDir ?? join(REPO_ROOT, "tests/celld/uat/results", runId);
  const cache = new Map();
  const records = [];
  for (const scenario of scenarios) {
    const started = new Date();
    let execution = null;
    let cleanupStatus = "not_required";
    const covered = new Set();
    const evaluatedAssertions = new Map();
    const liveDriverEvidence = [];
    const artifacts = [];
    const executorId = scenario.execution.mode === "deterministic"
      ? scenario.execution.executor_id
      : scenario.execution.supporting_executor_id;
    const coveredAssertions = scenario.execution.mode === "deterministic"
      ? scenario.execution.covers_assertions
      : scenario.execution.supporting_covers_assertions;
    if (executorId) {
      if (!cache.has(executorId)) cache.set(executorId, await execute(EXECUTORS[executorId], executorId));
      execution = cache.get(executorId);
      cleanupStatus = execution.cleanup_status ?? "not_required";
      for (const assertionId of coveredAssertions) covered.add(assertionId);
    }
    for (const assertion of scenario.assertions) {
      if (!covered.has(assertion.id)) continue;
      const status = execution.kind === "pass" ? "PASS" : execution.kind === "not_run" ? "NOT_RUN" : "FAIL";
      const safeReason = redact(execution.reason);
      evaluatedAssertions.set(assertion.id, { ...assertion, status, observed: safeReason, reason: safeReason, evidence_refs: ["command"] });
    }
    if (scenario.execution.mode === "live" && liveProfile && execution?.kind !== "fail" && execution?.kind !== "not_run") {
      mkdirSync(join(outputDir, "artifacts"), { recursive: true, mode: 0o700 });
      for (const driver of scenario.execution.live_drivers ?? []) {
        const profileEntry = liveProfile.drivers?.[driver.id];
        if (!profileEntry?.enabled) {
          liveDriverEvidence.push({ driver_id: driver.id, status: "NOT_RUN", reason: "LIVE_DRIVER_DISABLED", assigned_assertions: driver.covers_assertions });
          continue;
        }
        const definition = LIVE_DRIVERS[driver.id];
        const liveResult = await executeLive(definition, {
          driverId: driver.id,
          scenarioId: scenario.id,
          runId,
          assertionIds: new Set(driver.covers_assertions),
          profilePath: options.liveProfilePath,
          outputDir,
          expectedGit: liveProfile.expected_sandbox_git,
          expectedHostSha256: liveProfile.environment.host_sha256,
          expectedProfileId: liveProfile.profile_id,
          repoRoot: REPO_ROOT,
        });
        let evaluated = liveResult;
        if (liveResult.kind === "observation") {
          evaluated = evaluateLiveObservation(liveResult.observation, {
            driverId: driver.id,
            scenarioId: scenario.id,
            runId,
            assertionIds: new Set(driver.covers_assertions),
            outputDir,
            expectedGit: liveProfile.expected_sandbox_git,
            expectedHostSha256: liveProfile.environment.host_sha256,
            expectedProfileId: liveProfile.profile_id,
          }, evaluators);
          evaluated.command = liveResult.command;
        }
        if (evaluated.cleanup_status === "failed") cleanupStatus = "failed";
        for (const artifact of evaluated.artifacts ?? []) artifacts.push(artifact);
        if (evaluated.kind === "evaluated") {
          for (const result of evaluated.assertions) {
            const assertion = scenario.assertions.find((candidate) => candidate.id === result.id);
            evaluatedAssertions.set(result.id, { ...assertion, ...result });
          }
        } else {
          const status = evaluated.kind === "not_run" ? "NOT_RUN" : "ERROR";
          for (const assertionId of driver.covers_assertions) {
            const assertion = scenario.assertions.find((candidate) => candidate.id === assertionId);
            evaluatedAssertions.set(assertionId, { ...assertion, status, observed: null, reason: redact(evaluated.reason), evidence_refs: [] });
          }
        }
        liveDriverEvidence.push({
          driver_id: driver.id,
          status: evaluated.kind === "evaluated" ? scenarioStatus([...evaluatedAssertions.values()].filter((item) => driver.covers_assertions.includes(item.id)), evaluated.cleanup_status) : evaluated.kind === "not_run" ? "NOT_RUN" : "ERROR",
          reason: redact(evaluated.reason),
          assigned_assertions: driver.covers_assertions,
          command: evaluated.command ?? null,
          identities: evaluated.identities ?? {},
          metrics: evaluated.metrics ?? [],
          faults: evaluated.faults ?? [],
        });
      }
    }
    const assertions = scenario.assertions.map((assertion) => evaluatedAssertions.get(assertion.id) ?? {
      ...assertion,
      status: "NOT_RUN",
      observed: null,
      reason: scenario.execution.mode === "live" ? "live prerequisite evidence is unavailable" : "no deterministic executor covers this assertion",
      evidence_refs: [],
    });
    const ended = new Date();
    const status = scenarioStatus(assertions, cleanupStatus);
    records.push({
      evidence_schema: EVIDENCE_SCHEMA,
      run_id: runId,
      scenario_id: scenario.id,
      scenario_version: "1",
      title: scenario.title,
      issues: scenario.issues,
      lane: scenario.lane,
      trigger: scenario.trigger,
      personas: scenario.personas,
      tags: scenario.tags,
      status,
      reason_code: status === "NOT_RUN" ? (scenario.execution.mode === "live" ? "LIVE_PREREQUISITES_UNAVAILABLE" : "ASSERTION_EVIDENCE_UNAVAILABLE") : status === "ERROR" ? (cleanupStatus === "failed" ? "CLEANUP_FAILED" : "LIVE_EVIDENCE_ERROR") : status,
      started_at: started.toISOString(),
      ended_at: ended.toISOString(),
      duration_ms: ended - started,
      identities: {
        sandbox_git: gitCommit(),
        celld_version: "v0.2.1",
        celld_commit: "ae8fac053d79f971bfcb996054bb43eb2f9b05da",
        worker_digest: readIdentity("runtimes/celld/instance-cell/bundle.json", "digest"),
        adapter_version: "2026.8.3",
        protocol_version: "celld-internal-v1",
        runner_version: "1",
      },
      environment: { os: platform(), kernel: release(), architecture: arch(), hostname_sha256: sha256(hostname()) },
      prerequisites: scenario.execution.mode === "live" ? scenario.execution.live_prerequisites : scenario.prerequisites,
      assertions,
      supporting_evidence: scenario.execution.mode === "live" && executorId ? {
        executor_id: executorId,
        covered_assertions: [...covered],
        status: execution.kind === "pass" ? "PASS" : execution.kind === "not_run" ? "NOT_RUN" : "FAIL",
      } : null,
      live_driver_evidence: liveDriverEvidence,
      command: execution?.command ?? null,
      artifacts_required: scenario.artifacts,
      artifacts,
      cleanup: { status: cleanupStatus, requirements: scenario.cleanup },
    });
  }
  return { runId, records };
}

function groupVerdicts(records, field) {
  const grouped = {};
  for (const record of records) {
    const keys = Array.isArray(record[field]) ? record[field] : [record[field]];
    for (const key of keys) {
      grouped[key] ??= [];
      grouped[key].push(record.status);
    }
  }
  return Object.fromEntries(Object.entries(grouped).sort().map(([key, statuses]) => [key, statuses.reduce((current, status) => STATUS_ORDER[status] > STATUS_ORDER[current] ? status : current, "PASS")]));
}

export function determineExitCode({ records = [], validationErrors = [], evidenceError = false } = {}) {
  if (records.some((record) => record.cleanup?.status === "failed")) return 4;
  if (validationErrors.length > 0 || evidenceError || records.some((record) => record.status === "ERROR")) return 3;
  if (records.some((record) => record.status === "FAIL")) return 1;
  if (records.some((record) => record.status === "NOT_RUN")) return 2;
  return 0;
}

function xmlEscape(value) {
  return String(value).replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&apos;");
}

function renderJunit(records, runId) {
  const failures = records.filter((record) => record.status === "FAIL").length;
  const errors = records.filter((record) => record.status === "ERROR").length;
  const skipped = records.filter((record) => record.status === "NOT_RUN").length;
  const cases = records.map((record) => {
    let detail = "";
    if (record.status === "FAIL") detail = `<failure message="hard-gate assertion failed">${xmlEscape(JSON.stringify(record.assertions.filter((a) => a.status === "FAIL")))}</failure>`;
    if (record.status === "ERROR") detail = `<error message="runner or cleanup error"/>`;
    if (record.status === "NOT_RUN") detail = `<skipped message="live prerequisites unavailable"/>`;
    return `  <testcase classname="celld.uat.${xmlEscape(record.lane)}" name="${xmlEscape(record.scenario_id)}" time="${(record.duration_ms / 1000).toFixed(3)}">${detail}</testcase>`;
  }).join("\n");
  return `<?xml version="1.0" encoding="UTF-8"?>\n<testsuite name="celld-uat-${xmlEscape(runId)}" tests="${records.length}" failures="${failures}" errors="${errors}" skipped="${skipped}">\n${cases}\n</testsuite>\n`;
}

function renderReport(summary, records) {
  const rows = records.map((record) => `| ${record.scenario_id} | ${record.issues.join(", ")} | ${record.lane} | ${record.status} |`).join("\n");
  const gates = summary.hard_gate_failures.length === 0 ? "None." : summary.hard_gate_failures.map((gate) => `- ${gate.scenario_id}: ${gate.assertion_id}`).join("\n");
  const measured = Object.keys(summary.measured_lane_verdicts).length === 0 ? "None recorded." : `\`\`\`json\n${JSON.stringify(summary.measured_lane_verdicts, null, 2)}\n\`\`\``;
  return `# Celld UAT report\n\nRun: \`${summary.run_id}\`  \nGenerated: ${summary.generated_at}  \nSelection: \`${summary.selection.label}\`\n\nA \`NOT_RUN\` result is never treated as a pass. Lane and issue verdicts are independent. Credential-free supporting checks: ${summary.supporting_checks.counts.PASS} PASS, ${summary.supporting_checks.counts.FAIL} FAIL, ${summary.supporting_checks.counts.NOT_RUN} NOT_RUN across ${summary.supporting_checks.selected_scenarios} live scenarios.\n\n| Scenario | Issues | Lane | Status |\n|---|---|---|---|\n${rows}\n\n## Lane verdicts\n\n\`\`\`json\n${JSON.stringify(summary.lane_verdicts, null, 2)}\n\`\`\`\n\n## Measured product-lane verdicts\n\n${measured}\n\n## Hard-gate failures\n\n${gates}\n`;
}

export function verifyManifest(outputDir, manifest) {
  const errors = [];
  for (const line of manifest.trimEnd().split("\n")) {
    const match = line.match(/^([0-9a-f]{64})  ([A-Za-z0-9._/-]+)$/);
    if (!match) { errors.push(`invalid manifest line: ${line}`); continue; }
    const path = resolve(outputDir, match[2]);
    if (!path.startsWith(`${resolve(outputDir)}/`)) { errors.push(`manifest path escapes output directory: ${match[2]}`); continue; }
    try { if (sha256(readFileSync(path)) !== match[1]) errors.push(`manifest hash mismatch: ${match[2]}`); }
    catch { errors.push(`manifest file is missing: ${match[2]}`); }
  }
  return errors;
}

export function writeOutputs(outputDir, catalog, runId, records) {
  mkdirSync(outputDir, { recursive: true });
  const counts = Object.fromEntries(["PASS", "FAIL", "NOT_RUN", "ERROR"].map((status) => [status, records.filter((record) => record.status === status).length]));
  const supportingEvidence = records.map((record) => record.supporting_evidence).filter(Boolean);
  const supportingCounts = Object.fromEntries(["PASS", "FAIL", "NOT_RUN"].map((status) => [status, supportingEvidence.filter((evidence) => evidence.status === status).length]));
  const summary = {
    summary_schema: "agentic-sandbox.celld-uat-summary/v1",
    run_id: runId,
    generated_at: new Date().toISOString(),
    selected: records.length,
    selection: {
      label: Array.from({ length: 13 }, (_value, index) => `UAT-CELLD-${String(index + 3).padStart(3, "0")}`).every((id) => records.some((record) => record.scenario_id === id)) ? "complete-uat-003-015" : "partial-selection",
      selected_scenarios: records.map((record) => record.scenario_id),
    },
    counts,
    supporting_checks: { selected_scenarios: supportingEvidence.length, counts: supportingCounts },
    lane_verdicts: groupVerdicts(records, "lane"),
    measured_lane_verdicts: Object.assign({}, ...records.map((record) => record.measured_lane_verdicts ?? {})),
    issue_verdicts: groupVerdicts(records, "issues"),
    hard_gate_failures: records.flatMap((record) => record.assertions.filter((assertion) => assertion.hard_gate && assertion.status === "FAIL").map((assertion) => ({ scenario_id: record.scenario_id, assertion_id: assertion.id }))),
    automatic_no_go: catalog.automatic_no_go,
  };
  const files = {
    "summary.json": `${JSON.stringify(summary, null, 2)}\n`,
    "evidence.jsonl": `${records.map((record) => JSON.stringify(record)).join("\n")}\n`,
    "junit.xml": renderJunit(records, runId),
    "report.md": renderReport(summary, records),
  };
  for (const [name, contents] of Object.entries(files)) writeFileSync(join(outputDir, name), contents, { mode: 0o600 });
  const artifactNames = records.flatMap((record) => (record.artifacts ?? []).map((artifact) => artifact.path));
  if (new Set(artifactNames).size !== artifactNames.length) throw new Error("duplicate evidence artifact path");
  for (const name of artifactNames) {
    if (!/^artifacts\/[A-Za-z0-9._-]+$/.test(name)) throw new Error(`unsafe evidence artifact path: ${name}`);
  }
  const manifestNames = [...Object.keys(files), ...artifactNames].sort();
  const manifest = manifestNames.map((name) => `${sha256(readFileSync(join(outputDir, name)))}  ${name}`).join("\n") + "\n";
  writeFileSync(join(outputDir, "manifest.sha256"), manifest, { mode: 0o600 });
  const manifestErrors = verifyManifest(outputDir, manifest);
  if (manifestErrors.length > 0) throw new Error(`evidence manifest verification failed: ${manifestErrors.join("; ")}`);
  return { summary, files: [...Object.keys(files), ...artifactNames, "manifest.sha256"] };
}

function parseArgs(argv) {
  const options = { ids: [], tags: [], triggers: [], list: false, catalog: DEFAULT_CATALOG, outputDir: null, runId: null, liveProfilePath: null };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--id") options.ids.push(...(argv[++index] ?? "").split(",").filter(Boolean));
    else if (argument === "--tag") options.tags.push(...(argv[++index] ?? "").split(",").filter(Boolean));
    else if (argument === "--trigger") options.triggers.push(...(argv[++index] ?? "").split(",").filter(Boolean));
    else if (argument === "--catalog") options.catalog = resolve(argv[++index] ?? "");
    else if (argument === "--output-dir") options.outputDir = resolve(argv[++index] ?? "");
    else if (argument === "--run-id") options.runId = argv[++index] ?? null;
    else if (argument === "--live-profile") options.liveProfilePath = resolve(argv[++index] ?? "");
    else if (argument === "--list") options.list = true;
    else if (argument === "--help") options.help = true;
    else throw new Error(`unknown argument: ${argument}`);
  }
  return options;
}

function usage() {
  return `Usage: node scripts/run-celld-uat.mjs [--id ID[,ID]] [--tag TAG[,TAG]] [--trigger automated|operator_soak|operator_human] [--run-id ID] [--output-dir DIR] [--live-profile FILE] [--list]\nExit codes: 0 pass, 1 test failure, 2 NOT_RUN/prerequisite, 3 invalid evidence/catalog, 4 cleanup failure.`;
}

export async function main(argv = process.argv.slice(2)) {
  let options;
  try { options = parseArgs(argv); } catch (error) { console.error(redact(error.message)); console.error(usage()); return 3; }
  if (options.help) { console.log(usage()); return 0; }
  let catalog;
  try { catalog = JSON.parse(readFileSync(options.catalog, "utf8")); } catch (error) { console.error(`catalog read failed: ${redact(error.message)}`); return 3; }
  const validationErrors = validateCatalog(catalog);
  if (validationErrors.length > 0) { for (const error of validationErrors) console.error(`catalog: ${error}`); return 3; }
  let selected;
  try { selected = selectScenarios(catalog.scenarios, options); } catch (error) { console.error(redact(error.message)); return 3; }
  if (selected.length === 0) { console.error("no scenarios selected"); return 3; }
  if (options.list) { for (const scenario of selected) console.log(`${scenario.id}\t${scenario.lane}\t${scenario.trigger}\t${scenario.tags.join(",")}\t${scenario.title}`); return 0; }
  let liveProfile = null;
  if (options.liveProfilePath) {
    try { liveProfile = JSON.parse(readFileSync(options.liveProfilePath, "utf8")); }
    catch (error) { console.error(`live profile read failed: ${redact(error.message)}`); return 3; }
    const profileErrors = validateLiveProfile(liveProfile);
    if (profileErrors.length > 0) { for (const error of profileErrors) console.error(`live profile: ${error}`); return 3; }
  }
  const runId = options.runId ?? liveProfile?.run_id ?? `${new Date().toISOString().replaceAll(/[:.]/g, "-")}-${randomUUID().slice(0, 8)}`;
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(runId)) { console.error("run ID must use 1..128 letters, digits, dots, underscores, or hyphens"); return 3; }
  if (liveProfile && liveProfile.run_id !== runId) { console.error("live profile run_id does not match --run-id"); return 3; }
  if (liveProfile && liveProfile.expected_sandbox_git !== gitCommit()) { console.error("live profile expected_sandbox_git does not match HEAD"); return 3; }
  const outputDir = options.outputDir ?? join(REPO_ROOT, "tests/celld/uat/results", runId);
  try {
    const result = await runUat(catalog, selected, { runId, liveProfile, liveProfilePath: options.liveProfilePath, outputDir });
    const written = writeOutputs(outputDir, catalog, result.runId, result.records);
    console.log(`Celld UAT: ${JSON.stringify(written.summary.counts)}; evidence=${relative(REPO_ROOT, outputDir)}`);
    return determineExitCode({ records: result.records });
  } catch (error) {
    console.error(`runner failed: ${redact(error.stack ?? error.message)}`);
    return 3;
  }
}

if (process.argv[1] && resolve(process.argv[1]) === SCRIPT_PATH) process.exitCode = await main();
