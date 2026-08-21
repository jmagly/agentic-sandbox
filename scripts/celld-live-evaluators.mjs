import { evaluateStorageEvidence } from "./celld-storage-qualifier.mjs";

function object(value, name = "measurements") {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`${name} must be an object`);
  for (const key of ["verdict", "passed", "hard_gate", "assertion_status"]) {
    if (Object.hasOwn(value, key)) throw new Error(`${name}.${key} is a forbidden self-declared verdict field`);
  }
  return value;
}

function integer(value, name, minimum = 0) {
  if (!Number.isSafeInteger(value) || value < minimum) throw new Error(`${name} must be an integer >= ${minimum}`);
  return value;
}

function number(value, name, minimum = 0) {
  if (!Number.isFinite(value) || value < minimum) throw new Error(`${name} must be a number >= ${minimum}`);
  return value;
}

function boolean(value, name) {
  if (typeof value !== "boolean") throw new Error(`${name} must be boolean`);
  return value;
}

function strings(value, name) {
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string" || item.length === 0)) throw new Error(`${name} must be a string array`);
  return value;
}

function string(value, name) {
  if (typeof value !== "string" || value.length === 0) throw new Error(`${name} must be a non-empty string`);
  return value;
}

function objects(value, name) {
  if (!Array.isArray(value)) throw new Error(`${name} must be an object array`);
  return value.map((item, index) => object(item, `${name}[${index}]`));
}

function exactStrings(value, expected, name) {
  const observed = strings(value, name);
  return observed.length === expected.length && expected.every((item) => observed.includes(item));
}

function evaluated(measurements, passed, reason) {
  return { passed, observed: measurements, reason };
}

function allZero(measurements, fields) {
  return fields.every((field) => integer(measurements[field], field) === 0);
}

const lifecycleActions = Object.freeze(["provision", "start", "stop", "destroy"]);
const substrates = Object.freeze(["qemu", "docker"]);
const crashPoints = Object.freeze(["before_dispatch", "during_dispatch", "after_dispatch"]);
const excludedCapabilities = Object.freeze(["process", "pty", "workspace", "filesystem", "raw_network", "vm", "container", "host_api"]);
const advertisedCapabilities = Object.freeze(["fetch", "rpc", "storage", "alarm", "websocket", "outbound_https", "wasm", "assets"]);
const resourceFamilies = Object.freeze(["cpu", "memory", "request_rate", "storage", "resident_cells", "outbound"]);
const denialClasses = Object.freeze(["forged_body", "forged_mac", "stale_timestamp", "nonce_replay", "wrong_key", "zero_generation", "wrong_generation", "public_or_cross_fleet"]);
const telemetryBoundaries = Object.freeze([
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
const telemetrySurfaces = Object.freeze(["cli", "api", "dashboard", "logs", "traces", "metrics", "alert_evaluator"]);
const operatorRepairSurfaces = Object.freeze(["cli", "api", "dashboard"]);
const correlationIdentityFields = Object.freeze(["fleet_id", "instance_id", "generation", "operation_id", "trace_id", "celld_version", "adapter_version", "node_id"]);

export const SAFE_LIVE_EVALUATORS = Object.freeze({
  "CELLD.003.ONE_EFFECT": (raw) => {
    const m = object(raw);
    const expectedOperations = lifecycleActions.length * substrates.length;
    const passed = integer(m.repeats_per_action, "repeats_per_action") === 10_000
      && exactStrings(m.actions, lifecycleActions, "actions")
      && exactStrings(m.substrates, substrates, "substrates")
      && integer(m.operation_ids, "operation_ids") === expectedOperations
      && integer(m.provider_effects, "provider_effects") === expectedOperations
      && integer(m.max_effects_per_operation, "max_effects_per_operation") === 1
      && integer(m.duplicate_effects, "duplicate_effects") === 0;
    return evaluated(m, passed, "exactly one provider effect per lifecycle operation identity");
  },
  "CELLD.003.COLLISION": (raw) => {
    const m = object(raw);
    const attempts = integer(m.collision_attempts, "collision_attempts", 1);
    const passed = attempts >= lifecycleActions.length * substrates.length
      && integer(m.rejected, "rejected") === attempts
      && integer(m.provider_effects_before, "provider_effects_before") === integer(m.provider_effects_after, "provider_effects_after");
    return evaluated(m, passed, "different-hash operation identity reuse is rejected before provider mutation");
  },
  "CELLD.004.NO_LOSS": (raw) => {
    const m = object(raw);
    const expected = 100 * crashPoints.length * substrates.length;
    const passed = integer(m.trials_per_crash_point, "trials_per_crash_point") === 100
      && exactStrings(m.crash_points, crashPoints, "crash_points")
      && exactStrings(m.substrates, substrates, "substrates")
      && integer(m.acknowledged, "acknowledged") === expected
      && integer(m.survived, "survived") === expected
      && integer(m.lost, "lost") === 0;
    return evaluated(m, passed, "every acknowledged intent survives every required crash point");
  },
  "CELLD.004.RECOVERY": (raw) => {
    const m = object(raw);
    const passed = integer(m.samples, "samples") >= 100 * crashPoints.length * substrates.length
      && number(m.p95_ms, "p95_ms") <= 30_000
      && integer(m.duplicate_effects, "duplicate_effects") === 0
      && boolean(m.components_healthy, "components_healthy")
      && boolean(m.inventory_restored, "inventory_restored");
    return evaluated(m, passed, "owner/restart recovery meets latency, uniqueness, and heal gates");
  },
  "CELLD.005.ORIGINAL_ID": (raw) => {
    const m = object(raw);
    const expected = 100 * lifecycleActions.length * substrates.length;
    const passed = integer(m.trials_per_action, "trials_per_action") === 100
      && exactStrings(m.actions, lifecycleActions, "actions")
      && exactStrings(m.substrates, substrates, "substrates")
      && integer(m.lookups, "lookups") === expected
      && integer(m.original_id_matches, "original_id_matches") === expected
      && integer(m.replacement_ids, "replacement_ids") === 0;
    return evaluated(m, passed, "all response-loss recovery reuses the original operation identity");
  },
  "CELLD.005.NO_SECOND_EFFECT": (raw) => {
    const m = object(raw);
    const passed = integer(m.trials, "trials", 1) >= 100 * lifecycleActions.length * substrates.length
      && integer(m.second_effects, "second_effects") === 0
      && number(m.p95_ms, "p95_ms") <= 30_000
      && boolean(m.proxy_healed, "proxy_healed");
    return evaluated(m, passed, "unknown outcomes converge without a second provider effect");
  },
  "CELLD.006.PRE_PROVIDER": (raw) => {
    const m = object(raw);
    const expected = 100 * 2 * substrates.length;
    const passed = integer(m.trials_per_action, "trials_per_action") === 100
      && exactStrings(m.actions, ["stop", "destroy"], "actions")
      && exactStrings(m.substrates, substrates, "substrates")
      && integer(m.attempts, "attempts") === expected
      && integer(m.rejected_before_provider, "rejected_before_provider") === expected
      && integer(m.provider_effects, "provider_effects") === 0;
    return evaluated(m, passed, "stale destructive commands are fenced before provider dispatch");
  },
  "CELLD.006.ACTIVE_SAFE": (raw) => {
    const m = object(raw);
    const passed = integer(m.stale_attempts, "stale_attempts", 1) >= 100 * 2 * substrates.length
      && integer(m.future_attempts, "future_attempts", 1) >= substrates.length
      && integer(m.active_generation_changes, "active_generation_changes") === 0
      && boolean(m.active_checksum_unchanged, "active_checksum_unchanged")
      && boolean(m.partition_healed, "partition_healed");
    return evaluated(m, passed, "stale and future actors cannot alter the active generation");
  },
  "CELLD.007.CLAIMS": (raw) => {
    const m = object(raw);
    const cases = integer(m.advertised_cases, "advertised_cases", advertisedCapabilities.length);
    const passed = exactStrings(m.capabilities, advertisedCapabilities, "capabilities")
      && integer(m.passed_cases, "passed_cases") === cases
      && integer(m.failed_cases, "failed_cases") === 0
      && integer(m.not_run_cases, "not_run_cases") === 0;
    return evaluated(m, passed, "every advertised live Worker capability case passes");
  },
  "CELLD.007.ROLLBACK": (raw) => {
    const m = object(raw);
    const passed = typeof m.previous_digest === "string" && /^sha256:[0-9a-f]{64}$/.test(m.previous_digest)
      && m.restored_digest === m.previous_digest
      && typeof m.state_sha256_before === "string" && m.state_sha256_after === m.state_sha256_before
      && boolean(m.approved_digest_active, "approved_digest_active");
    return evaluated(m, passed, "rollback restores the retained digest without durable-state loss");
  },
  "CELLD.008.LOUD_REJECTION": (raw) => {
    const m = object(raw);
    const expected = 100 * excludedCapabilities.length;
    const passed = exactStrings(m.capabilities, excludedCapabilities, "capabilities")
      && integer(m.attempts_per_capability, "attempts_per_capability") === 100
      && integer(m.attempts, "attempts") === expected
      && integer(m.typed_rejections, "typed_rejections") === expected
      && integer(m.silent_successes, "silent_successes") === 0;
    return evaluated(m, passed, "every excluded capability fails with a typed rejection");
  },
  "CELLD.008.NO_SIDE_EFFECT": (raw) => {
    const m = object(raw);
    const passed = integer(m.attempts, "attempts", 1) >= 100 * excludedCapabilities.length
      && allZero(m, ["processes_created", "files_created", "sockets_created", "containers_created", "vms_created"])
      && boolean(m.host_inventory_restored, "host_inventory_restored");
    return evaluated(m, passed, "excluded Worker APIs create no host-visible side effects");
  },
  "CELLD.009.CONTAINMENT": (raw) => {
    const m = object(raw);
    const passed = exactStrings(m.limit_families, resourceFamilies, "limit_families")
      && integer(m.families_enforced, "families_enforced") === resourceFamilies.length
      && number(m.max_enforcement_ms, "max_enforcement_ms") <= 5_000
      && integer(m.node_crashes, "node_crashes") === 0
      && boolean(m.offender_removed, "offender_removed");
    return evaluated(m, passed, "each advertised resource limit contains the offender within the bound");
  },
  "CELLD.009.NEIGHBOR": (raw) => {
    const m = object(raw);
    const attempts = integer(m.adjacent_attempts, "adjacent_attempts", 1);
    const successes = integer(m.adjacent_successes, "adjacent_successes");
    const passed = successes <= attempts && successes / attempts >= 0.99 && boolean(m.fleet_healthy, "fleet_healthy");
    return evaluated(m, passed, "adjacent workload success remains at least 99 percent");
  },
  "CELLD.010.STORAGE": (measurements) => {
    const result = evaluateStorageEvidence(measurements);
    if (result.status === "ERROR") throw new Error(`${result.reason_code}: ${result.errors.join("; ")}`);
    return { passed: result.status === "PASS", observed: { reason_code: result.reason_code, checks: result.checks }, reason: result.reason_code };
  },
  "CELLD.010.ISOLATION": (raw) => {
    const m = object(raw);
    const attempts = integer(m.forbidden_attempts, "forbidden_attempts", 1);
    const passed = exactStrings(m.classes, ["public_internal", "cross_fleet", "cross_bucket"], "classes")
      && integer(m.denied, "denied") === attempts
      && integer(m.succeeded, "succeeded") === 0
      && integer(m.provider_effects, "provider_effects") === 0
      && boolean(m.routes_healed, "routes_healed");
    return evaluated(m, passed, "all forbidden topology and scope routes are denied");
  },
  "CELLD.011.BUDGET": (raw) => {
    const m = object(raw);
    const passed = integer(m.nodes_expected, "nodes_expected") === 3
      && integer(m.max_unavailable_observed, "max_unavailable_observed") <= 1
      && integer(m.reserve_consumed, "reserve_consumed") === 0
      && boolean(m.membership_healthy, "membership_healthy");
    return evaluated(m, passed, "rolling replacement preserves the unavailable and reserve budgets");
  },
  "CELLD.011.SAFETY": (raw) => {
    const m = object(raw);
    const passed = allZero(m, ["lost_intents", "duplicate_effects", "stale_effects"])
      && number(m.reconcile_p95_ms, "reconcile_p95_ms") <= 30_000
      && boolean(m.approved_digests_restored, "approved_digests_restored");
    return evaluated(m, passed, "rollout, kill, and rollback preserve lifecycle safety");
  },
  "CELLD.011.REFUSAL": (raw) => {
    const m = object(raw);
    const passed = boolean(m.refused, "refused")
      && integer(m.node_mutations, "node_mutations") === 0
      && typeof m.inventory_sha256_before === "string"
      && m.inventory_sha256_after === m.inventory_sha256_before;
    return evaluated(m, passed, "an incompatible version pair is refused before node mutation");
  },
  "CELLD.012.DENIAL": (raw) => {
    const m = object(raw);
    const expected = 1_000 * denialClasses.length;
    const passed = exactStrings(m.classes, denialClasses, "classes")
      && integer(m.attempts_per_class, "attempts_per_class") === 1_000
      && integer(m.attempts, "attempts") === expected
      && integer(m.denied, "denied") === expected
      && integer(m.provider_effects, "provider_effects") === 0;
    return evaluated(m, passed, "every signed negative class is denied without an effect");
  },
  "CELLD.012.VALID": (raw) => {
    const m = object(raw);
    const passed = integer(m.attempts, "attempts") === 1
      && integer(m.successes, "successes") === 1
      && boolean(m.correlated, "correlated")
      && boolean(m.signature_value_absent, "signature_value_absent")
      && boolean(m.identity_removed, "identity_removed");
    return evaluated(m, passed, "the valid private identity succeeds once with correlation and no disclosure");
  },
  "CELLD.014.CLASSIFICATION": (raw) => {
    const m = object(raw);
    const cases = objects(m.cases, "cases");
    const boundaries = cases.map((item, index) => string(item.boundary, `cases[${index}].boundary`));
    const passed = cases.length === telemetryBoundaries.length
      && exactStrings(boundaries, telemetryBoundaries, "cases.boundaries")
      && new Set(boundaries).size === telemetryBoundaries.length
      && cases.every((item, caseIndex) => {
        const surfaces = objects(item.surfaces, `cases[${caseIndex}].surfaces`);
        const surfaceNames = surfaces.map((surface, surfaceIndex) => string(surface.surface, `cases[${caseIndex}].surfaces[${surfaceIndex}].surface`));
        const repairs = objects(item.repairs, `cases[${caseIndex}].repairs`);
        const repairNames = repairs.map((repair, repairIndex) => string(repair.surface, `cases[${caseIndex}].repairs[${repairIndex}].surface`));
        return boolean(item.injection_applied, `cases[${caseIndex}].injection_applied`)
          && boolean(item.injection_verified, `cases[${caseIndex}].injection_verified`)
          && boolean(item.healed, `cases[${caseIndex}].healed`)
          && boolean(item.heal_verified, `cases[${caseIndex}].heal_verified`)
          && surfaces.length === telemetrySurfaces.length
          && exactStrings(surfaceNames, telemetrySurfaces, `cases[${caseIndex}].surface_names`)
          && new Set(surfaceNames).size === telemetrySurfaces.length
          && surfaces.every((surface) => string(surface.classification, "surface.classification") === item.boundary)
          && repairs.length === operatorRepairSurfaces.length
          && exactStrings(repairNames, operatorRepairSurfaces, `cases[${caseIndex}].repair_names`)
          && new Set(repairNames).size === operatorRepairSurfaces.length
          && repairs.every((repair) => string(repair.representation, "repair.representation") === "plan" && boolean(repair.effect_claimed, "repair.effect_claimed") === false);
      });
    return evaluated(m, passed, "every injected boundary is consistently classified across operator surfaces");
  },
  "CELLD.014.CORRELATION": (raw) => {
    const m = object(raw);
    const records = objects(m.records, "records");
    const expectedRecords = telemetryBoundaries.length * telemetrySurfaces.length;
    const identityBundles = new Map();
    let identitiesAgree = true;
    const pairs = records.map((record, index) => {
      const boundary = string(record.boundary, `records[${index}].boundary`);
      const surface = string(record.surface, `records[${index}].surface`);
      const identities = object(record.identities, `records[${index}].identities`);
      for (const field of correlationIdentityFields) {
        if (field === "generation") integer(identities[field], `records[${index}].identities.${field}`, 1);
        else string(identities[field], `records[${index}].identities.${field}`);
      }
      if (!/^[0-9a-f]{32}$/.test(identities.trace_id) || /^0{32}$/.test(identities.trace_id)) throw new Error(`records[${index}].identities.trace_id must be a nonzero lowercase W3C trace ID`);
      const identityBundle = JSON.stringify(correlationIdentityFields.map((field) => identities[field]));
      if (identityBundles.has(boundary) && identityBundles.get(boundary) !== identityBundle) identitiesAgree = false;
      else identityBundles.set(boundary, identityBundle);
      return `${boundary}\u0000${surface}`;
    });
    const redaction = object(m.redaction, "redaction");
    const passed = records.length === expectedRecords
      && new Set(pairs).size === expectedRecords
      && identitiesAgree
      && records.every((record) => telemetryBoundaries.includes(record.boundary) && telemetrySurfaces.includes(record.surface))
      && telemetryBoundaries.every((boundary) => telemetrySurfaces.every((surface) => pairs.includes(`${boundary}\u0000${surface}`)))
      && exactStrings(redaction.surfaces_scanned, telemetrySurfaces, "redaction.surfaces_scanned")
      && integer(redaction.artifacts_scanned, "redaction.artifacts_scanned", expectedRecords) >= expectedRecords
      && integer(redaction.secret_findings, "redaction.secret_findings") === 0
      && boolean(m.evidence_exported, "evidence_exported")
      && boolean(m.fleet_baseline_restored, "fleet_baseline_restored");
    return evaluated(m, passed, "all applicable records carry required correlation without secrets");
  },
  "CELLD.014.ALERTS": (raw) => {
    const m = object(raw);
    const alerts = objects(m.alerts, "alerts");
    const boundaries = alerts.map((alert, index) => string(alert.boundary, `alerts[${index}].boundary`));
    const passed = alerts.length === telemetryBoundaries.length
      && exactStrings(boundaries, telemetryBoundaries, "alerts.boundaries")
      && new Set(boundaries).size === telemetryBoundaries.length
      && alerts.every((alert, index) => {
        const injectedAt = number(alert.injected_at_ms, `alerts[${index}].injected_at_ms`);
        const detectedAt = number(alert.detected_at_ms, `alerts[${index}].detected_at_ms`);
        const healedAt = number(alert.healed_at_ms, `alerts[${index}].healed_at_ms`);
        const resolvedAt = number(alert.resolved_at_ms, `alerts[${index}].resolved_at_ms`);
        const evaluationInterval = number(alert.evaluation_interval_ms, `alerts[${index}].evaluation_interval_ms`, 1);
        const retryInterval = number(alert.retry_interval_ms, `alerts[${index}].retry_interval_ms`, 1);
        const detectedWithinBound = alert.boundary === "divergence"
          ? detectedAt - injectedAt <= 300_000
          : alert.boundary === "unknown_effect"
            ? detectedAt - injectedAt <= retryInterval * 2
            : alert.boundary === "stale_generation"
              ? detectedAt - injectedAt <= evaluationInterval
              : true;
        return detectedAt >= injectedAt
          && detectedAt <= healedAt
          && resolvedAt >= healedAt
          && detectedWithinBound;
      });
    return evaluated(m, passed, "alert detection and resolution meet the documented bounds");
  },
  "CELLD.015.OBJECTIVES": (raw) => {
    const m = object(raw);
    const passed = integer(m.restore_executions, "restore_executions") === 2
      && number(m.rpo_seconds, "rpo_seconds") <= 300
      && number(m.rto_seconds, "rto_seconds") <= 1_800
      && boolean(m.latest_acknowledged_state_present, "latest_acknowledged_state_present")
      && boolean(m.tombstones_present, "tombstones_present");
    return evaluated(m, passed, "both restore exercises meet RPO/RTO and state completeness");
  },
  "CELLD.015.IDEMPOTENT": (raw) => {
    const m = object(raw);
    const passed = integer(m.runbook_executions, "runbook_executions") === 2
      && integer(m.first_execution_effects, "first_execution_effects", 1) >= 1
      && integer(m.second_execution_additional_effects, "second_execution_additional_effects") === 0
      && boolean(m.restore_state_unchanged, "restore_state_unchanged");
    return evaluated(m, passed, "the second recovery runbook execution creates no additional effects");
  },
  "CELLD.015.EVIDENCE": (raw) => {
    const m = object(raw);
    const artifacts = integer(m.external_artifacts, "external_artifacts", 1);
    const passed = integer(m.readable_after_fleet_loss, "readable_after_fleet_loss") === artifacts
      && integer(m.hashes_verified, "hashes_verified") === artifacts
      && integer(m.corruption_cases_detected, "corruption_cases_detected", 1) >= 1
      && boolean(m.retention_confirmed, "retention_confirmed");
    return evaluated(m, passed, "recovery evidence survives fleet loss and detects corruption");
  },
});

export const WITHHELD_LIVE_EVALUATOR_IDS = Object.freeze([
  "CELLD.013.NO_LEAK",
  "CELLD.013.SCOPE",
  "CELLD.013.PROVENANCE",
]);
