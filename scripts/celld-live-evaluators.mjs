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

function dateTime(value, name) {
  const observed = string(value, name);
  const timestamp = Date.parse(observed);
  if (!Number.isFinite(timestamp) || new Date(timestamp).toISOString() !== observed) throw new Error(`${name} must be a canonical RFC 3339 date-time`);
  return timestamp;
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
const crashPhaseEvidenceSchema = "agentic-sandbox.celld-crash-phase-evidence/v1";
const lifecycleProviderStates = Object.freeze({
  qemu: Object.freeze({
    provision: Object.freeze(["absent", "shut off"]),
    start: Object.freeze(["shut off", "running"]),
    stop: Object.freeze(["running", "shut off"]),
    destroy: Object.freeze(["shut off", "absent"]),
  }),
  docker: Object.freeze({
    provision: Object.freeze(["absent", "running"]),
    start: Object.freeze(["running", "running"]),
    stop: Object.freeze(["running", "exited"]),
    destroy: Object.freeze(["exited", "absent"]),
  }),
});
const excludedCapabilities = Object.freeze(["process", "pty", "workspace", "filesystem", "raw_network", "vm", "container", "host_api"]);
const advertisedCapabilities = Object.freeze(["fetch", "rpc", "storage", "alarm", "websocket", "outbound_https", "wasm", "assets"]);
const resourceFamilies = Object.freeze(["cpu", "memory", "request_rate", "storage", "resident_cells", "outbound"]);
const denialClasses = Object.freeze(["forged_body", "forged_mac", "stale_timestamp", "nonce_replay", "wrong_key", "zero_generation", "wrong_generation", "public_route", "cross_fleet_request"]);
const credentialKinds = Object.freeze(["s3_access_identity", "request_hmac", "mtls_identity", "celld_peer_secret", "fixture_administrator"]);
const credentialScanSurfaces = Object.freeze(["argv", "captured_env", "shell_trace", "logs", "crash_artifacts", "persistent_scratch", "support_evidence"]);
const provenanceMismatchFields = Object.freeze(["version", "commit", "digest", "signature"]);
const provenanceVerifiers = Object.freeze({
  version: "version_policy",
  commit: "commit_pin",
  digest: "digest_pin",
  signature: "signature_verification",
});
const credentialActivationMethods = Object.freeze({
  s3_access_identity: Object.freeze(["controlled_restart", "hot_reload"]),
  request_hmac: Object.freeze(["dual_key_overlap"]),
  mtls_identity: Object.freeze(["controlled_restart", "hot_reload"]),
  celld_peer_secret: Object.freeze(["controlled_restart"]),
  fixture_administrator: Object.freeze(["fixture_controller_only"]),
});
const credentialRoles = Object.freeze({
  s3_access_identity: Object.freeze({ owner: "fixture_controller", consumer: "celld_fleet" }),
  request_hmac: Object.freeze({ owner: "management_adapter", consumer: "management_and_worker" }),
  mtls_identity: Object.freeze({ owner: "project_ca", consumer: "management_and_callback_relay" }),
  celld_peer_secret: Object.freeze({ owner: "celld_store_authority", consumer: "celld_fleet" }),
  fixture_administrator: Object.freeze({ owner: "fixture_controller", consumer: "fixture_controller_only" }),
});

function providerObservation(raw, name, expectedSubstrate) {
  const observation = object(raw, name);
  const substrate = string(observation.substrate, `${name}.substrate`);
  if (substrate !== expectedSubstrate) throw new Error(`${name}.substrate does not match the case substrate`);
  const observedAt = dateTime(observation.observed_at, `${name}.observed_at`);
  const target = sha256Digest(observation.target_name_sha256, `${name}.target_name_sha256`);
  const present = boolean(observation.present, `${name}.present`);
  const providerStoragePresent = boolean(observation.provider_storage_present, `${name}.provider_storage_present`);
  const state = string(observation.state, `${name}.state`);
  if (!present) {
    if (state !== "absent" || observation.provider_identity_sha256 !== null || observation.configuration_sha256 !== null) {
      throw new Error(`${name} absent observation must not claim provider identity or configuration`);
    }
    return { observedAt, target, present, state, providerStoragePresent, identity: null, configuration: null };
  }
  return {
    observedAt,
    target,
    present,
    state,
    providerStoragePresent,
    identity: sha256Digest(observation.provider_identity_sha256, `${name}.provider_identity_sha256`),
    configuration: sha256Digest(observation.configuration_sha256, `${name}.configuration_sha256`),
  };
}

function sameProviderObservation(left, right) {
  return left.target === right.target
    && left.present === right.present
    && left.state === right.state
    && left.providerStoragePresent === right.providerStoragePresent
    && left.identity === right.identity
    && left.configuration === right.configuration;
}

function providerTransition(entry, index) {
  const before = providerObservation(entry.provider_before, `cases[${index}].provider_before`, entry.substrate);
  const after = providerObservation(entry.provider_after, `cases[${index}].provider_after`, entry.substrate);
  const expected = lifecycleProviderStates[entry.substrate][entry.action];
  const bothPresentStable = !before.present || !after.present
    || (before.identity === after.identity && before.configuration === after.configuration);
  const storageStatesValid = entry.substrate === "qemu"
    ? before.providerStoragePresent === before.present && after.providerStoragePresent === after.present
    : !before.providerStoragePresent && !after.providerStoragePresent;
  return {
    target: after.target,
    identity: after.identity ?? before.identity,
    configuration: after.configuration ?? before.configuration,
    passed: before.observedAt <= after.observedAt
    && before.target === after.target
    && before.state === expected[0]
    && after.state === expected[1]
    && bothPresentStable
    && storageStatesValid,
  };
}

function celldOwnerObservation(raw, name) {
  const observation = object(raw, name);
  return {
    observedAt: dateTime(observation.observed_at, `${name}.observed_at`),
    cell: sha256Digest(observation.cell_scope_sha256, `${name}.cell_scope_sha256`),
    target: sha256Digest(observation.owner_target_sha256, `${name}.owner_target_sha256`),
    node: sha256Digest(observation.owner_node_id_sha256, `${name}.owner_node_id_sha256`),
    epoch: integer(observation.owner_epoch, `${name}.owner_epoch`),
    liveNodes: integer(observation.live_nodes, `${name}.live_nodes`),
    routeAgreement: boolean(observation.route_agreement, `${name}.route_agreement`),
  };
}
const credentialDeliveryMethods = Object.freeze(["protected_tmpfs_file", "protected_inherited_fd"]);
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

const ALL_LIVE_EVALUATORS = Object.freeze({
  "CELLD.003.ONE_EFFECT": (raw) => {
    const m = object(raw);
    const matrix = exactCaseMatrix(m.cases, "cases", { substrate: substrates, action: lifecycleActions });
    const operationIds = new Set();
    const managementIds = new Set();
    const providerTargets = new Map(substrates.map((substrate) => [substrate, new Set()]));
    const providerIdentities = new Map(substrates.map((substrate) => [substrate, new Set()]));
    const providerConfigurations = new Map(substrates.map((substrate) => [substrate, new Set()]));
    const passed = matrix.complete && matrix.cases.every((entry, index) => {
      operationIds.add(sha256Digest(entry.operation_id_sha256, `cases[${index}].operation_id_sha256`));
      managementIds.add(sha256Digest(entry.management_operation_id_sha256, `cases[${index}].management_operation_id_sha256`));
      const transition = providerTransition(entry, index);
      providerTargets.get(entry.substrate).add(transition.target);
      providerIdentities.get(entry.substrate).add(transition.identity);
      providerConfigurations.get(entry.substrate).add(transition.configuration);
      return integer(entry.replay_count, `cases[${index}].replay_count`) === 10_000
        && integer(entry.replay_http_200, `cases[${index}].replay_http_200`) === 10_000
        && integer(entry.replay_management_operation_matches, `cases[${index}].replay_management_operation_matches`) === 10_000
        && integer(entry.replay_terminal_status_matches, `cases[${index}].replay_terminal_status_matches`) === 10_000
        && integer(entry.replay_terminal_code_matches, `cases[${index}].replay_terminal_code_matches`) === 10_000
        && integer(entry.replay_result_matches, `cases[${index}].replay_result_matches`) === 10_000
        && integer(entry.replay_provider_dispatch_count_matches, `cases[${index}].replay_provider_dispatch_count_matches`) === 10_000
        && integer(entry.effect_records, `cases[${index}].effect_records`) === 1
        && integer(entry.provider_dispatch_count, `cases[${index}].provider_dispatch_count`) === 1
        && transition.passed;
    })
      && operationIds.size === matrix.cases.length
      && managementIds.size === matrix.cases.length
      && substrates.every((substrate) => providerTargets.get(substrate).size === 1
        && providerIdentities.get(substrate).size === 1
        && providerConfigurations.get(substrate).size === 1);
    return evaluated(m, passed, "exactly one durable provider dispatch per lifecycle operation identity");
  },
  "CELLD.003.COLLISION": (raw) => {
    const m = object(raw);
    const matrix = exactCaseMatrix(m.cases, "cases", { substrate: substrates, action: lifecycleActions });
    const operationIds = new Set();
    const passed = matrix.complete && matrix.cases.every((entry, index) => {
      operationIds.add(sha256Digest(entry.operation_id_sha256, `cases[${index}].operation_id_sha256`));
      const providerBefore = providerObservation(entry.provider_before_collision, `cases[${index}].provider_before_collision`, entry.substrate);
      const providerAfter = providerObservation(entry.provider_after_collision, `cases[${index}].provider_after_collision`, entry.substrate);
      return integer(entry.response_status, `cases[${index}].response_status`) === 409
        && string(entry.response_code, `cases[${index}].response_code`) === "celld.operation_collision"
        && integer(entry.post_collision_replay_status, `cases[${index}].post_collision_replay_status`) === 200
        && boolean(entry.post_collision_terminal_matches, `cases[${index}].post_collision_terminal_matches`)
        && integer(entry.effect_records_before, `cases[${index}].effect_records_before`) === 1
        && integer(entry.effect_records_after, `cases[${index}].effect_records_after`) === 1
        && integer(entry.provider_dispatch_count_before, `cases[${index}].provider_dispatch_count_before`) === 1
        && boolean(entry.provider_dispatch_count_after_observed, `cases[${index}].provider_dispatch_count_after_observed`)
        && integer(entry.provider_dispatch_count_after, `cases[${index}].provider_dispatch_count_after`) === 1
        && providerBefore.observedAt <= providerAfter.observedAt
        && sameProviderObservation(providerBefore, providerAfter);
    }) && operationIds.size === matrix.cases.length;
    return evaluated(m, passed, "different-hash operation identity reuse is rejected before provider mutation");
  },
  "CELLD.004.NO_LOSS": (raw) => {
    const m = object(raw);
    const matrix = exactCaseMatrix(m.cases, "cases", { substrate: substrates, crash_point: crashPoints, trial: Array.from({ length: 100 }, (_, index) => index + 1) });
    const operationIds = new Set();
    const managementFaultIds = new Set();
    const ownerFaultIds = new Set();
    const passed = matrix.complete && matrix.cases.every((entry, index) => {
      const operationId = sha256Digest(entry.operation_id_sha256, `cases[${index}].operation_id_sha256`);
      operationIds.add(operationId);
      managementFaultIds.add(sha256Digest(entry.management_fault_id_sha256, `cases[${index}].management_fault_id_sha256`));
      ownerFaultIds.add(sha256Digest(entry.owner_fault_id_sha256, `cases[${index}].owner_fault_id_sha256`));
      const commandSentAt = dateTime(entry.command_sent_at, `cases[${index}].command_sent_at`);
      const acknowledgedAt = dateTime(entry.acknowledged_at, `cases[${index}].acknowledged_at`);
      const managementFaultAt = dateTime(entry.management_fault_applied_at, `cases[${index}].management_fault_applied_at`);
      const ownerFaultAt = dateTime(entry.owner_fault_applied_at, `cases[${index}].owner_fault_applied_at`);
      const phase = object(entry.phase_evidence, `cases[${index}].phase_evidence`);
      const phaseReachedAt = dateTime(phase.reached_at, `cases[${index}].phase_evidence.reached_at`);
      const expectedObserver = entry.crash_point === "before_dispatch" ? "management_process_absent" : "management_dispatch_gate";
      const phaseOrder = entry.crash_point === "before_dispatch"
        ? phaseReachedAt === managementFaultAt && managementFaultAt <= commandSentAt && commandSentAt <= acknowledgedAt && acknowledgedAt <= ownerFaultAt
        : commandSentAt <= acknowledgedAt && acknowledgedAt <= phaseReachedAt && phaseReachedAt <= managementFaultAt && managementFaultAt <= ownerFaultAt;
      return boolean(entry.acknowledged, `cases[${index}].acknowledged`)
        && boolean(entry.independently_faulted, `cases[${index}].independently_faulted`)
        && string(entry.terminal_status, `cases[${index}].terminal_status`) === "succeeded"
        && string(phase.schema_version, `cases[${index}].phase_evidence.schema_version`) === crashPhaseEvidenceSchema
        && sha256Digest(phase.operation_id_sha256, `cases[${index}].phase_evidence.operation_id_sha256`) === operationId
        && string(phase.phase, `cases[${index}].phase_evidence.phase`) === entry.crash_point
        && string(phase.observer, `cases[${index}].phase_evidence.observer`) === expectedObserver
        && integer(phase.management_pid, `cases[${index}].phase_evidence.management_pid`, 1) > 0
        && phaseOrder;
    })
      && operationIds.size === matrix.cases.length
      && managementFaultIds.size === matrix.cases.length
      && ownerFaultIds.size === matrix.cases.length;
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
      && matrix.cases.every((entry, index) => {
        const before = celldOwnerObservation(entry.owner_before, `cases[${index}].owner_before`);
        const afterLoss = celldOwnerObservation(entry.owner_after_loss, `cases[${index}].owner_after_loss`);
        const afterHeal = celldOwnerObservation(entry.owner_after_heal, `cases[${index}].owner_after_heal`);
        const providerBefore = providerObservation(entry.provider_before_fault, `cases[${index}].provider_before_fault`, entry.substrate);
        const providerAfter = providerObservation(entry.provider_after_heal, `cases[${index}].provider_after_heal`, entry.substrate);
        return integer(entry.effect_records, `cases[${index}].effect_records`) === 1
          && integer(entry.provider_dispatch_count, `cases[${index}].provider_dispatch_count`) === 1
          && sha256Digest(entry.fault_target_sha256, `cases[${index}].fault_target_sha256`) === before.target
          && before.routeAgreement
          && afterLoss.routeAgreement
          && afterHeal.routeAgreement
          && before.liveNodes === 3
          && afterLoss.liveNodes === 2
          && afterHeal.liveNodes === 3
          && before.epoch > 0
          && before.cell === afterLoss.cell
          && afterLoss.cell === afterHeal.cell
          && before.target !== afterLoss.target
          && before.node !== afterLoss.node
          && afterLoss.epoch > before.epoch
          && afterHeal.target === afterLoss.target
          && afterHeal.node === afterLoss.node
          && afterHeal.epoch === afterLoss.epoch
          && before.observedAt <= afterLoss.observedAt
          && afterLoss.observedAt <= afterHeal.observedAt
          && boolean(entry.baseline_after_heal_succeeded, `cases[${index}].baseline_after_heal_succeeded`)
          && integer(entry.baseline_provider_dispatch_count, `cases[${index}].baseline_provider_dispatch_count`) === 1
          && providerBefore.present
          && providerBefore.state === "running"
          && providerBefore.providerStoragePresent === (entry.substrate === "qemu")
          && providerBefore.observedAt <= providerAfter.observedAt
          && sameProviderObservation(providerBefore, providerAfter);
      })
      && providerMatrix.cases.every((entry, index) => {
        const before = providerObservation(entry.provider_before, `provider_cases[${index}].provider_before`, entry.substrate);
        const after = providerObservation(entry.provider_after, `provider_cases[${index}].provider_after`, entry.substrate);
        return before.present
          && before.state === "running"
          && before.providerStoragePresent === (entry.substrate === "qemu")
          && before.observedAt <= after.observedAt
          && sameProviderObservation(before, after);
      })
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
    const providerTargets = new Map(substrates.map((substrate) => [substrate, new Set()]));
    const passed = matrix.complete
      && matrix.cases.every((entry, index) => {
        const transition = providerTransition(entry, index);
        providerTargets.get(entry.substrate).add(transition.target);
        return integer(entry.effect_records, `cases[${index}].effect_records`) === 1
          && integer(entry.management_replay_status, `cases[${index}].management_replay_status`) === 200
          && boolean(entry.management_replay_terminal_matches, `cases[${index}].management_replay_terminal_matches`)
          && boolean(entry.provider_dispatch_count_observed, `cases[${index}].provider_dispatch_count_observed`)
          && integer(entry.provider_dispatch_count, `cases[${index}].provider_dispatch_count`) === 1
          && integer(entry.attempts, `cases[${index}].attempts`, 1) <= 3
          && transition.passed;
      })
      && substrates.every((substrate) => providerTargets.get(substrate).size === 1)
      && p95 <= 30_000
      && boolean(m.proxy_healed, "proxy_healed");
    return evaluated({ ...m, derived_p95_ms: p95 }, passed, "unknown outcomes converge without a second provider dispatch");
  },
  "CELLD.006.PRE_PROVIDER": (raw) => {
    const m = object(raw);
    const matrix = exactCaseMatrix(m.cases, "cases", { substrate: substrates, action: ["stop", "destroy"], trial: Array.from({ length: 100 }, (_, index) => index + 1) });
    const operationIds = new Set();
    const passed = matrix.complete && matrix.cases.every((entry, index) => {
      operationIds.add(sha256Digest(entry.operation_id_sha256, `cases[${index}].operation_id_sha256`));
      const before = providerObservation(entry.provider_before, `cases[${index}].provider_before`, entry.substrate);
      const after = providerObservation(entry.provider_after, `cases[${index}].provider_after`, entry.substrate);
      return integer(entry.response_status, `cases[${index}].response_status`) === 409
        && string(entry.response_code, `cases[${index}].response_code`) === "celld.stale_generation_fenced"
        && boolean(entry.provider_dispatch_count_delta_observed, `cases[${index}].provider_dispatch_count_delta_observed`)
        && integer(entry.provider_dispatch_count_delta, `cases[${index}].provider_dispatch_count_delta`) === 0
        && before.present
        && before.state === lifecycleProviderStates[entry.substrate].provision[1]
        && before.providerStoragePresent === (entry.substrate === "qemu")
        && before.observedAt <= after.observedAt
        && sameProviderObservation(before, after);
    }) && operationIds.size === matrix.cases.length;
    return evaluated(m, passed, "stale destructive commands are fenced before provider dispatch");
  },
  "CELLD.006.ACTIVE_SAFE": (raw) => {
    const m = object(raw);
    const matrix = exactCaseMatrix(m.cases, "cases", { substrate: substrates });
    const passed = matrix.complete && matrix.cases.every((entry, index) => {
      const before = providerObservation(entry.provider_before, `cases[${index}].provider_before`, entry.substrate);
      const after = providerObservation(entry.provider_after, `cases[${index}].provider_after`, entry.substrate);
      return integer(entry.future_response_status, `cases[${index}].future_response_status`) === 409
        && string(entry.future_response_code, `cases[${index}].future_response_code`) === "cell.generation_fenced"
        && before.present
        && before.state === lifecycleProviderStates[entry.substrate].provision[1]
        && before.providerStoragePresent === (entry.substrate === "qemu")
        && before.observedAt <= after.observedAt
        && sameProviderObservation(before, after)
        && boolean(entry.partition_applied, `cases[${index}].partition_applied`)
        && boolean(entry.partition_healed, `cases[${index}].partition_healed`)
        && boolean(entry.baseline_after_heal_succeeded, `cases[${index}].baseline_after_heal_succeeded`)
        && integer(entry.baseline_provider_dispatch_count, `cases[${index}].baseline_provider_dispatch_count`) === 1;
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
      && typeof m.previous_version_id === "string" && /^[0-9a-f]{16}$/.test(m.previous_version_id)
      && typeof m.candidate_version_id === "string" && /^[0-9a-f]{16}$/.test(m.candidate_version_id)
      && m.candidate_version_id !== m.previous_version_id
      && typeof m.restored_version_id === "string" && /^[0-9a-f]{16}$/.test(m.restored_version_id)
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
    const providerBefore = integer(m.provider_counter_before, "provider_counter_before");
    const providerAfter = integer(m.provider_counter_after, "provider_counter_after");
    const providerEffects = integer(m.provider_effects, "provider_effects");
    const passed = exactStrings(m.classes, ["public_internal", "cross_fleet", "cross_bucket"], "classes")
      && attempts === 3_000
      && integer(m.denied, "denied") === attempts
      && integer(m.succeeded, "succeeded") === 0
      && boolean(m.provider_counter_observed, "provider_counter_observed")
      && providerAfter - providerBefore === providerEffects
      && providerEffects === 0
      && boolean(m.routes_healed, "routes_healed")
      && integer(m.directional_partitions, "directional_partitions") === 4
      && boolean(m.partition_matrices_complete, "partition_matrices_complete")
      && integer(m.probe_concurrency_limit, "probe_concurrency_limit") === 32
      && integer(m.probe_max_in_flight, "probe_max_in_flight", 1) <= 32;
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
    const providerBefore = integer(m.provider_counter_before, "provider_counter_before");
    const providerAfter = integer(m.provider_counter_after, "provider_counter_after");
    const providerEffects = integer(m.provider_effects, "provider_effects");
    const passed = exactStrings(m.classes, denialClasses, "classes")
      && integer(m.attempts_per_class, "attempts_per_class") === 1_000
      && integer(m.attempts, "attempts") === expected
      && integer(m.denied, "denied") === expected
      && boolean(m.provider_counter_observed, "provider_counter_observed")
      && providerAfter - providerBefore === providerEffects
      && providerEffects === 0;
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
  "CELLD.013.NO_LEAK": (raw) => {
    const m = object(raw);
    const credentials = objects(m.protected_credentials, "protected_credentials");
    const scans = exactCaseMatrix(m.scans, "scans", { surface: credentialScanSurfaces });
    const kinds = new Set();
    const credentialIds = new Set();
    const credentialRefs = new Set();
    const canaryIds = new Set();
    const passed = credentials.length >= credentialKinds.length
      && scans.complete
      && credentials.every((entry, index) => {
        const kind = string(entry.secret_kind, `protected_credentials[${index}].secret_kind`);
        const delivery = string(entry.delivery, `protected_credentials[${index}].delivery`);
        kinds.add(kind);
        credentialIds.add(sha256Digest(entry.credential_id_sha256, `protected_credentials[${index}].credential_id_sha256`));
        credentialRefs.add(sha256Digest(entry.credential_ref_sha256, `protected_credentials[${index}].credential_ref_sha256`));
        canaryIds.add(sha256Digest(entry.canary_id_sha256, `protected_credentials[${index}].canary_id_sha256`));
        const protectedDelivery = delivery === "protected_tmpfs_file"
          ? ["0400", "0600"].includes(string(entry.mode_octal, `protected_credentials[${index}].mode_octal`))
            && boolean(entry.regular_file, `protected_credentials[${index}].regular_file`)
            && !boolean(entry.symlink, `protected_credentials[${index}].symlink`)
          : delivery === "protected_inherited_fd"
            && integer(entry.fd_number, `protected_credentials[${index}].fd_number`, 3) >= 3
            && boolean(entry.close_on_exec, `protected_credentials[${index}].close_on_exec`)
            && boolean(entry.inherited_by_exact_consumer, `protected_credentials[${index}].inherited_by_exact_consumer`);
        return credentialKinds.includes(kind)
          && credentialDeliveryMethods.includes(delivery)
          && protectedDelivery
          && boolean(entry.owner_only, `protected_credentials[${index}].owner_only`)
          && integer(entry.canary_matches_in_reference, `protected_credentials[${index}].canary_matches_in_reference`) === 1
          && integer(entry.canary_matches_outside_reference, `protected_credentials[${index}].canary_matches_outside_reference`) === 0
          && boolean(entry.revoked_or_removed, `protected_credentials[${index}].revoked_or_removed`);
      })
      && exactStrings([...kinds], credentialKinds, "protected_credentials.secret_kinds")
      && credentialIds.size === credentials.length
      && credentialRefs.size === credentials.length
      && canaryIds.size === credentials.length
      && [...credentialIds].every((id) => !credentialRefs.has(id) && !canaryIds.has(id))
      && [...credentialRefs].every((id) => !canaryIds.has(id))
      && scans.cases.every((entry, index) => {
        const expectedArtifacts = integer(entry.expected_artifact_count, `scans[${index}].expected_artifact_count`, 1);
        const expectedInventory = sha256Digest(entry.expected_inventory_sha256, `scans[${index}].expected_inventory_sha256`);
        const scannedInventory = sha256Digest(entry.scanned_inventory_sha256, `scans[${index}].scanned_inventory_sha256`);
        return integer(entry.artifacts_scanned, `scans[${index}].artifacts_scanned`, 1) === expectedArtifacts
          && expectedInventory === scannedInventory
          && integer(entry.canaries_expected, `scans[${index}].canaries_expected`, 1) === credentials.length
          && integer(entry.canaries_scanned, `scans[${index}].canaries_scanned`, 1) === credentials.length
          && integer(entry.canary_matches, `scans[${index}].canary_matches`) === 0;
      })
      && integer(m.unprotected_secret_files, "unprotected_secret_files") === 0
      && integer(m.evidence_secret_findings, "evidence_secret_findings") === 0
      && boolean(m.all_disposable_secrets_removed, "all_disposable_secrets_removed");
    return evaluated(m, passed, "every credential canary remains confined to one protected reference and all required surfaces are clean");
  },
  "CELLD.013.SCOPE": (raw) => {
    const m = object(raw);
    const lifecycles = exactCaseMatrix(m.lifecycles, "lifecycles", { secret_kind: credentialKinds });
    const crossScope = objects(m.cross_scope_cases, "cross_scope_cases");
    const sourceBucket = sha256Digest(m.source_bucket_sha256, "source_bucket_sha256");
    const evidenceIds = new Set();
    const targetBuckets = new Set();
    const lifecyclePassed = lifecycles.complete && lifecycles.cases.every((entry, index) => {
      const kind = entry.secret_kind;
      const method = string(entry.activation_method, `lifecycles[${index}].activation_method`);
      const delivery = string(entry.delivery, `lifecycles[${index}].delivery`);
      const overlap = number(entry.overlap_ms, `lifecycles[${index}].overlap_ms`);
      evidenceIds.add(sha256Digest(entry.evidence_id_sha256, `lifecycles[${index}].evidence_id_sha256`));
      const hotReload = method === "hot_reload";
      return credentialActivationMethods[kind]?.includes(method) === true
        && string(entry.owner, `lifecycles[${index}].owner`) === credentialRoles[kind]?.owner
        && string(entry.consumer, `lifecycles[${index}].consumer`) === credentialRoles[kind]?.consumer
        && credentialDeliveryMethods.includes(delivery)
        && boolean(entry.delivered_to_runtime, `lifecycles[${index}].delivered_to_runtime`) === (kind !== "fixture_administrator")
        && boolean(entry.activation_verified, `lifecycles[${index}].activation_verified`)
        && (!hotReload || boolean(entry.reload_proven, `lifecycles[${index}].reload_proven`))
        && (kind === "request_hmac" ? overlap > 0 && overlap <= 900_000 : overlap === 0)
        && boolean(entry.revocation_verified, `lifecycles[${index}].revocation_verified`)
        && boolean(entry.failure_recovered, `lifecycles[${index}].failure_recovered`)
        && boolean(entry.cleanup_verified, `lifecycles[${index}].cleanup_verified`);
    }) && evidenceIds.size === credentialKinds.length;
    const targetPassed = string(m.scope_mode, "scope_mode") === "per_fleet_bucket"
      && !boolean(m.shared_prefix_claimed, "shared_prefix_claimed")
      && crossScope.length === integer(m.other_fleet_bucket_count, "other_fleet_bucket_count", 1)
      && crossScope.every((entry, index) => {
        const target = sha256Digest(entry.target_bucket_sha256, `cross_scope_cases[${index}].target_bucket_sha256`);
        targetBuckets.add(target);
        const attempts = integer(entry.attempts, `cross_scope_cases[${index}].attempts`, 1);
        return target !== sourceBucket
          && string(entry.scope_kind, `cross_scope_cases[${index}].scope_kind`) === "other_fleet_bucket"
          && integer(entry.denied, `cross_scope_cases[${index}].denied`) === attempts
          && integer(entry.succeeded, `cross_scope_cases[${index}].succeeded`) === 0
          && integer(entry.provider_effects, `cross_scope_cases[${index}].provider_effects`) === 0;
      })
      && targetBuckets.size === crossScope.length;
    const originalConfig = sha256Digest(m.original_config_sha256, "original_config_sha256");
    const candidateConfig = sha256Digest(m.candidate_config_sha256, "candidate_config_sha256");
    const restoredConfig = sha256Digest(m.restored_config_sha256, "restored_config_sha256");
    const passed = lifecyclePassed
      && targetPassed
      && boolean(m.hmac_canary_succeeded, "hmac_canary_succeeded")
      && boolean(m.old_hmac_revoked_after_canary, "old_hmac_revoked_after_canary")
      && boolean(m.revoked_hmac_denied, "revoked_hmac_denied")
      && boolean(m.failed_canary_restored_original, "failed_canary_restored_original")
      && candidateConfig !== originalConfig
      && restoredConfig === originalConfig
      && boolean(m.active_path_healthy, "active_path_healthy");
    return evaluated(m, passed, "all other-fleet buckets are denied and each distinct secret lifecycle restores a healthy scoped path");
  },
  "CELLD.013.PROVENANCE": (raw) => {
    const m = object(raw);
    const matrix = exactCaseMatrix(m.cases, "cases", { mismatch_field: provenanceMismatchFields });
    const candidates = new Set();
    const passed = matrix.complete
      && matrix.cases.every((entry, index) => {
        const field = entry.mismatch_field;
        const before = sha256Digest(entry.approved_identity_before_sha256, `cases[${index}].approved_identity_before_sha256`);
        const after = sha256Digest(entry.approved_identity_after_sha256, `cases[${index}].approved_identity_after_sha256`);
        const candidate = sha256Digest(entry.candidate_identity_sha256, `cases[${index}].candidate_identity_sha256`);
        const approvedFields = object(entry.approved_fields_sha256, `cases[${index}].approved_fields_sha256`);
        const candidateFields = object(entry.candidate_fields_sha256, `cases[${index}].candidate_fields_sha256`);
        const fieldNamesComplete = exactStrings(Object.keys(approvedFields), provenanceMismatchFields, `cases[${index}].approved_fields_sha256.keys`)
          && exactStrings(Object.keys(candidateFields), provenanceMismatchFields, `cases[${index}].candidate_fields_sha256.keys`);
        const fieldDifferenceExact = fieldNamesComplete && provenanceMismatchFields.every((identityField) => {
          const approvedValue = sha256Digest(approvedFields[identityField], `cases[${index}].approved_fields_sha256.${identityField}`);
          const candidateValue = sha256Digest(candidateFields[identityField], `cases[${index}].candidate_fields_sha256.${identityField}`);
          return identityField === field ? candidateValue !== approvedValue : candidateValue === approvedValue;
        });
        candidates.add(candidate);
        const attempts = integer(entry.install_attempts, `cases[${index}].install_attempts`, 1);
        return candidate !== before
          && after === before
          && fieldDifferenceExact
          && string(entry.verifier, `cases[${index}].verifier`) === provenanceVerifiers[field]
          && boolean(entry.verifier_executed, `cases[${index}].verifier_executed`)
          && string(entry.verifier_result, `cases[${index}].verifier_result`) === "mismatch"
          && sha256Digest(entry.verifier_evidence_sha256, `cases[${index}].verifier_evidence_sha256`).length === 64
          && integer(entry.blocked_attempts, `cases[${index}].blocked_attempts`) === attempts
          && integer(entry.install_effects, `cases[${index}].install_effects`) === 0
          && string(entry.response_code, `cases[${index}].response_code`) === "celld.provenance_mismatch"
          && boolean(entry.mismatch_detected, `cases[${index}].mismatch_detected`);
      })
      && candidates.size === provenanceMismatchFields.length
      && integer(m.approved_pin_count, "approved_pin_count", 1) >= 1
      && integer(m.unapproved_pin_count, "unapproved_pin_count") === 0
      && boolean(m.only_approved_pins_remain, "only_approved_pins_remain");
    return evaluated(m, passed, "every version, commit, digest, and signature mismatch is blocked before installation");
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

export const CANDIDATE_LIVE_EVALUATORS = Object.freeze(Object.fromEntries(
  WITHHELD_LIVE_EVALUATOR_IDS.map((id) => [id, ALL_LIVE_EVALUATORS[id]]),
));

export const SAFE_LIVE_EVALUATORS = Object.freeze(Object.fromEntries(
  Object.entries(ALL_LIVE_EVALUATORS).filter(([id]) => !WITHHELD_LIVE_EVALUATOR_IDS.includes(id)),
));
