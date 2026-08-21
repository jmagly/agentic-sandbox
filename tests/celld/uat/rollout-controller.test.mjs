import assert from "node:assert/strict";
import test from "node:test";

import { executeRolloutController, incompatiblePairRefusal } from "../../../scripts/celld-rollout-controller.mjs";

const previous = { version: "v0.2.1", manifest_digest: `sha256:${"a".repeat(64)}` };
const candidate = { version: "v0.3.0", manifest_digest: `sha256:${"b".repeat(64)}`, compatible_from: ["v0.2.1"] };

function plan(overrides = {}) {
  return {
    run_id: "titan-123",
    previous,
    candidate,
    max_unavailable: 1,
    reserve: 1,
    drain_timeout_ms: 120_000,
    authorization: { destructive_faults: true, exact_run_owner: "titan-123" },
    nodes: [
      { name: "celld-node-1", role: "active", manifest_digest: previous.manifest_digest },
      { name: "celld-node-2", role: "active", manifest_digest: previous.manifest_digest },
      { name: "celld-node-3", role: "reserve", manifest_digest: previous.manifest_digest },
    ],
    ...overrides,
  };
}

function adapter() {
  const nodes = new Map(plan().nodes.map((node) => [node.name, { ...node, running: true, ready: true, member: true }]));
  const events = [];
  const requireIntent = (kind, name) => {
    const last = events.at(-1);
    assert.equal(last?.type, "intent", `${kind} ran without a directly preceding persisted intent`);
    assert.ok([kind].flat().includes(last.intent.kind), `${last.intent.kind} did not authorize ${kind}`);
    assert.equal(last.intent.node, name);
  };
  let rollbackSignal = false;
  return {
    events,
    persistIntent: async (intent) => events.push({ type: "intent", intent: structuredClone(intent) }),
    observeFleet: async () => ({ nodes: [...nodes.values()].map((node) => structuredClone(node)) }),
    observeInventory: async () => ({ nodes: [...nodes.values()].map(({ name, role, manifest_digest }) => ({ name, role, manifest_digest })) }),
    drainNode: async (name) => {
      const kind = nodes.get(name).manifest_digest === candidate.manifest_digest ? "drain_for_rollback" : "drain";
      requireIntent(kind, name);
      Object.assign(nodes.get(name), { ready: false, member: false });
      events.push({ type: "mutation", kind, name });
    },
    replaceNode: async (name, artifact) => {
      const kind = artifact.manifest_digest === previous.manifest_digest ? "rollback" : "replace";
      requireIntent(kind === "rollback" ? ["rollback", "emergency_rollback"] : kind, name);
      Object.assign(nodes.get(name), { running: true, ready: true, member: true, manifest_digest: artifact.manifest_digest });
      events.push({ type: "mutation", kind, name });
    },
    killNode: async (name) => {
      requireIntent("kill", name);
      Object.assign(nodes.get(name), { running: false, ready: false, member: false });
      events.push({ type: "mutation", kind: "kill", name });
    },
    restoreNode: async (name, artifact) => {
      requireIntent("restore", name);
      Object.assign(nodes.get(name), { running: true, ready: true, member: true, manifest_digest: artifact.manifest_digest });
      events.push({ type: "mutation", kind: "restore", name });
    },
    injectRollbackSignal: async (name) => {
      requireIntent("inject_rollback_signal", name);
      rollbackSignal = true;
      events.push({ type: "mutation", kind: "inject_rollback_signal", name });
    },
    observeSafety: async () => ({
      acknowledged_intent_ids: ["intent-1", "intent-2"],
      rebuilt_intent_ids: ["intent-1", "intent-2"],
      effects: [
        { operation_id: "operation-1", instance_id: "instance-1", generation: 2 },
        { operation_id: "operation-2", instance_id: "instance-2", generation: 4 },
      ],
      current_generations: { "instance-1": 2, "instance-2": 4 },
      reconcile_samples_ms: [100, 150, 200, 250, 300],
      error_rate: rollbackSignal ? 0.02 : 0,
      baseline_coordination_p99_ms: 100,
      coordination_p99_ms: 100,
    }),
  };
}

test("one-node rollout, kill/rebuild, threshold rollback, and final restoration are measured", async () => {
  const fixture = adapter();
  const result = await executeRolloutController(plan(), fixture);
  const assertions = Object.fromEntries(result.assertions.map((entry) => [entry.id, entry.measurements]));

  assert.deepEqual(assertions["CELLD.011.BUDGET"], { nodes_expected: 3, max_unavailable_observed: 1, reserve_consumed: 0, membership_healthy: true });
  assert.deepEqual(assertions["CELLD.011.SAFETY"], { lost_intents: 0, duplicate_effects: 0, stale_effects: 0, reconcile_p95_ms: 300, approved_digests_restored: true });
  assert.equal(assertions["CELLD.011.REFUSAL"].refused, true);
  assert.equal(assertions["CELLD.011.REFUSAL"].node_mutations, 0);
  assert.equal(assertions["CELLD.011.REFUSAL"].inventory_sha256_before, assertions["CELLD.011.REFUSAL"].inventory_sha256_after);
  assert.equal(result.mutation_count, 11);
  assert.equal(fixture.events.filter((event) => event.type === "intent").length, fixture.events.filter((event) => event.type === "mutation").length);
  assert.ok(result.timeline.some((entry) => entry.phase === "rollback_threshold_breached"));
});

test("authorization, compatibility, and budget failures happen before adapter access", async () => {
  for (const unsafe of [
    plan({ authorization: { destructive_faults: false } }),
    plan({ candidate: { ...candidate, compatible_from: [] } }),
    plan({ max_unavailable: 0 }),
    plan({ candidate: { ...candidate, version: previous.version } }),
  ]) {
    let touched = false;
    const fixture = new Proxy({}, { get: () => { touched = true; return undefined; } });
    await assert.rejects(executeRolloutController(unsafe, fixture));
    assert.equal(touched, false);
  }
});

test("reserve consumption and unavailable-budget violations stop the next mutation", async () => {
  const fixture = adapter();
  const originalDrain = fixture.drainNode;
  fixture.drainNode = async (name, timeout) => {
    await originalDrain(name, timeout);
    if (name === "celld-node-1") {
      const originalObserve = fixture.observeFleet;
      fixture.observeFleet = async () => {
        const observed = await originalObserve();
        const reserve = observed.nodes.find((node) => node.role === "reserve");
        Object.assign(reserve, { running: false, ready: false, member: false });
        return observed;
      };
    }
  };
  await assert.rejects(executeRolloutController(plan(), fixture), /max_unavailable|reserve/);
  assert.deepEqual(fixture.events.filter((event) => event.type === "mutation").map((event) => event.kind), ["drain", "rollback"]);
});

test("an aborted campaign restores every changed node through persisted emergency intents", async () => {
  const fixture = adapter();
  fixture.observeSafety = async () => ({
    acknowledged_intent_ids: ["intent-1"], rebuilt_intent_ids: ["intent-1"], effects: [], current_generations: {},
    reconcile_samples_ms: [100], error_rate: 0, baseline_coordination_p99_ms: 100, coordination_p99_ms: 100,
  });
  await assert.rejects(executeRolloutController(plan(), fixture), /rollback threshold was not breached/);
  const final = await fixture.observeFleet();
  assert.ok(final.nodes.every((node) => node.running && node.ready && node.member && node.manifest_digest === previous.manifest_digest));
  const emergency = fixture.events.filter((event) => event.type === "intent" && event.intent.kind === "emergency_rollback");
  assert.equal(emergency.length, 2);
});

test("malformed lifecycle observations are evidence errors and still trigger emergency rollback", async () => {
  const fixture = adapter();
  fixture.observeSafety = async () => ({
    acknowledged_intent_ids: ["intent-1", "intent-1"], rebuilt_intent_ids: ["intent-1"], effects: [], current_generations: {},
    reconcile_samples_ms: [100], error_rate: 0.02, baseline_coordination_p99_ms: 100, coordination_p99_ms: 100,
  });
  await assert.rejects(executeRolloutController(plan(), fixture), /intent observations are invalid/);
  const final = await fixture.observeFleet();
  assert.ok(final.nodes.every((node) => node.manifest_digest === previous.manifest_digest));
});

test("refusal evidence is canonical and rejects a compatible control pair", () => {
  const inventory = { nodes: plan().nodes, generation: 7 };
  const refused = incompatiblePairRefusal(inventory, { previous, candidate: { ...candidate, compatible_from: [] } });
  assert.equal(refused.refused, true);
  assert.equal(refused.node_mutations, 0);
  assert.equal(refused.inventory_sha256_before, refused.inventory_sha256_after);
  assert.throws(() => incompatiblePairRefusal(inventory, { previous, candidate }), /must use an incompatible/);
  const changed = incompatiblePairRefusal(inventory, { previous, candidate: { ...candidate, compatible_from: [] } }, { ...inventory, generation: 8 });
  assert.notEqual(changed.inventory_sha256_before, changed.inventory_sha256_after);
});
