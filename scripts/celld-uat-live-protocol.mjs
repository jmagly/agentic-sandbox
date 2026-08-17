import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, lstatSync, readFileSync, realpathSync } from "node:fs";
import { isAbsolute, join, resolve, sep } from "node:path";

export const LIVE_PROFILE_SCHEMA = "agentic-sandbox.celld-live-profile/v1";
export const LIVE_OBSERVATION_SCHEMA = "agentic-sandbox.celld-live-observation/v1";

export const LIVE_DRIVERS = Object.freeze({
  "celld-live-orchestration": Object.freeze({ program: process.execPath, args: ["scripts/celld-live-orchestration.mjs"], timeout_ms: 120 * 60 * 1000 }),
  "celld-live-worker": Object.freeze({ program: process.execPath, args: ["scripts/celld-live-worker.mjs"], timeout_ms: 45 * 60 * 1000 }),
  "celld-live-storage-topology": Object.freeze({ program: process.execPath, args: ["scripts/celld-live-storage-topology.mjs"], timeout_ms: 45 * 60 * 1000 }),
  "celld-live-network-auth": Object.freeze({ program: process.execPath, args: ["scripts/celld-live-network-auth.mjs"], timeout_ms: 75 * 60 * 1000 }),
  "celld-live-credential-provenance": Object.freeze({ program: process.execPath, args: ["scripts/celld-live-credential-provenance.mjs"], timeout_ms: 75 * 60 * 1000 }),
  "celld-live-rollout": Object.freeze({ program: process.execPath, args: ["scripts/celld-live-rollout.mjs"], timeout_ms: 75 * 60 * 1000 }),
  "celld-live-observability": Object.freeze({ program: process.execPath, args: ["scripts/celld-live-observability.mjs"], timeout_ms: 75 * 60 * 1000 }),
  "celld-live-recovery": Object.freeze({ program: process.execPath, args: ["scripts/celld-live-recovery.mjs"], timeout_ms: 75 * 60 * 1000 })
});

const RUN_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const DRIVER_ID = /^celld-live-[a-z0-9-]+$/;
const SCENARIO_ID = /^UAT-CELLD-\d{3}$/;
const ASSERTION_ID = /^CELLD\.[A-Z0-9_.-]+$/;
const REASON_CODE = /^[A-Z0-9][A-Z0-9_.-]+$/;
const ARTIFACT_PATH = /^artifacts\/[A-Za-z0-9._-]+$/;
const SHA256 = /^[0-9a-f]{64}$/;
const SECRET_LIKE = /(?:authorization|api[-_]?key|password|secret|token|credential)\s*[:=]\s*[^\s,;]+|bearer\s+[A-Za-z0-9._~+\/-]+/i;

function ownKeys(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? Object.keys(value) : [];
}

function rejectUnknown(errors, value, allowed, context) {
  for (const key of ownKeys(value)) if (!allowed.has(key)) errors.push(`${context}.${key} is not allowed`);
}

function nonEmptyString(value) {
  return typeof value === "string" && value.trim() !== "";
}

function validDate(value) {
  return nonEmptyString(value) && Number.isFinite(Date.parse(value));
}

export function validateLiveProfile(profile, registeredDriverIds = new Set(Object.keys(LIVE_DRIVERS))) {
  const errors = [];
  if (!profile || typeof profile !== "object" || Array.isArray(profile)) return ["profile must be an object"];
  rejectUnknown(errors, profile, new Set(["schema_version", "profile_id", "run_id", "expected_sandbox_git", "environment", "authorization", "drivers"]), "profile");
  if (profile.schema_version !== LIVE_PROFILE_SCHEMA) errors.push(`profile.schema_version must be ${LIVE_PROFILE_SCHEMA}`);
  if (!RUN_ID.test(profile.profile_id ?? "")) errors.push("profile.profile_id is invalid");
  if (!RUN_ID.test(profile.run_id ?? "")) errors.push("profile.run_id is invalid");
  if (!/^[0-9a-f]{40}$/.test(profile.expected_sandbox_git ?? "")) errors.push("profile.expected_sandbox_git must be a lowercase 40-character commit");
  rejectUnknown(errors, profile.environment, new Set(["kind", "single_host", "host_sha256"]), "profile.environment");
  if (!["titan-single-host", "disposable-local"].includes(profile.environment?.kind)) errors.push("profile.environment.kind is unsupported");
  if (typeof profile.environment?.single_host !== "boolean") errors.push("profile.environment.single_host must be boolean");
  if (profile.environment?.single_host !== true) errors.push("profile.environment.single_host must be true for the supported live profiles");
  if (!SHA256.test(profile.environment?.host_sha256 ?? "")) errors.push("profile.environment.host_sha256 must be sha256");
  rejectUnknown(errors, profile.authorization, new Set(["destructive_faults", "inventory_path", "exact_run_owner"]), "profile.authorization");
  if (typeof profile.authorization?.destructive_faults !== "boolean") errors.push("profile.authorization.destructive_faults must be boolean");
  if (!isAbsolute(profile.authorization?.inventory_path ?? "")) errors.push("profile.authorization.inventory_path must be absolute");
  if (profile.authorization?.destructive_faults && !nonEmptyString(profile.authorization?.exact_run_owner)) errors.push("profile.authorization.exact_run_owner is required for destructive faults");
  if (profile.authorization?.destructive_faults && profile.authorization?.exact_run_owner !== profile.run_id) errors.push("profile.authorization.exact_run_owner must match profile.run_id");
  if (!profile.drivers || typeof profile.drivers !== "object" || Array.isArray(profile.drivers)) {
    errors.push("profile.drivers must be an object");
  } else {
    for (const [id, driver] of Object.entries(profile.drivers)) {
      if (!registeredDriverIds.has(id)) errors.push(`profile.drivers contains unregistered driver ${id}`);
      rejectUnknown(errors, driver, new Set(["enabled", "config_path"]), `profile.drivers.${id}`);
      if (typeof driver?.enabled !== "boolean") errors.push(`profile.drivers.${id}.enabled must be boolean`);
      if (!isAbsolute(driver?.config_path ?? "")) errors.push(`profile.drivers.${id}.config_path must be absolute`);
    }
  }
  if (SECRET_LIKE.test(JSON.stringify(profile))) errors.push("profile contains secret-like inline data; use protected file references only");
  return errors;
}

export function validateLiveObservation(observation, context) {
  const errors = [];
  if (!observation || typeof observation !== "object" || Array.isArray(observation)) return ["observation must be an object"];
  rejectUnknown(errors, observation, new Set(["schema_version", "driver_id", "run_id", "scenario_id", "started_at", "ended_at", "mutation_started", "prerequisites", "assertions", "identities", "metrics", "faults", "artifacts", "cleanup"]), "observation");
  if (observation.schema_version !== LIVE_OBSERVATION_SCHEMA) errors.push(`observation.schema_version must be ${LIVE_OBSERVATION_SCHEMA}`);
  if (!DRIVER_ID.test(observation.driver_id ?? "") || observation.driver_id !== context.driverId) errors.push("observation.driver_id does not match the registered driver");
  if (!RUN_ID.test(observation.run_id ?? "") || observation.run_id !== context.runId) errors.push("observation.run_id does not match the runner");
  if (!SCENARIO_ID.test(observation.scenario_id ?? "") || observation.scenario_id !== context.scenarioId) errors.push("observation.scenario_id does not match the selected scenario");
  if (!validDate(observation.started_at) || !validDate(observation.ended_at) || Date.parse(observation.ended_at) < Date.parse(observation.started_at)) errors.push("observation timestamps are invalid or reversed");
  if (typeof observation.mutation_started !== "boolean") errors.push("observation.mutation_started must be boolean");
  const prerequisites = Array.isArray(observation.prerequisites) ? observation.prerequisites : [];
  if (!Array.isArray(observation.prerequisites)) errors.push("observation.prerequisites must be an array");
  const prerequisiteIds = new Set();
  for (const [index, prerequisite] of prerequisites.entries()) {
    rejectUnknown(errors, prerequisite, new Set(["id", "status", "reason_code"]), `observation.prerequisites[${index}]`);
    if (!REASON_CODE.test(prerequisite?.id ?? "")) errors.push(`observation.prerequisites[${index}].id is invalid`);
    if (!["available", "unavailable"].includes(prerequisite?.status)) errors.push(`observation.prerequisites[${index}].status is invalid`);
    if (!REASON_CODE.test(prerequisite?.reason_code ?? "")) errors.push(`observation.prerequisites[${index}].reason_code is invalid`);
    if (prerequisiteIds.has(prerequisite?.id)) errors.push(`observation prerequisite is duplicated: ${prerequisite?.id}`);
    prerequisiteIds.add(prerequisite?.id);
  }
  if (observation.mutation_started && prerequisites.some((item) => item.status === "unavailable")) errors.push("unavailable prerequisites are only valid before mutation starts");
  const assertions = Array.isArray(observation.assertions) ? observation.assertions : [];
  if (!Array.isArray(observation.assertions)) errors.push("observation.assertions must be an array");
  const ids = new Set();
  for (const [index, assertion] of assertions.entries()) {
    rejectUnknown(errors, assertion, new Set(["id", "measurements", "evidence_refs"]), `observation.assertions[${index}]`);
    if (!ASSERTION_ID.test(assertion?.id ?? "")) errors.push(`observation.assertions[${index}].id is invalid`);
    if (ids.has(assertion?.id)) errors.push(`observation assertion is duplicated: ${assertion?.id}`);
    ids.add(assertion?.id);
    if (!context.assertionIds.has(assertion?.id)) errors.push(`observation contains unassigned assertion ${assertion?.id}`);
    if (!assertion?.measurements || typeof assertion.measurements !== "object" || Array.isArray(assertion.measurements)) errors.push(`observation.assertions[${index}].measurements must be an object`);
    if (!Array.isArray(assertion?.evidence_refs) || assertion.evidence_refs.some((path) => !ARTIFACT_PATH.test(path))) errors.push(`observation.assertions[${index}].evidence_refs are invalid`);
  }
  for (const [key, expected] of [["identities", "object"], ["metrics", "array"], ["faults", "array"], ["artifacts", "array"]]) {
    if (expected === "array" ? !Array.isArray(observation[key]) : !observation[key] || typeof observation[key] !== "object" || Array.isArray(observation[key])) errors.push(`observation.${key} must be an ${expected}`);
  }
  rejectUnknown(errors, observation.identities, new Set(["profile_id", "sandbox_git", "environment_host_sha256", "driver_version"]), "observation.identities");
  if (!RUN_ID.test(observation.identities?.profile_id ?? "") || (context.expectedProfileId && observation.identities.profile_id !== context.expectedProfileId)) errors.push("observation.identities.profile_id does not match the live profile");
  if (!/^[0-9a-f]{40}$/.test(observation.identities?.sandbox_git ?? "") || (context.expectedGit && observation.identities.sandbox_git !== context.expectedGit)) errors.push("observation.identities.sandbox_git does not match the live profile");
  if (!SHA256.test(observation.identities?.environment_host_sha256 ?? "") || (context.expectedHostSha256 && observation.identities.environment_host_sha256 !== context.expectedHostSha256)) errors.push("observation.identities.environment_host_sha256 does not match the live profile");
  if (!nonEmptyString(observation.identities?.driver_version) || observation.identities.driver_version.length > 128) errors.push("observation.identities.driver_version is invalid");
  const artifactPaths = new Set();
  for (const [index, artifact] of (observation.artifacts ?? []).entries()) {
    rejectUnknown(errors, artifact, new Set(["path", "mime_type", "sha256", "bytes", "contains_restricted_data"]), `observation.artifacts[${index}]`);
    if (!ARTIFACT_PATH.test(artifact?.path ?? "")) errors.push(`observation.artifacts[${index}].path is unsafe`);
    if (artifactPaths.has(artifact?.path)) errors.push(`observation artifact is duplicated: ${artifact?.path}`);
    artifactPaths.add(artifact?.path);
    if (!nonEmptyString(artifact?.mime_type) || !SHA256.test(artifact?.sha256 ?? "") || !Number.isSafeInteger(artifact?.bytes) || artifact.bytes < 0 || artifact?.contains_restricted_data !== false) errors.push(`observation.artifacts[${index}] metadata is invalid`);
  }
  for (const assertion of assertions) for (const path of assertion.evidence_refs ?? []) if (!artifactPaths.has(path)) errors.push(`observation assertion ${assertion.id} references undeclared artifact ${path}`);
  rejectUnknown(errors, observation.cleanup, new Set(["status", "assertions"]), "observation.cleanup");
  if (!["not_required", "passed", "failed"].includes(observation.cleanup?.status)) errors.push("observation.cleanup.status is invalid");
  if (!Array.isArray(observation.cleanup?.assertions) || observation.cleanup.assertions.some((value) => !nonEmptyString(value))) errors.push("observation.cleanup.assertions must be an array of strings");
  if (SECRET_LIKE.test(JSON.stringify(observation))) errors.push("observation contains secret-like data");
  return errors;
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

export function verifyObservationArtifacts(observation, outputDir) {
  const errors = [];
  const root = resolve(outputDir);
  let realRoot;
  try { realRoot = realpathSync(root); }
  catch { return ["evidence root is missing or inaccessible"]; }
  for (const artifact of observation.artifacts ?? []) {
    const path = resolve(root, artifact.path);
    if (!path.startsWith(`${root}${sep}`) || !existsSync(path)) {
      errors.push(`${artifact.path} is missing or escapes the evidence root`);
      continue;
    }
    const metadata = lstatSync(path);
    if (!metadata.isFile() || metadata.isSymbolicLink()) { errors.push(`${artifact.path} must be a regular non-symlink file`); continue; }
    const realPath = realpathSync(path);
    if (!realPath.startsWith(`${realRoot}${sep}`)) { errors.push(`${artifact.path} resolves outside the evidence root`); continue; }
    const bytes = readFileSync(path);
    if (metadata.size !== artifact.bytes) errors.push(`${artifact.path} byte count does not match`);
    if (sha256(bytes) !== artifact.sha256) errors.push(`${artifact.path} sha256 does not match`);
  }
  return errors;
}

export function evaluateLiveObservation(observation, context, evaluators = {}) {
  const validationErrors = validateLiveObservation(observation, context);
  if (validationErrors.length === 0) validationErrors.push(...verifyObservationArtifacts(observation, context.outputDir));
  if (validationErrors.length > 0) return { kind: "error", reason: validationErrors.join("; "), cleanup_status: observation?.cleanup?.status ?? "not_required", assertions: [], artifacts: [] };
  const unavailable = observation.prerequisites.filter((item) => item.status === "unavailable");
  if (unavailable.length > 0) return { kind: "not_run", reason: unavailable.map((item) => item.reason_code).join(","), cleanup_status: observation.cleanup.status, assertions: [], artifacts: observation.artifacts };
  const byId = new Map(observation.assertions.map((assertion) => [assertion.id, assertion]));
  const results = [];
  for (const id of context.assertionIds) {
    const assertion = byId.get(id);
    if (!assertion) return { kind: "error", reason: `driver omitted assigned assertion ${id}`, cleanup_status: observation.cleanup.status, assertions: results, artifacts: observation.artifacts };
    const evaluator = evaluators[id];
    if (typeof evaluator !== "function") return { kind: "error", reason: `trusted evaluator is not registered for ${id}`, cleanup_status: observation.cleanup.status, assertions: results, artifacts: observation.artifacts };
    try {
      const evaluated = evaluator(assertion.measurements, observation);
      if (!evaluated || typeof evaluated.passed !== "boolean") throw new Error("evaluator must return a boolean passed field");
      results.push({ id, status: evaluated.passed ? "PASS" : "FAIL", observed: evaluated.observed ?? assertion.measurements, reason: evaluated.reason ?? (evaluated.passed ? "trusted evaluator passed" : "trusted evaluator failed"), evidence_refs: assertion.evidence_refs });
    } catch (error) {
      return { kind: "error", reason: `trusted evaluator failed for ${id}: ${error.message}`, cleanup_status: observation.cleanup.status, assertions: results, artifacts: observation.artifacts };
    }
  }
  return { kind: "evaluated", reason: "trusted evaluators completed", cleanup_status: observation.cleanup.status, assertions: results, artifacts: observation.artifacts, identities: observation.identities, metrics: observation.metrics, faults: observation.faults };
}

function safeEnvironment() {
  return Object.fromEntries(["PATH", "LANG", "LC_ALL", "TMPDIR", "HOME", "USER"].flatMap((key) => process.env[key] === undefined ? [] : [[key, process.env[key]]]));
}

export function runSafeLiveDriver(definition, context, redact = String) {
  const args = [...definition.args, "--scenario-id", context.scenarioId, "--run-id", context.runId, "--profile", context.profilePath, "--artifact-dir", join(context.outputDir, "artifacts")];
  const started = new Date();
  if (definition.program === process.execPath && definition.args[0]?.endsWith(".mjs") && !existsSync(resolve(context.repoRoot, definition.args[0]))) {
    const ended = new Date();
    return {
      kind: "not_run",
      reason: `registered live driver is not installed: ${context.driverId}`,
      cleanup_status: "not_required",
      command: { argv_redacted: [definition.program, ...args].map(redact), shell: false, started_at: started.toISOString(), ended_at: ended.toISOString(), exit_code: null, signal: null, stdout_sha256: sha256(""), stderr_sha256: sha256(""), stderr_tail: "", redacted: true },
    };
  }
  const outcome = spawnSync(definition.program, args, { cwd: context.repoRoot, encoding: "utf8", env: safeEnvironment(), shell: false, timeout: definition.timeout_ms, maxBuffer: 8 * 1024 * 1024 });
  const ended = new Date();
  const command = { argv_redacted: [definition.program, ...args].map(redact), shell: false, started_at: started.toISOString(), ended_at: ended.toISOString(), exit_code: outcome.status, signal: outcome.signal, stdout_sha256: sha256(redact(outcome.stdout ?? "")), stderr_sha256: sha256(redact(outcome.stderr ?? "")), stderr_tail: redact(outcome.stderr ?? "").slice(-4096), redacted: true };
  if (outcome.error?.code === "ENOENT") return { kind: "not_run", reason: `registered live driver is not installed: ${context.driverId}`, cleanup_status: "not_required", command };
  if (outcome.error || outcome.status !== 0) return { kind: "error", reason: redact(outcome.error?.message ?? `live driver exited ${outcome.status}`), cleanup_status: "failed", command };
  try { return { kind: "observation", observation: JSON.parse(outcome.stdout), command }; }
  catch (error) { return { kind: "error", reason: `live driver emitted invalid JSON: ${redact(error.message)}`, cleanup_status: "failed", command }; }
}
