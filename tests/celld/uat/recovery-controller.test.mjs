import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";

import {
  RECOVERY_EVIDENCE_KINDS,
  RECOVERY_RUNBOOKS,
  RecoveryCleanupError,
  executeRecoveryCampaign,
} from "../../../scripts/celld-recovery-controller.mjs";
import { SAFE_LIVE_EVALUATORS } from "../../../scripts/celld-live-evaluators.mjs";

function canonicalJson(value) {
  if (value === null || typeof value === "boolean" || typeof value === "string") return JSON.stringify(value);
  if (typeof value === "number" && Number.isFinite(value)) return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
}

function digest(value) { return createHash("sha256").update(value).digest("hex"); }

function fixtureAdapter(overrides = {}) {
  const calls = [];
  const baseline = "a".repeat(64);
  const state = { writersStopped: false, authority: false, fleetUnavailable: false, restores: new Set(), snapshots: new Set() };
  const snapshot = (execution) => ({
    execution,
    snapshot_version_id: `snapshot-version-${execution}`,
    source_prefix: "fleet/source",
    latest_acknowledged_at_ms: execution * 10_000,
    snapshot_captured_at_ms: execution * 10_000 + 300_000,
    generation_manifest_sha256: digest(`generation-${execution}`),
    tombstone_manifest_sha256: digest(`tombstone-${execution}`),
  });
  const adapter = {
    calls,
    state,
    captureBaseline: async () => ({ baseline_sha256: baseline }),
    observeStores: async () => ({ affected_fleet_store_id: "affected-store", external_evidence_store_id: "external-store" }),
    persistIntent: async (intent) => {
      calls.push(`persist:${intent.action}:${intent.target}`);
      return { intent_sha256: digest(canonicalJson(intent)), persisted: true };
    },
    stopSourceWriters: async () => { calls.push("stop-writers"); state.writersStopped = true; },
    observeSourceWriters: async () => ({ stopped: state.writersStopped }),
    acquireRestoreAuthority: async () => { calls.push("acquire-authority"); state.authority = true; },
    observeRestoreAuthority: async () => ({ exclusive: state.authority, authority_id: state.authority ? "restore-controller" : null }),
    createSnapshot: async (execution) => { calls.push(`snapshot:${execution}`); state.snapshots.add(execution); },
    observeSnapshot: async (execution) => snapshot(execution),
    restoreSnapshot: async (execution) => { calls.push(`restore:${execution}`); state.restores.add(execution); },
    observeRestore: async (execution) => ({
      execution,
      snapshot_version_id: `snapshot-version-${execution}`,
      source_prefix: "fleet/source",
      restore_prefix: `fleet/isolated-restore-${execution}`,
      restore_started_at_ms: execution * 10_000 + 301_000,
      restore_ready_at_ms: execution * 10_000 + 1_801_000,
      isolated_restore: true,
      quarantined: true,
      source_writers_stopped: state.writersStopped,
      restore_authority_exclusive: state.authority,
      generation_manifest_sha256: digest(`generation-${execution}`),
      tombstone_manifest_sha256: digest(`tombstone-${execution}`),
    }),
    releaseRestoreAuthority: async () => { calls.push("release-authority"); state.authority = false; },
    startSourceWriters: async () => { calls.push("start-writers"); state.writersStopped = false; },
    executeRunbook: async (runbook, ordinal) => { calls.push(`runbook:${runbook}:${ordinal}`); },
    observeRunbook: async (runbook, ordinal) => ({ runbook, ordinal, operation_ids: [`operation-${runbook}`], lifecycle_effect_ids: [`effect-${runbook}`], state_sha256_after: digest(`state-${runbook}`), healed: true, cleanup_verified: true }),
    cleanupRunbook: async (runbook) => ({ runbook, healed: true, cleanup_verified: true }),
    uploadEvidence: async (kind) => { calls.push(`upload:${kind}`); },
    observeEvidenceUpload: async (kind) => ({ kind, storage_authority_id: "external-store", bytes: 1_024, sha256: digest(`evidence-${kind}`), retained: true }),
    makeAffectedFleetUnavailable: async () => { calls.push("lose-fleet"); state.fleetUnavailable = true; },
    observeFleetAvailability: async () => ({ affected_fleet_unavailable: state.fleetUnavailable, external_evidence_store_reachable: true }),
    readExternalEvidence: async (kind) => ({ kind, downloaded_sha256: digest(`evidence-${kind}`), read_after_fleet_loss: state.fleetUnavailable }),
    probeEvidenceCorruption: async (kind) => ({ kind, tampered_sha256: digest(`tampered-${kind}`), detected: true }),
    verifyEvidenceManifest: async () => ({ manifest_verified: true, malicious_runner_tamper_proof_claimed: false }),
    restoreAffectedFleet: async () => { calls.push("restore-fleet"); state.fleetUnavailable = false; },
    cleanupRestore: async (execution) => { calls.push(`cleanup-restore:${execution}`); state.restores.delete(execution); },
    observeRestoreCleanup: async (execution) => ({ execution, removed: !state.restores.has(execution) }),
    cleanupSnapshot: async (execution) => { calls.push(`cleanup-snapshot:${execution}`); state.snapshots.delete(execution); },
    observeSnapshotCleanup: async (execution) => ({ execution, removed: !state.snapshots.has(execution) }),
    verifyBaseline: async () => ({ baseline_sha256: baseline, restored: !state.writersStopped && !state.authority && !state.fleetUnavailable && state.restores.size === 0 && state.snapshots.size === 0 }),
  };
  Object.assign(adapter, overrides);
  return adapter;
}

test("recovery controller derives two restores, repeated runbooks, and independent evidence accepted by trusted evaluators", async () => {
  const adapter = fixtureAdapter();
  const campaign = await executeRecoveryCampaign({ runId: "test-run", adapter });
  assert.equal(campaign.restores.length, 2);
  assert.equal(campaign.runbooks.length, 5);
  assert.equal(campaign.evidence.artifacts.length, 4);
  assert.equal(campaign.timeline.filter((entry) => entry.phase === "intent_persisted").length, 28);
  assert.equal(SAFE_LIVE_EVALUATORS["CELLD.015.OBJECTIVES"]({ restores: campaign.restores }).passed, true);
  assert.equal(SAFE_LIVE_EVALUATORS["CELLD.015.IDEMPOTENT"]({ runbooks: campaign.runbooks }).passed, true);
  assert.equal(SAFE_LIVE_EVALUATORS["CELLD.015.EVIDENCE"](campaign.evidence).passed, true);
});

test("every recovery mutation follows a durably acknowledged intent", async () => {
  const adapter = fixtureAdapter();
  await executeRecoveryCampaign({ runId: "test-run", adapter });
  const pairs = [
    ["persist:stop_source_writers:source", "stop-writers"],
    ["persist:acquire_restore_authority:source", "acquire-authority"],
    ["persist:create_versioned_snapshot:snapshot:1", "snapshot:1"],
    ["persist:restore_snapshot:restore:1", "restore:1"],
    ["persist:make_affected_fleet_unavailable:affected_fleet", "lose-fleet"],
    ["persist:restore_affected_fleet:affected_fleet", "restore-fleet"],
  ];
  for (const [intent, mutation] of pairs) assert.ok(adapter.calls.indexOf(intent) < adapter.calls.indexOf(mutation), `${intent} precedes ${mutation}`);
});

test("the external evidence authority must be distinct before mutation", async () => {
  const adapter = fixtureAdapter({ observeStores: async () => ({ affected_fleet_store_id: "same-store", external_evidence_store_id: "same-store" }) });
  await assert.rejects(executeRecoveryCampaign({ runId: "test-run", adapter }), /must be independent/);
  assert.equal(adapter.calls.length, 0);
});

test("an RPO violation fails closed and restores writers and authority", async () => {
  const adapter = fixtureAdapter({
    observeSnapshot: async (execution) => ({
      execution, snapshot_version_id: `snapshot-version-${execution}`, source_prefix: "fleet/source", latest_acknowledged_at_ms: 1_000,
      snapshot_captured_at_ms: 301_001, generation_manifest_sha256: digest(`generation-${execution}`), tombstone_manifest_sha256: digest(`tombstone-${execution}`),
    }),
  });
  await assert.rejects(executeRecoveryCampaign({ runId: "test-run", adapter }), /RPO objective/);
  assert.equal(adapter.state.writersStopped, false);
  assert.equal(adapter.state.authority, false);
  assert.equal(adapter.state.snapshots.size, 0);
});

test("the second runbook execution cannot introduce a new lifecycle effect", async () => {
  const adapter = fixtureAdapter({
    observeRunbook: async (runbook, ordinal) => ({ runbook, ordinal, operation_ids: [`operation-${runbook}`], lifecycle_effect_ids: [`effect-${runbook}`, ...(runbook === "node_loss" && ordinal === 2 ? ["new-effect"] : [])], state_sha256_after: digest(`state-${runbook}`), healed: true, cleanup_verified: true }),
  });
  await assert.rejects(executeRecoveryCampaign({ runId: "test-run", adapter }), /additional or divergent effects/);
  assert.equal(adapter.state.restores.size, 0);
  assert.equal(adapter.state.snapshots.size, 0);
});

test("unreadable external evidence restores the affected fleet and removes restore fixtures", async () => {
  const adapter = fixtureAdapter({ readExternalEvidence: async (kind) => ({ kind, downloaded_sha256: digest(`wrong-${kind}`), read_after_fleet_loss: true }) });
  await assert.rejects(executeRecoveryCampaign({ runId: "test-run", adapter }), /not independently readable/);
  assert.equal(adapter.state.fleetUnavailable, false);
  assert.equal(adapter.state.restores.size, 0);
  assert.equal(adapter.state.snapshots.size, 0);
  assert.ok(adapter.calls.includes("persist:emergency_restore_affected_fleet:affected_fleet"));
});

test("unknown adapter evidence fields are rejected before evidence export", async () => {
  const adapter = fixtureAdapter({
    observeEvidenceUpload: async (kind) => ({ kind, storage_authority_id: "external-store", bytes: 1_024, sha256: digest(`evidence-${kind}`), retained: true, bearer_token: "redacted" }),
  });
  await assert.rejects(executeRecoveryCampaign({ runId: "test-run", adapter }), /unknown fields: bearer_token/);
  assert.equal(adapter.state.restores.size, 0);
});

test("cleanup failure outranks an operation failure", async () => {
  const adapter = fixtureAdapter({
    observeSnapshot: async (execution) => ({ execution, snapshot_version_id: `snapshot-version-${execution}`, source_prefix: "fleet/source", latest_acknowledged_at_ms: 1_000, snapshot_captured_at_ms: 301_001, generation_manifest_sha256: digest(`generation-${execution}`), tombstone_manifest_sha256: digest(`tombstone-${execution}`) }),
    releaseRestoreAuthority: async () => { throw new Error("authority release unavailable"); },
  });
  await assert.rejects(
    executeRecoveryCampaign({ runId: "test-run", adapter }),
    (error) => error instanceof RecoveryCleanupError && /authority release unavailable/.test(error.message) && /baseline/.test(error.message),
  );
});

test("adapter contract requires every recovery authority boundary", async () => {
  const adapter = fixtureAdapter();
  delete adapter.probeEvidenceCorruption;
  await assert.rejects(executeRecoveryCampaign({ runId: "test-run", adapter }), /adapter\.probeEvidenceCorruption is required/);
});

test("controller constants retain the exact five runbooks and four evidence kinds", () => {
  assert.deepEqual([...RECOVERY_RUNBOOKS], ["node_loss", "full_restart", "authorization_loss", "snapshot_restore", "credential_rotation"]);
  assert.deepEqual([...RECOVERY_EVIDENCE_KINDS], ["snapshot_identity", "restore_timeline", "generation_comparison", "evidence_manifest"]);
});
