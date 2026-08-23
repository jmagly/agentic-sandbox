import { createHash } from "node:crypto";

export const OBSERVABILITY_BOUNDARIES = Object.freeze([
  "celld",
  "management",
  "store_latency",
  "store_authorization",
  "store_condition",
  "provider",
  "divergence",
  "unknown_effect",
  "stale_generation",
  "below_reserve",
]);

export const OBSERVABILITY_SURFACES = Object.freeze([
  "cli",
  "api",
  "dashboard",
  "logs",
  "traces",
  "metrics",
  "alert_evaluator",
]);

export const OBSERVABILITY_REPAIR_SURFACES = Object.freeze(["cli", "api", "dashboard"]);
export const OBSERVABILITY_IDENTITY_FIELDS = Object.freeze([
  "fleet_id",
  "instance_id",
  "generation",
  "operation_id",
  "trace_id",
  "celld_version",
  "adapter_version",
  "node_id",
]);

const REQUIRED_ADAPTER_METHODS = Object.freeze([
  "captureBaseline",
  "persistIntent",
  "injectFault",
  "observeFault",
  "collectSurface",
  "collectRepairPlan",
  "observeAlertDetection",
  "healFault",
  "observeHeal",
  "observeAlertResolution",
  "scanRedaction",
  "verifyBaseline",
]);
const RUN_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const TRACE_ID = /^(?!0{32}$)[0-9a-f]{32}$/;

export class ObservabilityCleanupError extends Error {
  constructor(operationError, cleanupErrors) {
    super(`observability cleanup failed after ${operationError?.message ?? "campaign failure"}: ${cleanupErrors.join(";")}`);
    this.name = "ObservabilityCleanupError";
    this.operationError = operationError;
    this.cleanupErrors = [...cleanupErrors];
  }
}

function canonicalJson(value) {
  if (value === null || typeof value === "boolean" || typeof value === "string") return JSON.stringify(value);
  if (typeof value === "number" && Number.isFinite(value)) return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value && typeof value === "object") return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
  throw new Error("observability evidence contains a non-JSON value");
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function object(value, name) {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`${name} must be an object`);
  return value;
}

function rejectUnknown(value, allowed, name) {
  const unknown = Object.keys(value).filter((key) => !allowed.includes(key));
  if (unknown.length > 0) throw new Error(`${name} contains unknown fields: ${unknown.join(",")}`);
}

function exactObject(value, allowed, name) {
  const observed = object(value, name);
  rejectUnknown(observed, allowed, name);
  if (Object.keys(observed).length !== allowed.length || allowed.some((field) => !Object.hasOwn(observed, field))) throw new Error(`${name} must contain the exact field inventory`);
  return observed;
}

function string(value, name) {
  if (typeof value !== "string" || value.length === 0) throw new Error(`${name} must be a non-empty string`);
  return value;
}

function timestamp(value, name) {
  if (!Number.isSafeInteger(value) || value < 0) throw new Error(`${name} must be a non-negative integer timestamp`);
  return value;
}

function interval(value, name) {
  if (!Number.isSafeInteger(value) || value < 1) throw new Error(`${name} must be a positive integer interval`);
  return value;
}

function digest(value, name) {
  if (!SHA256.test(value ?? "")) throw new Error(`${name} must be a lowercase SHA-256 digest`);
  return value;
}

function validateAdapter(adapter) {
  for (const method of REQUIRED_ADAPTER_METHODS) if (typeof adapter?.[method] !== "function") throw new Error(`observability adapter.${method} is required`);
}

function validateIdentity(value, name) {
  const identity = exactObject(value, OBSERVABILITY_IDENTITY_FIELDS, name);
  for (const field of OBSERVABILITY_IDENTITY_FIELDS) {
    if (field === "generation") {
      if (!Number.isSafeInteger(identity[field]) || identity[field] < 1) throw new Error(`${name}.generation must be an integer >= 1`);
    } else string(identity[field], `${name}.${field}`);
  }
  if (!TRACE_ID.test(identity.trace_id)) throw new Error(`${name}.trace_id must be a nonzero lowercase W3C trace ID`);
  return Object.fromEntries(OBSERVABILITY_IDENTITY_FIELDS.map((field) => [field, identity[field]]));
}

function sameIdentity(left, right) {
  return OBSERVABILITY_IDENTITY_FIELDS.every((field) => left[field] === right[field]);
}

function alertWithinBound(boundary, injectedAt, detectedAt, evaluationInterval, retryInterval) {
  if (boundary === "divergence") return detectedAt - injectedAt <= 300_000;
  if (boundary === "unknown_effect") return detectedAt - injectedAt <= retryInterval * 2;
  if (boundary === "stale_generation") return detectedAt - injectedAt <= evaluationInterval;
  return true;
}

export async function executeObservabilityCampaign({ runId, adapter }) {
  if (!RUN_ID.test(runId ?? "")) throw new Error("observability runId is invalid");
  validateAdapter(adapter);
  const baseline = exactObject(await adapter.captureBaseline(), ["baseline_sha256"], "observability baseline");
  digest(baseline.baseline_sha256, "observability baseline digest");

  const timeline = [];
  const cases = [];
  const records = [];
  const alerts = [];
  const active = [];
  const operationIds = new Set();
  const traceIds = new Set();
  let intentSequence = 0;
  let mutationStarted = false;

  const persist = async (action, boundary) => {
    intentSequence += 1;
    const intent = {
      schema_version: "agentic-sandbox.celld-observability-intent/v1",
      run_id: runId,
      sequence: intentSequence,
      action,
      boundary,
    };
    const expectedDigest = sha256(canonicalJson(intent));
    const acknowledgment = exactObject(await adapter.persistIntent(structuredClone(intent)), ["intent_sha256", "persisted"], `observability intent ${intentSequence} acknowledgment`);
    if (acknowledgment.persisted !== true || digest(acknowledgment.intent_sha256, `observability intent ${intentSequence} digest`) !== expectedDigest) throw new Error(`observability ${action} intent was not durably acknowledged`);
    timeline.push({ sequence: timeline.length + 1, phase: "intent_persisted", intent, intent_sha256: expectedDigest });
  };

  try {
    for (const boundary of OBSERVABILITY_BOUNDARIES) {
      await persist("inject", boundary);
      await adapter.injectFault(boundary);
      mutationStarted = true;
      active.push(boundary);
      timeline.push({ sequence: timeline.length + 1, phase: "fault_injected", boundary });

      const fault = exactObject(
        await adapter.observeFault(boundary),
        ["boundary", "injection_applied", "injection_verified", "injected_at_ms", "identities"],
        `${boundary} fault observation`,
      );
      if (fault.boundary !== boundary || fault.injection_applied !== true || fault.injection_verified !== true) throw new Error(`${boundary} fault injection was not independently verified`);
      const identities = validateIdentity(fault.identities, `${boundary} fault identities`);
      const injectedAt = timestamp(fault.injected_at_ms, `${boundary} injected_at_ms`);
      if (operationIds.has(identities.operation_id) || traceIds.has(identities.trace_id)) throw new Error("observability fault operation and trace identities must be unique across boundaries");
      operationIds.add(identities.operation_id);
      traceIds.add(identities.trace_id);

      const surfaces = [];
      for (const surface of OBSERVABILITY_SURFACES) {
        const capture = exactObject(
          await adapter.collectSurface(boundary, surface),
          ["boundary", "surface", "classification", "identities"],
          `${boundary}/${surface} capture`,
        );
        const capturedIdentity = validateIdentity(capture.identities, `${boundary}/${surface} identities`);
        if (capture.boundary !== boundary || capture.surface !== surface || capture.classification !== boundary || !sameIdentity(identities, capturedIdentity)) throw new Error(`${boundary}/${surface} did not agree with the injected fault identity and classification`);
        surfaces.push({ surface, classification: capture.classification });
        records.push({ boundary, surface, identities: capturedIdentity });
      }

      const repairs = [];
      for (const surface of OBSERVABILITY_REPAIR_SURFACES) {
        const repair = exactObject(
          await adapter.collectRepairPlan(boundary, surface),
          ["boundary", "surface", "representation", "effect_claimed"],
          `${boundary}/${surface} repair presentation`,
        );
        if (repair.boundary !== boundary || repair.surface !== surface || repair.representation !== "plan" || repair.effect_claimed !== false) throw new Error(`${boundary}/${surface} repair presentation is not an honest plan`);
        repairs.push({ surface, representation: repair.representation, effect_claimed: repair.effect_claimed });
      }

      const detection = exactObject(
        await adapter.observeAlertDetection(boundary),
        ["boundary", "detected_at_ms", "evaluation_interval_ms", "retry_interval_ms"],
        `${boundary} alert detection`,
      );
      if (detection.boundary !== boundary) throw new Error(`${boundary} alert detection is misclassified`);
      const detectedAt = timestamp(detection.detected_at_ms, `${boundary} detected_at_ms`);
      const evaluationInterval = interval(detection.evaluation_interval_ms, `${boundary} evaluation_interval_ms`);
      const retryInterval = interval(detection.retry_interval_ms, `${boundary} retry_interval_ms`);
      if (detectedAt < injectedAt || !alertWithinBound(boundary, injectedAt, detectedAt, evaluationInterval, retryInterval)) throw new Error(`${boundary} alert detection missed its required timing bound`);

      await persist("heal", boundary);
      await adapter.healFault(boundary);
      const heal = exactObject(await adapter.observeHeal(boundary), ["boundary", "healed", "heal_verified", "healed_at_ms"], `${boundary} heal observation`);
      if (heal.boundary !== boundary || heal.healed !== true || heal.heal_verified !== true) throw new Error(`${boundary} heal was not independently verified`);
      const healedIndex = active.lastIndexOf(boundary);
      if (healedIndex >= 0) active.splice(healedIndex, 1);
      const healedAt = timestamp(heal.healed_at_ms, `${boundary} healed_at_ms`);
      if (healedAt < detectedAt) throw new Error(`${boundary} was healed before the alert was detected`);
      const resolution = exactObject(await adapter.observeAlertResolution(boundary), ["boundary", "resolved_at_ms"], `${boundary} alert resolution`);
      if (resolution.boundary !== boundary) throw new Error(`${boundary} alert resolution is misclassified`);
      const resolvedAt = timestamp(resolution.resolved_at_ms, `${boundary} resolved_at_ms`);
      if (resolvedAt < healedAt) throw new Error(`${boundary} alert resolved before the heal`);

      cases.push({ boundary, injection_applied: true, injection_verified: true, surfaces, repairs, healed: true, heal_verified: true });
      alerts.push({ boundary, injected_at_ms: injectedAt, detected_at_ms: detectedAt, healed_at_ms: healedAt, resolved_at_ms: resolvedAt, evaluation_interval_ms: evaluationInterval, retry_interval_ms: retryInterval });
      timeline.push({ sequence: timeline.length + 1, phase: "fault_healed_and_alert_resolved", boundary, injected_at_ms: injectedAt, detected_at_ms: detectedAt, healed_at_ms: healedAt, resolved_at_ms: resolvedAt });
    }

    const redaction = exactObject(await adapter.scanRedaction({ records: structuredClone(records), surfaces: [...OBSERVABILITY_SURFACES] }), ["surfaces_scanned", "artifacts_scanned", "secret_findings"], "observability redaction scan");
    if (!Array.isArray(redaction.surfaces_scanned) || redaction.surfaces_scanned.length !== OBSERVABILITY_SURFACES.length || new Set(redaction.surfaces_scanned).size !== OBSERVABILITY_SURFACES.length || OBSERVABILITY_SURFACES.some((surface) => !redaction.surfaces_scanned.includes(surface))) throw new Error("observability redaction scan did not cover the exact surface inventory");
    if (!Number.isSafeInteger(redaction.artifacts_scanned) || redaction.artifacts_scanned < records.length || redaction.secret_findings !== 0) throw new Error("observability redaction scan is incomplete or found restricted data");

    const restored = exactObject(await adapter.verifyBaseline(structuredClone(baseline)), ["baseline_sha256", "restored"], "observability restored baseline");
    if (digest(restored.baseline_sha256, "restored baseline digest") !== baseline.baseline_sha256 || restored.restored !== true) throw new Error("observability fleet baseline was not restored");
    return {
      mutation_started: mutationStarted,
      cases,
      records,
      alerts,
      redaction: { surfaces_scanned: [...redaction.surfaces_scanned], artifacts_scanned: redaction.artifacts_scanned, secret_findings: redaction.secret_findings },
      baseline: { baseline_sha256: baseline.baseline_sha256, restored: true },
      timeline,
      cleanup: { status: "passed", active_faults: 0, baseline_restored: true },
    };
  } catch (operationError) {
    const cleanupErrors = [];
    for (const boundary of [...active].reverse()) {
      try {
        await persist("emergency_heal", boundary);
        await adapter.healFault(boundary);
        const heal = exactObject(await adapter.observeHeal(boundary), ["boundary", "healed", "heal_verified", "healed_at_ms"], `${boundary} emergency heal observation`);
        if (heal.boundary !== boundary || heal.healed !== true || heal.heal_verified !== true) throw new Error(`${boundary} emergency heal was not verified`);
      } catch (cleanupError) {
        cleanupErrors.push(`${boundary}:${cleanupError.message}`);
      }
    }
    try {
      const restored = exactObject(await adapter.verifyBaseline(structuredClone(baseline)), ["baseline_sha256", "restored"], "observability emergency baseline verification");
      if (restored.baseline_sha256 !== baseline.baseline_sha256 || restored.restored !== true) throw new Error("fleet baseline did not match preflight");
    } catch (cleanupError) {
      cleanupErrors.push(`baseline:${cleanupError.message}`);
    }
    if (cleanupErrors.length > 0) throw new ObservabilityCleanupError(operationError, cleanupErrors);
    throw operationError;
  }
}
