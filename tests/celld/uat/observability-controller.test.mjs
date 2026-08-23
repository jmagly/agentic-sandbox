import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";

import {
  OBSERVABILITY_BOUNDARIES,
  OBSERVABILITY_REPAIR_SURFACES,
  OBSERVABILITY_SURFACES,
  ObservabilityCleanupError,
  executeObservabilityCampaign,
} from "../../../scripts/celld-observability-controller.mjs";
import { SAFE_LIVE_EVALUATORS } from "../../../scripts/celld-live-evaluators.mjs";

function canonicalJson(value) {
  if (value === null || typeof value === "boolean" || typeof value === "string") return JSON.stringify(value);
  if (typeof value === "number" && Number.isFinite(value)) return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
}

function sha256(value) { return createHash("sha256").update(value).digest("hex"); }

function fixtureAdapter(options = {}) {
  const calls = [];
  const active = new Set();
  const baseline = "a".repeat(64);
  const indexFor = (boundary) => OBSERVABILITY_BOUNDARIES.indexOf(boundary);
  const identities = (boundary) => {
    const index = indexFor(boundary) + 1;
    return {
      fleet_id: "fleet-test",
      instance_id: `instance-${index}`,
      generation: index,
      operation_id: `operation-${index}`,
      trace_id: index.toString(16).padStart(32, "0"),
      celld_version: "v0.3.0",
      adapter_version: "2026.8.3",
      node_id: `node-${index % 3}`,
    };
  };
  const timing = (boundary) => {
    const index = indexFor(boundary);
    const injected = 1_000_000 + index * 1_000_000;
    const evaluation = 60_000;
    const retry = 60_000;
    const delay = boundary === "divergence" ? 300_000 : boundary === "unknown_effect" ? retry * 2 : boundary === "stale_generation" ? evaluation : 1_000;
    return { injected, detected: injected + delay, healed: injected + delay + 1_000, resolved: injected + delay + 2_000, evaluation, retry };
  };
  const adapter = {
    calls,
    active,
    captureBaseline: async () => ({ baseline_sha256: baseline }),
    persistIntent: async (intent) => {
      calls.push(`persist:${intent.action}:${intent.boundary}`);
      return { intent_sha256: sha256(canonicalJson(intent)), persisted: true };
    },
    injectFault: async (boundary) => {
      calls.push(`inject:${boundary}`);
      active.add(boundary);
    },
    observeFault: async (boundary) => ({ boundary, injection_applied: true, injection_verified: true, injected_at_ms: timing(boundary).injected, identities: identities(boundary) }),
    collectSurface: async (boundary, surface) => ({ boundary, surface, classification: boundary, identities: identities(boundary) }),
    collectRepairPlan: async (boundary, surface) => ({ boundary, surface, representation: "plan", effect_claimed: false }),
    observeAlertDetection: async (boundary) => ({ boundary, detected_at_ms: timing(boundary).detected, evaluation_interval_ms: timing(boundary).evaluation, retry_interval_ms: timing(boundary).retry }),
    healFault: async (boundary) => {
      calls.push(`heal:${boundary}`);
      active.delete(boundary);
    },
    observeHeal: async (boundary) => ({ boundary, healed: true, heal_verified: true, healed_at_ms: timing(boundary).healed }),
    observeAlertResolution: async (boundary) => ({ boundary, resolved_at_ms: timing(boundary).resolved }),
    scanRedaction: async () => ({ surfaces_scanned: [...OBSERVABILITY_SURFACES], artifacts_scanned: OBSERVABILITY_BOUNDARIES.length * OBSERVABILITY_SURFACES.length, secret_findings: 0 }),
    verifyBaseline: async () => ({ baseline_sha256: baseline, restored: active.size === 0 }),
  };
  Object.assign(adapter, options);
  return adapter;
}

test("observability controller derives exact promoting matrices accepted by trusted evaluators", async () => {
  const adapter = fixtureAdapter();
  const campaign = await executeObservabilityCampaign({ runId: "test-run", adapter });
  assert.equal(campaign.cases.length, 10);
  assert.equal(campaign.records.length, 70);
  assert.equal(campaign.alerts.length, 10);
  assert.equal(campaign.timeline.filter((entry) => entry.phase === "intent_persisted").length, 20);
  assert.deepEqual(campaign.cleanup, { status: "passed", active_faults: 0, baseline_restored: true });
  assert.equal(SAFE_LIVE_EVALUATORS["CELLD.014.CLASSIFICATION"]({ cases: campaign.cases }).passed, true);
  assert.equal(SAFE_LIVE_EVALUATORS["CELLD.014.CORRELATION"]({ records: campaign.records, redaction: campaign.redaction, evidence_exported: true, fleet_baseline_restored: campaign.baseline.restored }).passed, true);
  assert.equal(SAFE_LIVE_EVALUATORS["CELLD.014.ALERTS"]({ alerts: campaign.alerts }).passed, true);
});

test("every observability mutation follows its durably acknowledged intent", async () => {
  const adapter = fixtureAdapter();
  await executeObservabilityCampaign({ runId: "test-run", adapter });
  for (const boundary of OBSERVABILITY_BOUNDARIES) {
    assert.ok(adapter.calls.indexOf(`persist:inject:${boundary}`) < adapter.calls.indexOf(`inject:${boundary}`));
    assert.ok(adapter.calls.indexOf(`persist:heal:${boundary}`) < adapter.calls.indexOf(`heal:${boundary}`));
  }
});

test("unknown adapter evidence fields are rejected before artifact construction", async () => {
  const base = fixtureAdapter();
  const adapter = fixtureAdapter({
    collectSurface: async (boundary, surface) => ({ ...(await base.collectSurface(boundary, surface)), authorization_header: "redacted" }),
  });
  await assert.rejects(executeObservabilityCampaign({ runId: "test-run", adapter }), /unknown fields: authorization_header/);
  assert.equal(adapter.active.size, 0);
});

test("cross-surface identity disagreement is rejected and healed", async () => {
  const base = fixtureAdapter();
  const adapter = fixtureAdapter({
    collectSurface: async (boundary, surface) => {
      const capture = await base.collectSurface(boundary, surface);
      if (boundary === "celld" && surface === "dashboard") capture.identities.operation_id = "wrong-operation";
      return capture;
    },
  });
  await assert.rejects(executeObservabilityCampaign({ runId: "test-run", adapter }), /did not agree/);
  assert.equal(adapter.active.size, 0);
  assert.ok(adapter.calls.includes("persist:emergency_heal:celld"));
});

test("operator surfaces cannot claim repair effects", async () => {
  const adapter = fixtureAdapter({
    collectRepairPlan: async (boundary, surface) => ({ boundary, surface, representation: "plan", effect_claimed: surface === OBSERVABILITY_REPAIR_SURFACES[0] }),
  });
  await assert.rejects(executeObservabilityCampaign({ runId: "test-run", adapter }), /not an honest plan/);
  assert.equal(adapter.active.size, 0);
});

test("an unverified normal heal remains eligible for emergency healing", async () => {
  const adapter = fixtureAdapter();
  let observations = 0;
  adapter.observeHeal = async (boundary) => {
    observations += 1;
    const healed = observations > 1;
    return { boundary, healed, heal_verified: healed, healed_at_ms: 1_010_000 };
  };
  await assert.rejects(executeObservabilityCampaign({ runId: "test-run", adapter }), /heal was not independently verified/);
  assert.ok(adapter.calls.includes("persist:emergency_heal:celld"));
  assert.equal(adapter.active.size, 0);
});

test("late bounded alerts fail the controller and trigger healing", async () => {
  const adapter = fixtureAdapter({
    observeAlertDetection: async (boundary) => ({ boundary, detected_at_ms: boundary === "divergence" ? 1_301_000 : 1_001_000, evaluation_interval_ms: 60_000, retry_interval_ms: 60_000 }),
  });
  await assert.rejects(executeObservabilityCampaign({ runId: "test-run", adapter }), /timing bound/);
  assert.equal(adapter.active.size, 0);
});

test("cleanup failure outranks an operation failure", async () => {
  const base = fixtureAdapter();
  const adapter = fixtureAdapter({
    collectSurface: async (boundary, surface) => {
      const capture = await base.collectSurface(boundary, surface);
      if (boundary === "celld" && surface === "api") capture.classification = "management";
      return capture;
    },
    healFault: async (boundary) => { adapter.calls.push(`heal:${boundary}`); throw new Error("heal controller unavailable"); },
    verifyBaseline: async () => ({ baseline_sha256: "a".repeat(64), restored: false }),
  });
  await assert.rejects(
    executeObservabilityCampaign({ runId: "test-run", adapter }),
    (error) => error instanceof ObservabilityCleanupError && /heal controller unavailable/.test(error.message) && /baseline/.test(error.message),
  );
});

test("adapter contract is exact and mandatory", async () => {
  const adapter = fixtureAdapter();
  delete adapter.scanRedaction;
  await assert.rejects(executeObservabilityCampaign({ runId: "test-run", adapter }), /adapter\.scanRedaction is required/);
});
