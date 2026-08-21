import { createHash } from "node:crypto";

const DIGEST = /^sha256:[0-9a-f]{64}$/;
const VERSION = /^v?\d+\.\d+\.\d+(?:[-+][A-Za-z0-9.-]+)?$/;
const REQUIRED_ADAPTER_METHODS = Object.freeze([
  "persistIntent",
  "observeFleet",
  "drainNode",
  "replaceNode",
  "killNode",
  "restoreNode",
  "injectRollbackSignal",
  "observeSafety",
  "observeInventory",
]);

function canonicalJson(value) {
  if (value === null || typeof value === "boolean" || typeof value === "string") return JSON.stringify(value);
  if (typeof value === "number" && Number.isFinite(value)) return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value && typeof value === "object") return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
  throw new Error("rollout inventory contains a non-JSON value");
}

function sha256(value) {
  return `sha256:${createHash("sha256").update(value).digest("hex")}`;
}

function artifact(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)
    || !VERSION.test(value.version ?? "") || !DIGEST.test(value.manifest_digest ?? "")) {
    throw new Error(`${label} Celld artifact is invalid`);
  }
  return { version: value.version, manifest_digest: value.manifest_digest };
}

function validatePlan(plan) {
  if (!plan || typeof plan !== "object" || Array.isArray(plan)) throw new Error("rollout plan is invalid");
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(plan.run_id ?? "")) throw new Error("rollout run_id is invalid");
  const previous = artifact(plan.previous, "previous");
  const candidate = artifact(plan.candidate, "candidate");
  if (previous.version === candidate.version || previous.manifest_digest === candidate.manifest_digest) throw new Error("rollout artifacts must have distinct versions and digests");
  if (!Array.isArray(plan.candidate.compatible_from) || !plan.candidate.compatible_from.includes(previous.version)) throw new Error("rollout version pair is not explicitly compatible");
  if (plan.max_unavailable !== 1 || plan.reserve !== 1) throw new Error("rollout requires max_unavailable one and reserve one");
  if (!Number.isSafeInteger(plan.drain_timeout_ms) || plan.drain_timeout_ms < 1_000 || plan.drain_timeout_ms > 300_000) throw new Error("rollout drain timeout must be between one and 300 seconds");
  if (!Array.isArray(plan.nodes) || plan.nodes.length !== 3) throw new Error("rollout requires exactly three nodes");
  const names = new Set();
  let reserves = 0;
  for (const node of plan.nodes) {
    if (!node || typeof node !== "object" || !/^[a-z0-9][a-z0-9_.-]{0,127}$/.test(node.name ?? "") || names.has(node.name)) throw new Error("rollout node identity is invalid or duplicated");
    names.add(node.name);
    if (!new Set(["active", "reserve"]).has(node.role)) throw new Error(`rollout node ${node.name} has an invalid role`);
    if (node.role === "reserve") reserves += 1;
    if (node.manifest_digest !== previous.manifest_digest) throw new Error(`rollout node ${node.name} is not on the previous approved digest`);
  }
  if (reserves !== 1) throw new Error("rollout requires exactly one declared reserve node");
  if (plan.authorization?.destructive_faults !== true || plan.authorization?.exact_run_owner !== plan.run_id) throw new Error("exact-run destructive rollout authorization is required");
  return { previous, candidate };
}

function validateAdapter(adapter) {
  for (const method of REQUIRED_ADAPTER_METHODS) if (typeof adapter?.[method] !== "function") throw new Error(`rollout adapter.${method} is required`);
}

function validateFleetObservation(observation, plan) {
  if (!observation || !Array.isArray(observation.nodes) || observation.nodes.length !== plan.nodes.length) throw new Error("fleet observation has the wrong node count");
  const configured = new Map(plan.nodes.map((node) => [node.name, node]));
  const observed = new Set();
  for (const node of observation.nodes) {
    if (!configured.has(node?.name) || observed.has(node.name) || typeof node.running !== "boolean" || typeof node.ready !== "boolean" || typeof node.member !== "boolean" || !DIGEST.test(node.manifest_digest ?? "")) throw new Error("fleet observation contains an invalid node");
    observed.add(node.name);
  }
  return observation;
}

function percentile95(samples) {
  if (!Array.isArray(samples) || samples.length === 0 || samples.some((value) => !Number.isFinite(value) || value < 0)) throw new Error("reconciliation samples are invalid");
  const ordered = [...samples].sort((a, b) => a - b);
  return ordered[Math.ceil(ordered.length * 0.95) - 1];
}

function safetyMeasurements(safety, approvedDigestsRestored) {
  if (!safety || !Array.isArray(safety.acknowledged_intent_ids) || !Array.isArray(safety.rebuilt_intent_ids)
    || !Array.isArray(safety.effects) || !safety.current_generations || typeof safety.current_generations !== "object") {
    throw new Error("rollout safety observation is invalid");
  }
  if (safety.acknowledged_intent_ids.some((id) => typeof id !== "string" || id === "")
    || safety.rebuilt_intent_ids.some((id) => typeof id !== "string" || id === "")
    || new Set(safety.acknowledged_intent_ids).size !== safety.acknowledged_intent_ids.length
    || new Set(safety.rebuilt_intent_ids).size !== safety.rebuilt_intent_ids.length) {
    throw new Error("rollout intent observations are invalid or duplicated");
  }
  const rebuilt = new Set(safety.rebuilt_intent_ids);
  const lostIntents = safety.acknowledged_intent_ids.filter((id) => !rebuilt.has(id)).length;
  const effects = new Set();
  let duplicateEffects = 0;
  let staleEffects = 0;
  for (const effect of safety.effects) {
    if (typeof effect?.operation_id !== "string" || effect.operation_id === "" || !Number.isSafeInteger(effect.generation) || effect.generation < 1 || typeof effect.instance_id !== "string" || effect.instance_id === "" || !Number.isSafeInteger(safety.current_generations[effect.instance_id])) throw new Error("rollout effect observation is invalid");
    const key = `${effect.operation_id}:${effect.generation}`;
    if (effects.has(key)) duplicateEffects += 1;
    effects.add(key);
    if (safety.current_generations[effect.instance_id] !== effect.generation) staleEffects += 1;
  }
  return {
    lost_intents: lostIntents,
    duplicate_effects: duplicateEffects,
    stale_effects: staleEffects,
    reconcile_p95_ms: percentile95(safety.reconcile_samples_ms),
    approved_digests_restored: approvedDigestsRestored,
  };
}

function thresholdBreached(safety) {
  if (!Number.isFinite(safety.error_rate) || safety.error_rate < 0
    || !Number.isFinite(safety.coordination_p99_ms) || safety.coordination_p99_ms < 0
    || !Number.isFinite(safety.baseline_coordination_p99_ms) || safety.baseline_coordination_p99_ms <= 0) {
    throw new Error("rollout threshold observation is invalid");
  }
  const lifecycleFailure = safetyMeasurements(safety, false);
  return safety.error_rate > 0.01
    || safety.coordination_p99_ms > safety.baseline_coordination_p99_ms * 1.2
    || lifecycleFailure.lost_intents > 0
    || lifecycleFailure.duplicate_effects > 0
    || lifecycleFailure.stale_effects > 0;
}

export function incompatiblePairRefusal(inventory, { previous, candidate }, inventoryAfter = inventory) {
  const before = sha256(canonicalJson(inventory));
  let refused = false;
  try {
    artifact(previous, "previous");
    artifact(candidate, "candidate");
    if (previous.version === candidate.version || previous.manifest_digest === candidate.manifest_digest
      || !Array.isArray(candidate.compatible_from) || !candidate.compatible_from.includes(previous.version)) refused = true;
  } catch {
    refused = true;
  }
  if (!refused) throw new Error("refusal probe must use an incompatible or unknown pair");
  return { refused: true, node_mutations: 0, inventory_sha256_before: before, inventory_sha256_after: sha256(canonicalJson(inventoryAfter)) };
}

export async function executeRolloutController(plan, adapter) {
  const { previous, candidate } = validatePlan(plan);
  validateAdapter(adapter);
  const timeline = [];
  let mutationSequence = 0;
  let maxUnavailableObserved = 0;
  let reserveConsumed = 0;

  const observe = async (phase) => {
    const fleet = validateFleetObservation(await adapter.observeFleet(), plan);
    const unavailable = fleet.nodes.filter((node) => !node.running || !node.ready || !node.member).length;
    const reserve = fleet.nodes.find((node) => plan.nodes.find((item) => item.name === node.name)?.role === "reserve");
    maxUnavailableObserved = Math.max(maxUnavailableObserved, unavailable);
    if (!reserve.running || !reserve.ready || !reserve.member || reserve.manifest_digest !== previous.manifest_digest) reserveConsumed = 1;
    timeline.push({ sequence: timeline.length + 1, phase, unavailable, nodes: fleet.nodes.map((node) => ({ name: node.name, running: node.running, ready: node.ready, member: node.member, manifest_digest: node.manifest_digest })) });
    if (unavailable > plan.max_unavailable) throw new Error("rollout exceeded max_unavailable");
    if (reserveConsumed !== 0) throw new Error("rollout consumed the declared reserve");
    return fleet;
  };

  const mutate = async (kind, node, targetDigest, operation) => {
    mutationSequence += 1;
    const intent = { schema_version: "agentic-sandbox.celld-rollout-intent/v1", run_id: plan.run_id, sequence: mutationSequence, kind, node: node.name, target_manifest_digest: targetDigest };
    await adapter.persistIntent(intent);
    timeline.push({ sequence: timeline.length + 1, phase: "mutation_intent_persisted", intent });
    await operation();
  };

  const initial = await observe("preflight");
  if (initial.nodes.some((node) => !node.running || !node.ready || !node.member || node.manifest_digest !== previous.manifest_digest)) throw new Error("rollout preflight requires a healthy fleet on the previous digest");

  const activeNodes = plan.nodes.filter((node) => node.role === "active");
  let finalFleet;
  try {
    for (const node of activeNodes) {
      await mutate("drain", node, candidate.manifest_digest, () => adapter.drainNode(node.name, plan.drain_timeout_ms));
      await observe(`drained:${node.name}`);
      await mutate("replace", node, candidate.manifest_digest, () => adapter.replaceNode(node.name, candidate));
      const replaced = await observe(`replaced:${node.name}`);
      const state = replaced.nodes.find((item) => item.name === node.name);
      if (!state.running || !state.ready || !state.member || state.manifest_digest !== candidate.manifest_digest) throw new Error(`replacement node ${node.name} did not become ready on the candidate digest`);
    }

    const failedNode = activeNodes[0];
    await mutate("kill", failedNode, candidate.manifest_digest, () => adapter.killNode(failedNode.name));
    await observe(`killed:${failedNode.name}`);
    await mutate("restore", failedNode, candidate.manifest_digest, () => adapter.restoreNode(failedNode.name, candidate));
    const restored = await observe(`restored:${failedNode.name}`);
    const restoredState = restored.nodes.find((node) => node.name === failedNode.name);
    if (!restoredState.running || !restoredState.ready || !restoredState.member || restoredState.manifest_digest !== candidate.manifest_digest) throw new Error("killed node did not rebuild on the candidate digest");

    await mutate("inject_rollback_signal", failedNode, candidate.manifest_digest, () => adapter.injectRollbackSignal(failedNode.name));
    const thresholdObservation = await adapter.observeSafety();
    if (!thresholdBreached(thresholdObservation)) throw new Error("rollout rollback threshold was not breached");
    timeline.push({ sequence: timeline.length + 1, phase: "rollback_threshold_breached" });

    for (const node of [...activeNodes].reverse()) {
      await mutate("drain_for_rollback", node, previous.manifest_digest, () => adapter.drainNode(node.name, plan.drain_timeout_ms));
      await observe(`rollback_drained:${node.name}`);
      await mutate("rollback", node, previous.manifest_digest, () => adapter.replaceNode(node.name, previous));
      await observe(`rolled_back:${node.name}`);
    }
    finalFleet = await observe("complete");
  } catch (error) {
    const rollbackErrors = [];
    try {
      const current = validateFleetObservation(await adapter.observeFleet(), plan);
      for (const node of [...activeNodes].reverse()) {
        const state = current.nodes.find((item) => item.name === node.name);
        if (state.running && state.ready && state.member && state.manifest_digest === previous.manifest_digest) continue;
        try {
          await mutate("emergency_rollback", node, previous.manifest_digest, () => adapter.replaceNode(node.name, previous));
        } catch (rollbackError) {
          rollbackErrors.push(`${node.name}:${rollbackError.message}`);
        }
      }
      await observe("emergency_rollback_complete");
    } catch (rollbackError) {
      rollbackErrors.push(rollbackError.message);
    }
    if (rollbackErrors.length) throw new Error(`rollout failed (${error.message}) and emergency rollback failed (${rollbackErrors.join(";")})`);
    throw error;
  }

  const membershipHealthy = finalFleet.nodes.every((node) => node.running && node.ready && node.member);
  const approvedDigestsRestored = membershipHealthy && finalFleet.nodes.every((node) => node.manifest_digest === previous.manifest_digest);
  const finalSafety = safetyMeasurements(await adapter.observeSafety(), approvedDigestsRestored);
  const inventoryBeforeRefusal = await adapter.observeInventory();
  const refusal = incompatiblePairRefusal(inventoryBeforeRefusal, { previous, candidate: { version: "v0.0.0-unknown", manifest_digest: `sha256:${"0".repeat(64)}`, compatible_from: [] } }, await adapter.observeInventory());

  return {
    assertions: [
      { id: "CELLD.011.BUDGET", measurements: { nodes_expected: 3, max_unavailable_observed: maxUnavailableObserved, reserve_consumed: reserveConsumed, membership_healthy: membershipHealthy } },
      { id: "CELLD.011.SAFETY", measurements: finalSafety },
      { id: "CELLD.011.REFUSAL", measurements: refusal },
    ],
    timeline,
    mutation_count: mutationSequence,
  };
}
