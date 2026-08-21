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

function sha256Digest(value, name) {
  const observed = string(value, name);
  if (!/^[0-9a-f]{64}$/.test(observed)) throw new Error(`${name} must be a lowercase SHA-256 digest`);
  return observed;
}

function objects(value, name) {
  if (!Array.isArray(value)) throw new Error(`${name} must be an object array`);
  return value.map((item, index) => object(item, `${name}[${index}]`));
}

function exactStrings(value, expected, name) {
  const observed = strings(value, name);
  return observed.length === expected.length && expected.every((item) => observed.includes(item));
}

function exactCaseMatrix(value, name, dimensions) {
  const cases = objects(value, name);
  const expectedKeys = new Set([""]);
  for (const [field, expected] of Object.entries(dimensions)) {
    const prefixes = [...expectedKeys];
    expectedKeys.clear();
    for (const prefix of prefixes) for (const item of expected) expectedKeys.add(`${prefix}\u0000${field}=${item}`);
  }
  const observedKeys = new Set();
  let validDimensions = true;
  for (const [index, entry] of cases.entries()) {
    let key = "";
    for (const [field, expected] of Object.entries(dimensions)) {
      const valueForField = typeof expected[0] === "number"
        ? integer(entry[field], `${name}[${index}].${field}`)
        : string(entry[field], `${name}[${index}].${field}`);
      if (!expected.includes(valueForField)) validDimensions = false;
      key += `\u0000${field}=${valueForField}`;
    }
    if (observedKeys.has(key)) validDimensions = false;
    observedKeys.add(key);
  }
  const complete = validDimensions
    && observedKeys.size === expectedKeys.size
    && [...expectedKeys].every((key) => observedKeys.has(key));
  return { cases, complete };
}

function percentile(values, fraction, name) {
  if (!Array.isArray(values) || values.length === 0) throw new Error(`${name} must contain samples`);
  const sorted = values.map((value, index) => number(value, `${name}[${index}]`)).sort((a, b) => a - b);
  return sorted[Math.ceil(sorted.length * fraction) - 1];
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
const recoveryRunbooks = Object.freeze(["node_loss", "full_restart", "authorization_loss", "snapshot_restore", "credential_rotation"]);
const recoveryEvidenceKinds = Object.freeze(["snapshot_identity", "restore_timeline", "generation_comparison", "evidence_manifest"]);

export const SAFE_LIVE_EVALUATORS = Object.freeze({
  "CELLD.003.ONE_EFFECT": (raw) => {
    const m = object(raw);
    const matrix = exactCaseMatrix(m.cases, "cases", { substrate: substrates, action: lifecycleActions });
    const operationIds = new Set();
    const managementIds = new Set();
    const passed = matrix.complete && matrix.cases.every((entry, index) => {
      operationIds.add(sha256Digest(entry.operation_id_sha256, `cases[${index}].operation_id_sha256`));
      managementIds.add(sha256Digest(entry.management_operation_id_sha256, `cases[${index}].management_operation_id_sha256`));
      return integer(entry.replay_count, `cases[${index}].replay_count`) === 10_000
        && integer(entry.replay_http_200, `cases[${index}].replay_http_200`) === 10_000
        && integer(entry.replay_management_operation_matches, `cases[${index}].replay_management_operation_matches`) === 10_000
        && integer(entry.replay_terminal_status_matches, `cases[${index}].replay_terminal_status_matches`) === 10_000
        && integer(entry.effect_records, `cases[${index}].effect_records`) === 1
        && integer(entry.provider_effect_count, `cases[${index}].provider_effect_count`) === 1;
    }) && operationIds.size === matrix.cases.length && managementIds.size === matrix.cases.length;
    return evaluated(m, passed, "exactly one provider effect per lifecycle operation identity");
  },
  "CELLD.003.COLLISION": (raw) => {
    const m = object(raw);
    const matrix = exactCaseMatrix(m.cases, "cases", { substrate: substrates, action: lifecycleActions });
    const operationIds = new Set();
    const passed = matrix.complete && matrix.cases.every((entry, index) => {
      operationIds.add(sha256Digest(entry.operation_id_sha256, `cases[${index}].operation_id_sha256`));
      return integer(entry.response_status, `cases[${index}].response_status`) === 409
        && string(entry.response_code, `cases[${index}].response_code`) === "celld.operation_collision"
        && integer(entry.effect_records_before, `cases[${index}].effect_records_before`) === 1
        && integer(entry.effect_records_after, `cases[${index}].effect_records_after`) === 1
        && integer(entry.provider_effects_before, `cases[${index}].provider_effects_before`) === 1
        && integer(entry.provider_effects_after, `cases[${index}].provider_effects_after`) === 1;
    }) && operationIds.size === matrix.cases.length;
    return evaluated(m, passed, "different-hash operation identity reuse is rejected before provider mutation");
  },
  "CELLD.004.NO_LOSS": (raw) => {
    const m = object(raw);
    const matrix = exactCaseMatrix(m.cases, "cases", { substrate: substrates, crash_point: crashPoints, trial: Array.from({ length: 100 }, (_, index) => index + 1) });
    const operationIds = new Set();
    const passed = matrix.complete && matrix.cases.every((entry, index) => {
      operationIds.add(sha256Digest(entry.operation_id_sha256, `cases[${index}].operation_id_sha256`));
      return boolean(entry.acknowledged, `cases[${index}].acknowledged`)
        && string(entry.terminal_status, `cases[${index}].terminal_status`) === "succeeded";
    }) && operationIds.size === matrix.cases.length;
    return evaluated(m, passed, "every acknowledged intent survives every required crash point");
  },
  "CELLD.004.RECOVERY": (raw) => {
    const m = object(raw);
    const matrix = exactCaseMatrix(m.cases, "cases", { substrate: substrates, crash_point: crashPoints, trial: Array.from({ length: 100 }, (_, index) => index + 1) });
    const providerMatrix = exactCaseMatrix(m.provider_cases, "provider_cases", { substrate: substrates });
    const p95 = percentile(matrix.cases.map((entry) => entry.recovery_ms), 0.95, "recovery_ms");
    const passed = matrix.complete
      && providerMatrix.complete
      && p95 <= 30_000
      && matrix.cases.every((entry, index) => integer(entry.effect_records, `cases[${index}].effect_records`) === 1)
      && providerMatrix.cases.every((entry, index) => sha256Digest(entry.provider_checksum_before, `provider_cases[${index}].provider_checksum_before`) === sha256Digest(entry.provider_checksum_after, `provider_cases[${index}].provider_checksum_after`))
      && boolean(m.components_healthy, "components_healthy")
      && boolean(m.inventory_restored, "inventory_restored");
    return evaluated({ ...m, derived_p95_ms: p95 }, passed, "owner/restart recovery meets latency, uniqueness, and heal gates");
  },
  "CELLD.005.ORIGINAL_ID": (raw) => {
    const m = object(raw);
    const matrix = exactCaseMatrix(m.cases, "cases", { substrate: substrates, action: lifecycleActions, trial: Array.from({ length: 100 }, (_, index) => index + 1) });
    const operationIds = new Set();
    const passed = matrix.complete && matrix.cases.every((entry, index) => {
      operationIds.add(sha256Digest(entry.operation_id_sha256, `cases[${index}].operation_id_sha256`));
      return boolean(entry.unknown_observed, `cases[${index}].unknown_observed`)
        && boolean(entry.original_id_match, `cases[${index}].original_id_match`)
        && !boolean(entry.replacement_id_observed, `cases[${index}].replacement_id_observed`);
    }) && operationIds.size === matrix.cases.length;
    return evaluated(m, passed, "all response-loss recovery reuses the original operation identity");
  },
  "CELLD.005.NO_SECOND_EFFECT": (raw) => {
    const m = object(raw);
    const matrix = exactCaseMatrix(m.cases, "cases", { substrate: substrates, action: lifecycleActions, trial: Array.from({ length: 100 }, (_, index) => index + 1) });
    const p95 = percentile(matrix.cases.map((entry) => entry.convergence_ms), 0.95, "convergence_ms");
    const passed = matrix.complete
      && matrix.cases.every((entry, index) => integer(entry.effect_records, `cases[${index}].effect_records`) === 1
        && integer(entry.attempts, `cases[${index}].attempts`, 1) <= 3)
      && p95 <= 30_000
      && boolean(m.proxy_healed, "proxy_healed");
    return evaluated({ ...m, derived_p95_ms: p95 }, passed, "unknown outcomes converge without a second provider effect");
  },
  "CELLD.006.PRE_PROVIDER": (raw) => {
    const m = object(raw);
    const matrix = exactCaseMatrix(m.cases, "cases", { substrate: substrates, action: ["stop", "destroy"], trial: Array.from({ length: 100 }, (_, index) => index + 1) });
    const operationIds = new Set();
    const passed = matrix.complete && matrix.cases.every((entry, index) => {
      operationIds.add(sha256Digest(entry.operation_id_sha256, `cases[${index}].operation_id_sha256`));
      return integer(entry.response_status, `cases[${index}].response_status`) === 409
        && string(entry.response_code, `cases[${index}].response_code`) === "celld.stale_generation_fenced"
        && integer(entry.provider_effects, `cases[${index}].provider_effects`) === 0;
    }) && operationIds.size === matrix.cases.length;
    return evaluated(m, passed, "stale destructive commands are fenced before provider dispatch");
  },
  "CELLD.006.ACTIVE_SAFE": (raw) => {
    const m = object(raw);
    const matrix = exactCaseMatrix(m.cases, "cases", { substrate: substrates });
    const passed = matrix.complete && matrix.cases.every((entry, index) => {
      const before = sha256Digest(entry.active_checksum_before, `cases[${index}].active_checksum_before`);
      const after = sha256Digest(entry.active_checksum_after, `cases[${index}].active_checksum_after`);
      return integer(entry.future_response_status, `cases[${index}].future_response_status`) === 409
        && string(entry.future_response_code, `cases[${index}].future_response_code`) === "cell.generation_fenced"
        && before === after
        && boolean(entry.partition_applied, `cases[${index}].partition_applied`)
        && boolean(entry.partition_healed, `cases[${index}].partition_healed`)
        && boolean(entry.baseline_after_heal_succeeded, `cases[${index}].baseline_after_heal_succeeded`);
    });
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
    const restores = objects(m.restores, "restores");
    const snapshotVersions = restores.map((restore, index) => string(restore.snapshot_version_id, `restores[${index}].snapshot_version_id`));
    const restorePrefixes = restores.map((restore, index) => string(restore.restore_prefix, `restores[${index}].restore_prefix`));
    const passed = restores.length === 2
      && new Set(snapshotVersions).size === 2
      && new Set(restorePrefixes).size === 2
      && restores.every((restore, index) => {
        const latestAcknowledgedAt = number(restore.latest_acknowledged_at_ms, `restores[${index}].latest_acknowledged_at_ms`);
        const snapshotCapturedAt = number(restore.snapshot_captured_at_ms, `restores[${index}].snapshot_captured_at_ms`);
        const restoreStartedAt = number(restore.restore_started_at_ms, `restores[${index}].restore_started_at_ms`);
        const restoreReadyAt = number(restore.restore_ready_at_ms, `restores[${index}].restore_ready_at_ms`);
        const sourcePrefix = string(restore.source_prefix, `restores[${index}].source_prefix`);
        const generationBefore = sha256Digest(restore.generation_manifest_before_sha256, `restores[${index}].generation_manifest_before_sha256`);
        const generationAfter = sha256Digest(restore.generation_manifest_after_sha256, `restores[${index}].generation_manifest_after_sha256`);
        const tombstonesBefore = sha256Digest(restore.tombstone_manifest_before_sha256, `restores[${index}].tombstone_manifest_before_sha256`);
        const tombstonesAfter = sha256Digest(restore.tombstone_manifest_after_sha256, `restores[${index}].tombstone_manifest_after_sha256`);
        return integer(restore.execution, `restores[${index}].execution`, 1) === index + 1
          && sourcePrefix !== restore.restore_prefix
          && boolean(restore.isolated_restore, `restores[${index}].isolated_restore`)
          && boolean(restore.quarantined, `restores[${index}].quarantined`)
          && boolean(restore.source_writers_stopped, `restores[${index}].source_writers_stopped`)
          && boolean(restore.restore_authority_exclusive, `restores[${index}].restore_authority_exclusive`)
          && latestAcknowledgedAt <= snapshotCapturedAt
          && snapshotCapturedAt <= restoreStartedAt
          && restoreStartedAt <= restoreReadyAt
          && snapshotCapturedAt - latestAcknowledgedAt <= 300_000
          && restoreReadyAt - restoreStartedAt <= 1_800_000
          && generationAfter === generationBefore
          && tombstonesAfter === tombstonesBefore;
      });
    return evaluated(m, passed, "both restore exercises meet RPO/RTO and state completeness");
  },
  "CELLD.015.IDEMPOTENT": (raw) => {
    const m = object(raw);
    const runbooks = objects(m.runbooks, "runbooks");
    const names = runbooks.map((runbook, index) => string(runbook.runbook, `runbooks[${index}].runbook`));
    const passed = runbooks.length === recoveryRunbooks.length
      && exactStrings(names, recoveryRunbooks, "runbooks.names")
      && new Set(names).size === recoveryRunbooks.length
      && runbooks.every((runbook, runbookIndex) => {
        const executions = objects(runbook.executions, `runbooks[${runbookIndex}].executions`);
        if (executions.length !== 2) return false;
        const firstOperations = strings(executions[0].operation_ids, `runbooks[${runbookIndex}].executions[0].operation_ids`);
        const secondOperations = strings(executions[1].operation_ids, `runbooks[${runbookIndex}].executions[1].operation_ids`);
        const firstEffects = strings(executions[0].lifecycle_effect_ids, `runbooks[${runbookIndex}].executions[0].lifecycle_effect_ids`);
        const secondEffects = strings(executions[1].lifecycle_effect_ids, `runbooks[${runbookIndex}].executions[1].lifecycle_effect_ids`);
        const firstState = sha256Digest(executions[0].state_sha256_after, `runbooks[${runbookIndex}].executions[0].state_sha256_after`);
        const secondState = sha256Digest(executions[1].state_sha256_after, `runbooks[${runbookIndex}].executions[1].state_sha256_after`);
        return integer(executions[0].ordinal, `runbooks[${runbookIndex}].executions[0].ordinal`, 1) === 1
          && integer(executions[1].ordinal, `runbooks[${runbookIndex}].executions[1].ordinal`, 1) === 2
          && firstOperations.length >= 1
          && firstEffects.length >= 1
          && new Set(firstOperations).size === firstOperations.length
          && new Set(firstEffects).size === firstEffects.length
          && exactStrings(secondOperations, firstOperations, `runbooks[${runbookIndex}].second_operation_ids`)
          && new Set(secondOperations).size === secondOperations.length
          && exactStrings(secondEffects, firstEffects, `runbooks[${runbookIndex}].second_effect_ids`)
          && new Set(secondEffects).size === secondEffects.length
          && secondState === firstState
          && boolean(runbook.healed, `runbooks[${runbookIndex}].healed`)
          && boolean(runbook.cleanup_verified, `runbooks[${runbookIndex}].cleanup_verified`);
      });
    return evaluated(m, passed, "the second recovery runbook execution creates no additional effects");
  },
  "CELLD.015.EVIDENCE": (raw) => {
    const m = object(raw);
    const affectedFleetStore = string(m.affected_fleet_store_id, "affected_fleet_store_id");
    const externalStore = string(m.external_evidence_store_id, "external_evidence_store_id");
    const artifacts = objects(m.artifacts, "artifacts");
    const kinds = artifacts.map((artifact, index) => string(artifact.kind, `artifacts[${index}].kind`));
    const passed = externalStore !== affectedFleetStore
      && artifacts.length === recoveryEvidenceKinds.length
      && exactStrings(kinds, recoveryEvidenceKinds, "artifacts.kinds")
      && new Set(kinds).size === recoveryEvidenceKinds.length
      && artifacts.every((artifact, index) => {
        const expected = sha256Digest(artifact.sha256, `artifacts[${index}].sha256`);
        const downloaded = sha256Digest(artifact.downloaded_sha256, `artifacts[${index}].downloaded_sha256`);
        const corruption = object(artifact.corruption_probe, `artifacts[${index}].corruption_probe`);
        const tampered = sha256Digest(corruption.tampered_sha256, `artifacts[${index}].corruption_probe.tampered_sha256`);
        return string(artifact.storage_authority_id, `artifacts[${index}].storage_authority_id`) === externalStore
          && integer(artifact.bytes, `artifacts[${index}].bytes`, 1) >= 1
          && downloaded === expected
          && tampered !== expected
          && boolean(corruption.detected, `artifacts[${index}].corruption_probe.detected`)
          && boolean(artifact.read_after_fleet_loss, `artifacts[${index}].read_after_fleet_loss`)
          && boolean(artifact.retained, `artifacts[${index}].retained`);
      })
      && boolean(m.affected_fleet_unavailable, "affected_fleet_unavailable")
      && boolean(m.external_evidence_store_reachable, "external_evidence_store_reachable")
      && boolean(m.manifest_verified, "manifest_verified")
      && boolean(m.malicious_runner_tamper_proof_claimed, "malicious_runner_tamper_proof_claimed") === false;
    return evaluated(m, passed, "recovery evidence survives fleet loss and detects corruption");
  },
});

export const WITHHELD_LIVE_EVALUATOR_IDS = Object.freeze([
  "CELLD.013.NO_LEAK",
  "CELLD.013.SCOPE",
  "CELLD.013.PROVENANCE",
]);
