import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { SAFE_LIVE_EVALUATORS, WITHHELD_LIVE_EVALUATOR_IDS } from "../../../scripts/celld-live-evaluators.mjs";

const hash = (letter) => `sha256:${letter.repeat(64)}`;
const lifecycleActions = ["provision", "start", "stop", "destroy"];
const substrates = ["qemu", "docker"];
const crashPoints = ["before_dispatch", "during_dispatch", "after_dispatch"];
const excluded = ["process", "pty", "workspace", "filesystem", "raw_network", "vm", "container", "host_api"];
const advertised = ["fetch", "rpc", "storage", "alarm", "websocket", "outbound_https", "wasm", "assets"];
const limits = ["cpu", "memory", "request_rate", "storage", "resident_cells", "outbound"];
const denials = ["forged_body", "forged_mac", "stale_timestamp", "nonce_replay", "wrong_key", "zero_generation", "wrong_generation", "public_or_cross_fleet"];
const boundaries = ["management", "celld_owner", "node", "object_store", "provider", "network", "credential", "version"];

const passing = {
  "CELLD.003.ONE_EFFECT": { repeats_per_action: 10_000, actions: lifecycleActions, substrates, operation_ids: 8, provider_effects: 8, max_effects_per_operation: 1, duplicate_effects: 0 },
  "CELLD.003.COLLISION": { collision_attempts: 8, rejected: 8, provider_effects_before: 8, provider_effects_after: 8 },
  "CELLD.004.NO_LOSS": { trials_per_crash_point: 100, crash_points: crashPoints, substrates, acknowledged: 600, survived: 600, lost: 0 },
  "CELLD.004.RECOVERY": { samples: 600, p95_ms: 30_000, duplicate_effects: 0, components_healthy: true, inventory_restored: true },
  "CELLD.005.ORIGINAL_ID": { trials_per_action: 100, actions: lifecycleActions, substrates, lookups: 800, original_id_matches: 800, replacement_ids: 0 },
  "CELLD.005.NO_SECOND_EFFECT": { trials: 800, second_effects: 0, p95_ms: 30_000, proxy_healed: true },
  "CELLD.006.PRE_PROVIDER": { trials_per_action: 100, actions: ["stop", "destroy"], substrates, attempts: 400, rejected_before_provider: 400, provider_effects: 0 },
  "CELLD.006.ACTIVE_SAFE": { stale_attempts: 400, future_attempts: 2, active_generation_changes: 0, active_checksum_unchanged: true, partition_healed: true },
  "CELLD.007.CLAIMS": { capabilities: advertised, advertised_cases: 8, passed_cases: 8, failed_cases: 0, not_run_cases: 0 },
  "CELLD.007.ROLLBACK": { previous_digest: hash("a"), restored_digest: hash("a"), state_sha256_before: hash("b"), state_sha256_after: hash("b"), approved_digest_active: true },
  "CELLD.008.LOUD_REJECTION": { capabilities: excluded, attempts_per_capability: 100, attempts: 800, typed_rejections: 800, silent_successes: 0 },
  "CELLD.008.NO_SIDE_EFFECT": { attempts: 800, processes_created: 0, files_created: 0, sockets_created: 0, containers_created: 0, vms_created: 0, host_inventory_restored: true },
  "CELLD.009.CONTAINMENT": { limit_families: limits, families_enforced: 6, max_enforcement_ms: 5_000, node_crashes: 0, offender_removed: true },
  "CELLD.009.NEIGHBOR": { adjacent_attempts: 10_000, adjacent_successes: 9_900, fleet_healthy: true },
  "CELLD.010.ISOLATION": { classes: ["public_internal", "cross_fleet", "cross_bucket"], forbidden_attempts: 3_000, denied: 3_000, succeeded: 0, provider_effects: 0, routes_healed: true },
  "CELLD.011.BUDGET": { nodes_expected: 3, max_unavailable_observed: 1, reserve_consumed: 0, membership_healthy: true },
  "CELLD.011.SAFETY": { lost_intents: 0, duplicate_effects: 0, stale_effects: 0, reconcile_p95_ms: 30_000, approved_digests_restored: true },
  "CELLD.011.REFUSAL": { refused: true, node_mutations: 0, inventory_sha256_before: hash("c"), inventory_sha256_after: hash("c") },
  "CELLD.012.DENIAL": { classes: denials, attempts_per_class: 1_000, attempts: 8_000, denied: 8_000, provider_effects: 0 },
  "CELLD.012.VALID": { attempts: 1, successes: 1, correlated: true, signature_value_absent: true, identity_removed: true },
  "CELLD.014.CLASSIFICATION": { boundaries, injected: 8, correctly_classified: 8, cross_surface_disagreements: 0, faults_healed: true },
  "CELLD.014.CORRELATION": { applicable_records: 80, records_with_all_required_ids: 80, secret_findings: 0, evidence_exported: true },
  "CELLD.014.ALERTS": { divergence_detect_ms: 300_000, unknown_detect_intervals: 2, stale_detect_intervals: 1, all_alerts_resolved: true },
  "CELLD.015.OBJECTIVES": { restore_executions: 2, rpo_seconds: 300, rto_seconds: 1_800, latest_acknowledged_state_present: true, tombstones_present: true },
  "CELLD.015.IDEMPOTENT": { runbook_executions: 2, first_execution_effects: 1, second_execution_additional_effects: 0, restore_state_unchanged: true },
  "CELLD.015.EVIDENCE": { external_artifacts: 4, readable_after_fleet_loss: 4, hashes_verified: 4, corruption_cases_detected: 1, retention_confirmed: true },
};

test("every authorized non-storage live assertion has one trusted evaluator", () => {
  const catalog = JSON.parse(readFileSync(new URL("./scenarios.json", import.meta.url), "utf8"));
  const assigned = catalog.scenarios.flatMap((scenario) => (scenario.execution.live_drivers ?? []).flatMap((driver) => driver.covers_assertions));
  const expected = assigned.filter((id) => id !== "CELLD.010.STORAGE" && !WITHHELD_LIVE_EVALUATOR_IDS.includes(id)).sort();
  const actual = Object.keys(SAFE_LIVE_EVALUATORS).filter((id) => id !== "CELLD.010.STORAGE").sort();
  assert.deepEqual(actual, expected);
  assert.deepEqual([...WITHHELD_LIVE_EVALUATOR_IDS].sort(), ["CELLD.013.NO_LEAK", "CELLD.013.PROVENANCE", "CELLD.013.SCOPE"]);
});

test("trusted formulas pass exact boundary measurements", () => {
  assert.deepEqual(Object.keys(passing).sort(), Object.keys(SAFE_LIVE_EVALUATORS).filter((id) => id !== "CELLD.010.STORAGE").sort());
  for (const [id, measurements] of Object.entries(passing)) {
    const result = SAFE_LIVE_EVALUATORS[id](structuredClone(measurements));
    assert.equal(result.passed, true, id);
    assert.equal(result.reason.length > 0, true, id);
  }
});

test("trusted formulas reject threshold, uniqueness, isolation, and recovery violations", () => {
  const cases = [
    ["CELLD.003.ONE_EFFECT", { duplicate_effects: 1 }],
    ["CELLD.004.RECOVERY", { p95_ms: 30_001 }],
    ["CELLD.006.PRE_PROVIDER", { provider_effects: 1 }],
    ["CELLD.007.CLAIMS", { not_run_cases: 1, passed_cases: 7 }],
    ["CELLD.008.NO_SIDE_EFFECT", { sockets_created: 1 }],
    ["CELLD.009.NEIGHBOR", { adjacent_successes: 9_899 }],
    ["CELLD.010.ISOLATION", { denied: 2_999, succeeded: 1 }],
    ["CELLD.011.BUDGET", { max_unavailable_observed: 2 }],
    ["CELLD.012.DENIAL", { provider_effects: 1 }],
    ["CELLD.014.ALERTS", { unknown_detect_intervals: 3 }],
    ["CELLD.015.OBJECTIVES", { rto_seconds: 1_801 }],
  ];
  for (const [id, changes] of cases) {
    assert.equal(SAFE_LIVE_EVALUATORS[id]({ ...structuredClone(passing[id]), ...changes }).passed, false, id);
  }
});

test("malformed measurements are evidence errors, not product failures", () => {
  assert.throws(() => SAFE_LIVE_EVALUATORS["CELLD.003.ONE_EFFECT"]({ repeats_per_action: "10000" }), /integer/);
  assert.throws(() => SAFE_LIVE_EVALUATORS["CELLD.012.VALID"](null), /must be an object/);
  assert.throws(() => SAFE_LIVE_EVALUATORS["CELLD.012.VALID"]({ ...passing["CELLD.012.VALID"], verdict: "PASS" }), /forbidden self-declared verdict/);
});
