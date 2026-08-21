#!/usr/bin/env node

import { createHash, randomUUID } from "node:crypto";
import { basename, dirname, join, relative, resolve } from "node:path";
import { chmodSync, copyFileSync, mkdirSync, readFileSync, statSync } from "node:fs";
import { arch, hostname, platform, release } from "node:os";
import { fileURLToPath } from "node:url";

import {
  DEFAULT_CATALOG,
  determineExitCode,
  redact,
  writeOutputs,
} from "./run-celld-uat.mjs";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
const WORKFLOWS = Object.freeze(["diagnose", "safe_reconcile", "rollback", "evidence_export"]);
const COST_COMPONENTS = Object.freeze(["celld", "object_store", "telemetry", "egress", "qemu", "docker", "host"]);
const STATUS_ORDER = Object.freeze({ PASS: 0, NOT_RUN: 1, FAIL: 2, ERROR: 3 });
const PINNED_CELLD_COMMIT = "ae8fac053d79f971bfcb996054bb43eb2f9b05da";
const PINNED_WORKER_DIGEST = "sha256:f2ead310c1d05497c38afd882cfbc57d2ad292846ec919e1c7e27936d64d5496";

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function finiteNumber(value, field, { minimum = -Infinity, maximum = Infinity } = {}) {
  if (typeof value !== "number" || !Number.isFinite(value) || value < minimum || value > maximum) {
    throw new Error(`${field} must be a finite number between ${minimum} and ${maximum}`);
  }
  return value;
}

function integer(value, field, { minimum = 0 } = {}) {
  if (!Number.isSafeInteger(value) || value < minimum) throw new Error(`${field} must be an integer >= ${minimum}`);
  return value;
}

function timestamp(value, field) {
  if (typeof value !== "string" || !Number.isFinite(Date.parse(value))) throw new Error(`${field} must be an ISO-8601 timestamp`);
  return new Date(value);
}

function validateArtifacts(artifacts) {
  if (!Array.isArray(artifacts) || artifacts.length === 0) throw new Error("artifacts must contain at least one evidence artifact");
  for (const [index, artifact] of artifacts.entries()) {
    if (typeof artifact?.path !== "string" || artifact.path.trim() === "") throw new Error(`artifacts[${index}].path is required`);
    if (!/^[a-f0-9]{64}$/.test(artifact?.sha256 ?? "")) throw new Error(`artifacts[${index}].sha256 must be a lowercase SHA-256 digest`);
    if (/^0{64}$/.test(artifact.sha256)) throw new Error(`artifacts[${index}].sha256 is still the template placeholder`);
    if (typeof artifact?.mime_type !== "string" || !/^[a-z0-9.+-]+\/[a-z0-9.+-]+$/i.test(artifact.mime_type)) throw new Error(`artifacts[${index}].mime_type is required`);
    if (typeof artifact?.redaction_profile !== "string" || artifact.redaction_profile.trim() === "") throw new Error(`artifacts[${index}].redaction_profile is required`);
    if (artifact?.contains_restricted_data !== false) throw new Error(`artifacts[${index}].contains_restricted_data must be false`);
  }
  return artifacts;
}

function nonEmptyString(value, field) {
  if (typeof value !== "string" || value.trim() === "") throw new Error(`${field} must be a non-empty string`);
  return value;
}

function validateIdentities(identities) {
  if (!identities || typeof identities !== "object" || Array.isArray(identities)) throw new Error("identities must be an object");
  if (!/^[a-f0-9]{40}$/.test(identities.sandbox_git ?? "")) throw new Error("identities.sandbox_git must be a 40-character Git commit");
  nonEmptyString(identities.aiwg_git, "identities.aiwg_git");
  if (identities.celld_version !== "v0.2.1" || identities.celld_commit !== PINNED_CELLD_COMMIT) throw new Error("identities must record the pinned Celld v0.2.1 commit");
  if (!/^[a-f0-9]{64}$/.test(identities.celld_artifact_sha256 ?? "")) throw new Error("identities.celld_artifact_sha256 must be a lowercase SHA-256 digest");
  if (identities.worker_digest !== PINNED_WORKER_DIGEST) throw new Error("identities.worker_digest does not match the qualified reference bundle");
  if (identities.adapter_version !== "2026.8.3" || identities.protocol_version !== "celld-internal-v1") throw new Error("identities adapter/protocol pair is not qualified");
  return identities;
}

function validateEnvironment(environment, minimumNodes) {
  if (!environment || typeof environment !== "object" || Array.isArray(environment)) throw new Error("environment must be an object");
  for (const field of ["os", "kernel", "architecture", "cpu", "memory", "substrate", "network_profile", "storage_provider", "trust_domain"]) {
    nonEmptyString(environment[field], `environment.${field}`);
  }
  if (!Array.isArray(environment.node_ids) || environment.node_ids.length < minimumNodes || environment.node_ids.some((node) => typeof node !== "string" || node.trim() === "")) {
    throw new Error(`environment.node_ids must contain at least ${minimumNodes} non-empty node IDs`);
  }
  if (new Set(environment.node_ids).size !== environment.node_ids.length) throw new Error("environment.node_ids must be unique");
  return environment;
}

function validateCleanup(cleanup, ended) {
  if (!cleanup || !new Set(["passed", "failed"]).has(cleanup.status)) throw new Error("cleanup.status must be passed or failed");
  const completed = timestamp(cleanup.completed_at, "cleanup.completed_at");
  if (completed < ended) throw new Error("cleanup.completed_at must not precede ended_at");
  if (!Array.isArray(cleanup.assertions) || cleanup.assertions.length === 0 || cleanup.assertions.some((item) => typeof item !== "string" || item.trim() === "")) {
    throw new Error("cleanup.assertions must be a non-empty string array");
  }
  return { status: cleanup.status, assertions: cleanup.assertions, completed_at: completed.toISOString() };
}

function verifyArtifactFiles(artifacts, inputDirectory) {
  for (const [index, artifact] of artifacts.entries()) {
    const path = resolve(inputDirectory, artifact.path);
    let metadata;
    try { metadata = statSync(path); } catch { throw new Error(`artifacts[${index}] does not exist: ${artifact.path}`); }
    if (!metadata.isFile()) throw new Error(`artifacts[${index}] is not a regular file: ${artifact.path}`);
    const bytes = readFileSync(path);
    const actual = sha256(bytes);
    if (actual !== artifact.sha256) throw new Error(`artifacts[${index}] SHA-256 mismatch: ${artifact.path}`);
    if (artifact.mime_type.startsWith("text/") || artifact.mime_type === "application/json") {
      const contents = bytes.toString("utf8");
      if (redact(contents) !== contents) throw new Error(`artifacts[${index}] contains secret-like material: ${artifact.path}`);
    }
  }
}

function stageArtifacts(artifacts, inputDirectory, outputDirectory) {
  const artifactDirectory = join(outputDirectory, "artifacts");
  mkdirSync(artifactDirectory, { recursive: true, mode: 0o700 });
  return artifacts.map((artifact, index) => {
    const source = resolve(inputDirectory, artifact.path);
    const name = `${String(index + 1).padStart(2, "0")}-${basename(artifact.path).replaceAll(/[^A-Za-z0-9._-]/g, "_")}`;
    const destination = join(artifactDirectory, name);
    copyFileSync(source, destination);
    chmodSync(destination, 0o600);
    return { ...artifact, path: `artifacts/${name}`, bytes: statSync(destination).size };
  });
}

function assertion(id, expected, pass, observed, evidenceRefs) {
  return {
    id,
    expected,
    hard_gate: true,
    status: pass ? "PASS" : "FAIL",
    observed,
    reason: pass ? "measured gate passed" : "measured hard gate failed",
    evidence_refs: evidenceRefs,
  };
}

function overallStatus(assertions) {
  return assertions.reduce(
    (current, item) => STATUS_ORDER[item.status] > STATUS_ORDER[current] ? item.status : current,
    "PASS",
  );
}

function assertNoSecretLikeMaterial(input) {
  const serialized = JSON.stringify(input);
  if (serialized.includes("replace-with") || serialized.includes("replace-node") || serialized.includes("0000000000000000000000000000000000000000")) {
    throw new Error("operator evidence still contains template placeholders");
  }
  if (redact(serialized) !== serialized) {
    throw new Error("operator evidence contains secret-like material; redact it before evaluation");
  }
}

export function evaluateSoak(input, scenario) {
  if (input?.schema_version !== "agentic-sandbox.celld-soak-input/v1") throw new Error("unsupported soak input schema_version");
  if (input.template_only !== false) throw new Error("soak input must set template_only to false after replacing every illustrative value with measured evidence");
  assertNoSecretLikeMaterial(input);
  const started = timestamp(input.started_at, "started_at");
  const ended = timestamp(input.ended_at, "ended_at");
  const durationMs = ended - started;
  if (durationMs < 24 * 60 * 60 * 1000) throw new Error("soak evidence must cover at least 24 actual wall-clock hours");
  const metrics = input.metrics ?? {};
  const acceptance = finiteNumber(metrics.acceptance_p99_ms, "metrics.acceptance_p99_ms", { minimum: 0 });
  const convergence = finiteNumber(metrics.convergence_p99_seconds, "metrics.convergence_p99_seconds", { minimum: 0 });
  const errorRate = finiteNumber(metrics.error_rate, "metrics.error_rate", { minimum: 0, maximum: 1 });
  const duplicates = integer(metrics.duplicate_effects, "metrics.duplicate_effects");
  const lost = integer(metrics.lost_acknowledged_intents, "metrics.lost_acknowledged_intents");
  const cpu = finiteNumber(metrics.cpu_headroom_percent, "metrics.cpu_headroom_percent", { minimum: 0, maximum: 100 });
  const memory = finiteNumber(metrics.memory_headroom_percent, "metrics.memory_headroom_percent", { minimum: 0, maximum: 100 });
  const nodeLosses = integer(metrics.planned_node_loss_recoveries, "metrics.planned_node_loss_recoveries", { minimum: 1 });
  const resident = integer(metrics.resident_cells_per_node, "metrics.resident_cells_per_node", { minimum: 1 });
  const rate = finiteNumber(metrics.lifecycle_operations_per_second, "metrics.lifecycle_operations_per_second", { minimum: 0 });
  const nodes = integer(metrics.node_count, "metrics.node_count", { minimum: 1 });

  const costs = input.monthly_cost_usd ?? {};
  for (const component of COST_COMPONENTS) finiteNumber(costs[component], `monthly_cost_usd.${component}`, { minimum: 0 });
  const laneVerdicts = { ...(input.lane_verdicts ?? {}) };
  for (const lane of ["orchestration", "worker", "fleet"]) {
    if (!new Set(["PASS", "NO_GO"]).has(laneVerdicts[lane])) throw new Error(`lane_verdicts.${lane} must be PASS or NO_GO`);
  }
  const artifacts = validateArtifacts(input.artifacts);
  const refs = artifacts.map((artifact) => artifact.path);
  const assertions = [
    assertion(
      "CELLD.016.SLO",
      scenario.assertions[0].expected,
      acceptance <= 250 && convergence <= 30 && errorRate < 0.01,
      { acceptance_p99_ms: acceptance, convergence_p99_seconds: convergence, error_rate: errorRate },
      refs,
    ),
    assertion(
      "CELLD.016.SAFETY",
      scenario.assertions[1].expected,
      duplicates === 0 && lost === 0 && nodeLosses >= 1 && durationMs >= 86_400_000,
      { duplicate_effects: duplicates, lost_acknowledged_intents: lost, planned_node_loss_recoveries: nodeLosses, duration_hours: durationMs / 3_600_000 },
      refs,
    ),
    assertion(
      "CELLD.016.CAPACITY_COST",
      scenario.assertions[2].expected,
      cpu >= 30 && memory >= 30 && nodes >= 3 && resident >= 1_000 && rate >= 50,
      { cpu_headroom_percent: cpu, memory_headroom_percent: memory, node_count: nodes, resident_cells_per_node: resident, lifecycle_operations_per_second: rate, monthly_cost_usd: costs, lane_verdicts: laneVerdicts },
      refs,
    ),
  ];
  if (assertions.some((item) => item.status === "FAIL")) {
    for (const lane of Object.keys(laneVerdicts)) laneVerdicts[lane] = "NO_GO";
  }
  return {
    started,
    ended,
    durationMs,
    assertions,
    artifacts,
    signoffs: [],
    metrics,
    laneVerdicts,
    identities: validateIdentities(input.identities),
    environment: validateEnvironment(input.environment, 3),
    cleanup: validateCleanup(input.cleanup, ended),
  };
}

export function evaluateHuman(input, scenario) {
  if (input?.schema_version !== "agentic-sandbox.celld-human-uat-input/v1") throw new Error("unsupported human UAT input schema_version");
  if (input.template_only !== false) throw new Error("human UAT input must set template_only to false after replacing every illustrative value with observed evidence");
  assertNoSecretLikeMaterial(input);
  const started = timestamp(input.started_at, "started_at");
  const ended = timestamp(input.ended_at, "ended_at");
  if (ended < started) throw new Error("ended_at must not precede started_at");
  const artifacts = validateArtifacts(input.artifacts);
  if (!Array.isArray(input.participants) || input.participants.length === 0) throw new Error("participants must be a non-empty array");

  const represented = new Set();
  const subjects = new Set();
  let workflowCount = 0;
  let firstPassCount = 0;
  let ratingTotal = 0;
  let criticalDefects = 0;
  let destructiveChecks = 0;
  let destructiveContextShown = 0;
  const signoffs = [];
  for (const [index, participant] of input.participants.entries()) {
    if (!scenario.personas.includes(participant?.persona)) throw new Error(`participants[${index}].persona is not an accepted role`);
    if (typeof participant?.subject !== "string" || participant.subject.trim() === "") throw new Error(`participants[${index}].subject is required`);
    if (subjects.has(participant.subject)) throw new Error(`participants[${index}].subject is duplicated`);
    subjects.add(participant.subject);
    represented.add(participant.persona);
    const rating = integer(participant.rating_1_to_5, `participants[${index}].rating_1_to_5`, { minimum: 1 });
    if (rating > 5) throw new Error(`participants[${index}].rating_1_to_5 must be <= 5`);
    ratingTotal += rating;
    const critical = integer(participant.critical_defects, `participants[${index}].critical_defects`);
    criticalDefects += critical;
    if (!Array.isArray(participant.workflows)) throw new Error(`participants[${index}].workflows must be an array`);
    const byName = new Map(participant.workflows.map((workflow) => [workflow?.name, workflow]));
    if (participant.workflows.length !== WORKFLOWS.length || byName.size !== WORKFLOWS.length || participant.workflows.some((workflow) => !WORKFLOWS.includes(workflow?.name))) {
      throw new Error(`participants[${index}].workflows must contain each required workflow exactly once`);
    }
    for (const name of WORKFLOWS) {
      const workflow = byName.get(name);
      if (!workflow || typeof workflow.first_attempt_pass !== "boolean") throw new Error(`participants[${index}] must record workflow ${name}`);
      workflowCount += 1;
      if (workflow.first_attempt_pass) firstPassCount += 1;
      if (name === "safe_reconcile" || name === "rollback") {
        if (typeof workflow.destructive_context_shown !== "boolean") throw new Error(`participants[${index}].${name} must record destructive_context_shown`);
        destructiveChecks += 1;
        if (workflow.destructive_context_shown) destructiveContextShown += 1;
      }
    }
    signoffs.push({
      persona: participant.persona,
      subject: participant.subject,
      rating_1_to_5: rating,
      decision: critical === 0 && participant.workflows.every((workflow) => workflow.first_attempt_pass) ? "accept" : "needs_work",
      timestamp: ended.toISOString(),
    });
  }
  const missingRoles = scenario.personas.filter((role) => !represented.has(role));
  const passRate = workflowCount === 0 ? 0 : firstPassCount / workflowCount;
  const meanRating = ratingTotal / input.participants.length;
  const refs = artifacts.map((artifact) => artifact.path);
  const assertions = [
    assertion(
      "CELLD.017.WORKFLOWS",
      scenario.assertions[0].expected,
      missingRoles.length === 0 && passRate >= 0.9,
      { represented_roles: [...represented].sort(), missing_roles: missingRoles, first_attempt_pass_rate: passRate, passed: firstPassCount, total: workflowCount },
      refs,
    ),
    assertion(
      "CELLD.017.SATISFACTION",
      scenario.assertions[1].expected,
      meanRating >= 4 && criticalDefects === 0,
      { mean_rating: meanRating, critical_defects: criticalDefects, participants: input.participants.length },
      refs,
    ),
    assertion(
      "CELLD.017.DESTRUCTIVE_CONTEXT",
      scenario.assertions[2].expected,
      destructiveChecks > 0 && destructiveChecks === destructiveContextShown,
      { context_shown: destructiveContextShown, destructive_workflows: destructiveChecks },
      refs,
    ),
  ];
  return {
    started,
    ended,
    durationMs: ended - started,
    assertions,
    artifacts,
    signoffs,
    identities: validateIdentities(input.identities),
    environment: validateEnvironment(input.environment, 1),
    cleanup: validateCleanup(input.cleanup, ended),
  };
}

export function buildRecord(scenario, runId, evaluated) {
  const status = evaluated.cleanup.status === "failed" ? "ERROR" : overallStatus(evaluated.assertions);
  return {
    evidence_schema: "agentic-sandbox.celld-uat-evidence/v1",
    run_id: runId,
    scenario_id: scenario.id,
    scenario_version: "1",
    title: scenario.title,
    issues: scenario.issues,
    lane: scenario.lane,
    personas: scenario.personas,
    tags: scenario.tags,
    status,
    reason_code: evaluated.cleanup.status === "failed" ? "CLEANUP_FAILED" : status,
    started_at: evaluated.started.toISOString(),
    ended_at: evaluated.ended.toISOString(),
    duration_ms: evaluated.durationMs,
    identities: { ...evaluated.identities, runner_version: "1" },
    environment: {
      ...evaluated.environment,
      evaluator: { os: platform(), kernel: release(), architecture: arch(), hostname_sha256: sha256(hostname()) },
    },
    prerequisites: scenario.execution.live_prerequisites,
    assertions: evaluated.assertions,
    command: null,
    artifacts: evaluated.artifacts,
    artifacts_required: scenario.artifacts,
    signoffs: evaluated.signoffs,
    measured_lane_verdicts: evaluated.laneVerdicts ?? null,
    cleanup: { ...evaluated.cleanup, requirements: scenario.cleanup },
  };
}

function parseArgs(argv) {
  const mode = argv[0];
  if (!new Set(["soak", "human"]).has(mode)) throw new Error("first argument must be soak or human");
  const options = { mode, input: null, outputDir: null, runId: null };
  for (let index = 1; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--input") options.input = resolve(argv[++index] ?? "");
    else if (argument === "--output-dir") options.outputDir = resolve(argv[++index] ?? "");
    else if (argument === "--run-id") options.runId = argv[++index] ?? null;
    else if (argument === "--help") options.help = true;
    else throw new Error(`unknown argument: ${argument}`);
  }
  if (!options.input && !options.help) throw new Error("--input is required");
  return options;
}

function usage() {
  return "Usage: node scripts/run-celld-operator-uat.mjs <soak|human> --input FILE [--run-id ID] [--output-dir DIR]";
}

export async function main(argv = process.argv.slice(2)) {
  let options;
  try { options = parseArgs(argv); } catch (error) { console.error(redact(error.message)); console.error(usage()); return 3; }
  if (options.help) { console.log(usage()); return 0; }
  const runId = options.runId ?? `${options.mode}-${new Date().toISOString().replaceAll(/[:.]/g, "-")}-${randomUUID().slice(0, 8)}`;
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(runId)) { console.error("run ID must use 1..128 letters, digits, dots, underscores, or hyphens"); return 3; }
  try {
    const catalog = JSON.parse(readFileSync(DEFAULT_CATALOG, "utf8"));
    const scenarioId = options.mode === "soak" ? "UAT-CELLD-016" : "UAT-CELLD-017";
    const scenario = catalog.scenarios.find((candidate) => candidate.id === scenarioId);
    const input = JSON.parse(readFileSync(options.input, "utf8"));
    const evaluated = options.mode === "soak" ? evaluateSoak(input, scenario) : evaluateHuman(input, scenario);
    verifyArtifactFiles(evaluated.artifacts, dirname(options.input));
    const outputDir = options.outputDir ?? join(REPO_ROOT, "tests/celld/uat/results", runId);
    evaluated.artifacts = stageArtifacts(evaluated.artifacts, dirname(options.input), outputDir);
    const record = buildRecord(scenario, runId, evaluated);
    writeOutputs(outputDir, catalog, runId, [record]);
    console.log(`Celld ${options.mode} UAT: ${record.status}; evidence=${relative(REPO_ROOT, outputDir)}`);
    return determineExitCode({ records: [record] });
  } catch (error) {
    console.error(`operator UAT failed: ${redact(error.message)}`);
    return 3;
  }
}

if (process.argv[1] && resolve(process.argv[1]) === SCRIPT_PATH) process.exitCode = await main();
