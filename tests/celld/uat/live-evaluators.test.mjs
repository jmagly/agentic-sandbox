import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { CANDIDATE_LIVE_EVALUATORS, SAFE_LIVE_EVALUATORS, WITHHELD_LIVE_EVALUATOR_IDS } from "../../../scripts/celld-live-evaluators.mjs";

const TEST_LIVE_EVALUATORS = Object.freeze({ ...SAFE_LIVE_EVALUATORS, ...CANDIDATE_LIVE_EVALUATORS });

const hash = (letter) => `sha256:${letter.repeat(64)}`;
const digest = (letter) => letter.repeat(64);
const lifecycleActions = ["provision", "start", "stop", "destroy"];
const substrates = ["qemu", "docker"];
const crashPoints = ["before_dispatch", "during_dispatch", "after_dispatch"];
const excluded = ["process", "pty", "workspace", "filesystem", "raw_network", "vm", "container", "host_api"];
const advertised = ["fetch", "rpc", "storage", "alarm", "websocket", "outbound_https", "wasm", "assets"];
const limits = ["cpu", "memory", "request_rate", "storage", "resident_cells", "outbound"];
const denials = ["forged_body", "forged_mac", "stale_timestamp", "nonce_replay", "wrong_key", "zero_generation", "wrong_generation", "public_or_cross_fleet"];
const credentialKinds = ["s3_access_identity", "request_hmac", "mtls_identity", "celld_peer_secret", "fixture_administrator"];
const credentialScanSurfaces = ["argv", "captured_env", "shell_trace", "logs", "crash_artifacts", "persistent_scratch", "support_evidence"];
const credentialActivationMethods = {
  s3_access_identity: "controlled_restart",
  request_hmac: "dual_key_overlap",
  mtls_identity: "controlled_restart",
  celld_peer_secret: "controlled_restart",
  fixture_administrator: "fixture_controller_only",
};
const credentialRoles = {
  s3_access_identity: { owner: "fixture_controller", consumer: "celld_fleet" },
  request_hmac: { owner: "management_adapter", consumer: "management_and_worker" },
  mtls_identity: { owner: "project_ca", consumer: "management_and_callback_relay" },
  celld_peer_secret: { owner: "celld_store_authority", consumer: "celld_fleet" },
  fixture_administrator: { owner: "fixture_controller", consumer: "fixture_controller_only" },
};
const provenanceMismatchFields = ["version", "commit", "digest", "signature"];
const boundaries = ["celld", "management", "store_latency", "store_authorization", "store_condition", "provider", "divergence", "unknown_effect", "stale_generation", "below_reserve"];
const surfaces = ["cli", "api", "dashboard", "logs", "traces", "metrics", "alert_evaluator"];
const repairSurfaces = ["cli", "api", "dashboard"];

const matrixCases = (dimensions, build) => {
  const entries = [{}];
  for (const [field, values] of Object.entries(dimensions)) {
    const current = entries.splice(0);
    for (const entry of current) for (const value of values) entries.push({ ...entry, [field]: value });
  }
  return entries.map((entry, index) => build(entry, index));
};
const uniqueDigest = (index) => (index + 1).toString(16).padStart(64, "0");

const replayCases = matrixCases({ substrate: substrates, action: lifecycleActions }, (entry, index) => ({
  ...entry,
  operation_id_sha256: uniqueDigest(index),
  management_operation_id_sha256: uniqueDigest(index + 16),
  replay_count: 10_000,
  replay_http_200: 10_000,
  replay_management_operation_matches: 10_000,
  replay_terminal_status_matches: 10_000,
  effect_records: 1,
  provider_effect_count: 1,
}));
const collisionCases = replayCases.map(({ substrate, action, operation_id_sha256 }) => ({
  substrate,
  action,
  operation_id_sha256,
  response_status: 409,
  response_code: "celld.operation_collision",
  effect_records_before: 1,
  effect_records_after: 1,
  provider_effects_before: 1,
  provider_effects_after: 1,
}));
const restartCases = matrixCases({ substrate: substrates, crash_point: crashPoints, trial: Array.from({ length: 100 }, (_, index) => index + 1) }, (entry, index) => ({
  ...entry,
  operation_id_sha256: uniqueDigest(index),
  acknowledged: true,
  terminal_status: "succeeded",
  effect_records: 1,
  recovery_ms: 30_000,
}));
const restartProviderCases = substrates.map((substrate, index) => ({
  substrate,
  provider_checksum_before: uniqueDigest(index + 1_000),
  provider_checksum_after: uniqueDigest(index + 1_000),
}));
const responseLossCases = matrixCases({ substrate: substrates, action: lifecycleActions, trial: Array.from({ length: 100 }, (_, index) => index + 1) }, (entry, index) => ({
  ...entry,
  operation_id_sha256: uniqueDigest(index),
  original_id_match: true,
  replacement_id_observed: false,
  effect_records: 1,
  attempts: 3,
  unknown_observed: true,
  convergence_ms: 30_000,
}));
const staleCases = matrixCases({ substrate: substrates, action: ["stop", "destroy"], trial: Array.from({ length: 100 }, (_, index) => index + 1) }, (entry, index) => ({
  ...entry,
  operation_id_sha256: uniqueDigest(index),
  response_status: 409,
  response_code: "celld.stale_generation_fenced",
  provider_effects: 0,
}));
const activeGenerationCases = substrates.map((substrate, index) => ({
  substrate,
  future_response_status: 409,
  future_response_code: "cell.generation_fenced",
  active_checksum_before: uniqueDigest(index),
  active_checksum_after: uniqueDigest(index),
  partition_applied: true,
  partition_healed: true,
  baseline_after_heal_succeeded: true,
}));

const protectedCredentials = credentialKinds.map((secretKind, index) => ({
  secret_kind: secretKind,
  credential_id_sha256: uniqueDigest(index + 2_000),
  credential_ref_sha256: uniqueDigest(index + 2_100),
  canary_id_sha256: uniqueDigest(index + 2_200),
  delivery: "protected_tmpfs_file",
  mode_octal: "0600",
  regular_file: true,
  symlink: false,
  owner_only: true,
  canary_matches_in_reference: 1,
  canary_matches_outside_reference: 0,
  revoked_or_removed: true,
}));
const credentialScans = credentialScanSurfaces.map((surface, index) => ({
  surface,
  expected_artifact_count: 2,
  artifacts_scanned: 2,
  expected_inventory_sha256: uniqueDigest(index + 2_300),
  scanned_inventory_sha256: uniqueDigest(index + 2_300),
  canaries_expected: credentialKinds.length,
  canaries_scanned: credentialKinds.length,
  canary_matches: 0,
}));
const credentialLifecycles = credentialKinds.map((secretKind, index) => ({
  secret_kind: secretKind,
  owner: credentialRoles[secretKind].owner,
  consumer: credentialRoles[secretKind].consumer,
  delivery: "protected_tmpfs_file",
  activation_method: credentialActivationMethods[secretKind],
  delivered_to_runtime: secretKind !== "fixture_administrator",
  activation_verified: true,
  reload_proven: false,
  overlap_ms: secretKind === "request_hmac" ? 900_000 : 0,
  revocation_verified: true,
  failure_recovered: true,
  evidence_id_sha256: uniqueDigest(index + 3_000),
  cleanup_verified: true,
}));
const sourceBucketSha256 = uniqueDigest(4_000);
const crossScopeCases = [4_001, 4_002].map((index) => ({
  target_bucket_sha256: uniqueDigest(index),
  scope_kind: "other_fleet_bucket",
  attempts: 100,
  denied: 100,
  succeeded: 0,
  provider_effects: 0,
}));
const provenanceCases = provenanceMismatchFields.map((mismatchField, index) => ({
  mismatch_field: mismatchField,
  approved_identity_before_sha256: uniqueDigest(5_000),
  approved_identity_after_sha256: uniqueDigest(5_000),
  candidate_identity_sha256: uniqueDigest(index + 5_001),
  approved_fields_sha256: Object.fromEntries(provenanceMismatchFields.map((field, fieldIndex) => [field, uniqueDigest(5_100 + fieldIndex)])),
  candidate_fields_sha256: Object.fromEntries(provenanceMismatchFields.map((field, fieldIndex) => [
    field,
    uniqueDigest(field === mismatchField ? 5_200 + index : 5_100 + fieldIndex),
  ])),
  verifier: ({ version: "version_policy", commit: "commit_pin", digest: "digest_pin", signature: "signature_verification" })[mismatchField],
  verifier_executed: true,
  verifier_result: "mismatch",
  verifier_evidence_sha256: uniqueDigest(index + 5_300),
  install_attempts: 1,
  blocked_attempts: 1,
  install_effects: 0,
  response_code: "celld.provenance_mismatch",
  mismatch_detected: true,
}));

const classificationCases = boundaries.map((boundary) => ({
  boundary,
  injection_applied: true,
  injection_verified: true,
  surfaces: surfaces.map((surface) => ({ surface, classification: boundary })),
  repairs: repairSurfaces.map((surface) => ({ surface, representation: "plan", effect_claimed: false })),
  healed: true,
  heal_verified: true,
}));

const correlationRecords = boundaries.flatMap((boundary, boundaryIndex) => surfaces.map((surface) => ({
  boundary,
  surface,
  identities: {
    fleet_id: "fleet-test",
    instance_id: `instance-${boundaryIndex}`,
    generation: 1,
    operation_id: `operation-${boundaryIndex}`,
    trace_id: (boundaryIndex + 1).toString(16).padStart(32, "0"),
    celld_version: "v0.3.0",
    adapter_version: "2026.8.3",
    node_id: `node-${boundaryIndex % 3}`,
  },
})));

const alerts = boundaries.map((boundary) => {
  const injectedAt = 1_000;
  const retryInterval = 60_000;
  const evaluationInterval = 60_000;
  const detectionDelay = boundary === "divergence" ? 300_000 : boundary === "unknown_effect" ? retryInterval * 2 : boundary === "stale_generation" ? evaluationInterval : 1_000;
  const detectedAt = injectedAt + detectionDelay;
  const healedAt = detectedAt + 1_000;
  return { boundary, injected_at_ms: injectedAt, detected_at_ms: detectedAt, healed_at_ms: healedAt, resolved_at_ms: healedAt + 1_000, evaluation_interval_ms: evaluationInterval, retry_interval_ms: retryInterval };
});

const recoveryRunbooks = ["node_loss", "full_restart", "authorization_loss", "snapshot_restore", "credential_rotation"];
const recoveryRestores = [1, 2].map((execution) => ({
  execution,
  snapshot_version_id: `snapshot-version-${execution}`,
  source_prefix: "fleet/source",
  restore_prefix: `fleet/isolated-restore-${execution}`,
  isolated_restore: true,
  quarantined: true,
  source_writers_stopped: true,
  restore_authority_exclusive: true,
  latest_acknowledged_at_ms: 1_000,
  snapshot_captured_at_ms: 301_000,
  restore_started_at_ms: 302_000,
  restore_ready_at_ms: 1_802_000,
  generation_manifest_before_sha256: digest("a"),
  generation_manifest_after_sha256: digest("a"),
  tombstone_manifest_before_sha256: digest("b"),
  tombstone_manifest_after_sha256: digest("b"),
}));
const recoveryExecutions = recoveryRunbooks.map((runbook, index) => {
  const operationIds = [`operation-${index}`];
  const effectIds = [`effect-${index}`];
  return {
    runbook,
    executions: [
      { ordinal: 1, operation_ids: [...operationIds], lifecycle_effect_ids: [...effectIds], state_sha256_after: digest("c") },
      { ordinal: 2, operation_ids: [...operationIds], lifecycle_effect_ids: [...effectIds], state_sha256_after: digest("c") },
    ],
    healed: true,
    cleanup_verified: true,
  };
});
const recoveryEvidenceKinds = ["snapshot_identity", "restore_timeline", "generation_comparison", "evidence_manifest"];
const recoveryArtifacts = recoveryEvidenceKinds.map((kind, index) => ({
  kind,
  storage_authority_id: "external-evidence-store",
  bytes: 1_024 + index,
  sha256: digest((index + 1).toString(16)),
  downloaded_sha256: digest((index + 1).toString(16)),
  corruption_probe: { tampered_sha256: digest(String.fromCharCode(97 + index)), detected: true },
  read_after_fleet_loss: true,
  retained: true,
}));

const passing = {
  "CELLD.003.ONE_EFFECT": { cases: replayCases },
  "CELLD.003.COLLISION": { cases: collisionCases },
  "CELLD.004.NO_LOSS": { cases: restartCases },
  "CELLD.004.RECOVERY": { cases: restartCases, provider_cases: restartProviderCases, components_healthy: true, inventory_restored: true },
  "CELLD.005.ORIGINAL_ID": { cases: responseLossCases },
  "CELLD.005.NO_SECOND_EFFECT": { cases: responseLossCases, proxy_healed: true },
  "CELLD.006.PRE_PROVIDER": { cases: staleCases },
  "CELLD.006.ACTIVE_SAFE": { cases: activeGenerationCases },
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
  "CELLD.013.NO_LEAK": { protected_credentials: protectedCredentials, scans: credentialScans, unprotected_secret_files: 0, evidence_secret_findings: 0, all_disposable_secrets_removed: true },
  "CELLD.013.SCOPE": { lifecycles: credentialLifecycles, scope_mode: "per_fleet_bucket", shared_prefix_claimed: false, source_bucket_sha256: sourceBucketSha256, other_fleet_bucket_count: 2, cross_scope_cases: crossScopeCases, hmac_canary_succeeded: true, old_hmac_revoked_after_canary: true, revoked_hmac_denied: true, failed_canary_restored_original: true, original_config_sha256: uniqueDigest(4_100), candidate_config_sha256: uniqueDigest(4_101), restored_config_sha256: uniqueDigest(4_100), active_path_healthy: true },
  "CELLD.013.PROVENANCE": { cases: provenanceCases, approved_pin_count: 4, unapproved_pin_count: 0, only_approved_pins_remain: true },
  "CELLD.014.CLASSIFICATION": { cases: classificationCases },
  "CELLD.014.CORRELATION": { records: correlationRecords, redaction: { surfaces_scanned: surfaces, artifacts_scanned: correlationRecords.length, secret_findings: 0 }, evidence_exported: true, fleet_baseline_restored: true },
  "CELLD.014.ALERTS": { alerts },
  "CELLD.015.OBJECTIVES": { restores: recoveryRestores },
  "CELLD.015.IDEMPOTENT": { runbooks: recoveryExecutions },
  "CELLD.015.EVIDENCE": { affected_fleet_store_id: "affected-fleet-store", external_evidence_store_id: "external-evidence-store", artifacts: recoveryArtifacts, affected_fleet_unavailable: true, external_evidence_store_reachable: true, manifest_verified: true, malicious_runner_tamper_proof_claimed: false },
};

test("every authorized non-storage live assertion has one trusted evaluator", () => {
  const catalog = JSON.parse(readFileSync(new URL("./scenarios.json", import.meta.url), "utf8"));
  const assigned = catalog.scenarios.flatMap((scenario) => (scenario.execution.live_drivers ?? []).flatMap((driver) => driver.covers_assertions));
  const expected = assigned.filter((id) => id !== "CELLD.010.STORAGE" && !WITHHELD_LIVE_EVALUATOR_IDS.includes(id)).sort();
  const actual = Object.keys(SAFE_LIVE_EVALUATORS).filter((id) => id !== "CELLD.010.STORAGE").sort();
  assert.deepEqual(actual, expected);
  assert.deepEqual([...WITHHELD_LIVE_EVALUATOR_IDS], ["CELLD.013.NO_LEAK", "CELLD.013.SCOPE", "CELLD.013.PROVENANCE"]);
  assert.deepEqual(Object.keys(CANDIDATE_LIVE_EVALUATORS).sort(), [...WITHHELD_LIVE_EVALUATOR_IDS].sort());
});

test("trusted and withheld candidate formulas pass exact boundary measurements", () => {
  assert.deepEqual(Object.keys(passing).sort(), Object.keys(TEST_LIVE_EVALUATORS).filter((id) => id !== "CELLD.010.STORAGE").sort());
  for (const [id, measurements] of Object.entries(passing)) {
    const result = TEST_LIVE_EVALUATORS[id](structuredClone(measurements));
    assert.equal(result.passed, true, id);
    assert.equal(result.reason.length > 0, true, id);
  }
});

test("credential no-leak candidate accepts both bounded delivery forms", () => {
  const alternateDelivery = structuredClone(passing["CELLD.013.NO_LEAK"]);
  alternateDelivery.protected_credentials[0].mode_octal = "0400";
  const inherited = alternateDelivery.protected_credentials[1];
  inherited.delivery = "protected_inherited_fd";
  delete inherited.mode_octal;
  delete inherited.regular_file;
  delete inherited.symlink;
  inherited.fd_number = 3;
  inherited.close_on_exec = true;
  inherited.inherited_by_exact_consumer = true;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.NO_LEAK"](alternateDelivery).passed, true);
});

test("trusted formulas reject threshold, uniqueness, isolation, and recovery violations", () => {
  const cases = [
    ["CELLD.003.ONE_EFFECT", { cases: replayCases.map((entry, index) => index === 0 ? { ...entry, provider_effect_count: 2 } : entry) }],
    ["CELLD.004.RECOVERY", { cases: restartCases.map((entry) => ({ ...entry, recovery_ms: 30_001 })) }],
    ["CELLD.006.PRE_PROVIDER", { cases: staleCases.map((entry, index) => index === 0 ? { ...entry, provider_effects: 1 } : entry) }],
    ["CELLD.007.CLAIMS", { not_run_cases: 1, passed_cases: 7 }],
    ["CELLD.008.NO_SIDE_EFFECT", { sockets_created: 1 }],
    ["CELLD.009.NEIGHBOR", { adjacent_successes: 9_899 }],
    ["CELLD.010.ISOLATION", { denied: 2_999, succeeded: 1 }],
    ["CELLD.011.BUDGET", { max_unavailable_observed: 2 }],
    ["CELLD.012.DENIAL", { provider_effects: 1 }],
    ["CELLD.013.NO_LEAK", { scans: credentialScans.map((entry, index) => index === 0 ? { ...entry, canary_matches: 1 } : entry) }],
    ["CELLD.013.SCOPE", { lifecycles: credentialLifecycles.map((entry) => entry.secret_kind === "request_hmac" ? { ...entry, overlap_ms: 900_001 } : entry) }],
    ["CELLD.013.PROVENANCE", { cases: provenanceCases.map((entry) => entry.mismatch_field === "digest" ? { ...entry, install_effects: 1 } : entry) }],
    ["CELLD.014.ALERTS", { alerts: alerts.map((alert) => alert.boundary === "unknown_effect" ? { ...alert, detected_at_ms: alert.injected_at_ms + alert.retry_interval_ms * 2 + 1 } : alert) }],
    ["CELLD.015.OBJECTIVES", { restores: recoveryRestores.map((restore) => restore.execution === 2 ? { ...restore, restore_ready_at_ms: restore.restore_started_at_ms + 1_800_001 } : restore) }],
  ];
  for (const [id, changes] of cases) {
    assert.equal(TEST_LIVE_EVALUATORS[id]({ ...structuredClone(passing[id]), ...changes }).passed, false, id);
  }
});

test("malformed measurements are evidence errors, not product failures", () => {
  assert.throws(() => TEST_LIVE_EVALUATORS["CELLD.003.ONE_EFFECT"]({ cases: "not-an-array" }), /object array/);
  assert.throws(() => TEST_LIVE_EVALUATORS["CELLD.012.VALID"](null), /must be an object/);
  assert.throws(() => TEST_LIVE_EVALUATORS["CELLD.012.VALID"]({ ...passing["CELLD.012.VALID"], verdict: "PASS" }), /forbidden self-declared verdict/);
  assert.throws(() => TEST_LIVE_EVALUATORS["CELLD.014.CLASSIFICATION"]({ boundaries, injected: boundaries.length }), /object array/);
});

test("orchestration formulas reject aggregate-only, incomplete, duplicate, and amplified case evidence", () => {
  assert.throws(() => TEST_LIVE_EVALUATORS["CELLD.003.ONE_EFFECT"]({ repeats_per_action: 10_000, provider_effects: 8 }), /object array/);

  const missingCrashTrial = structuredClone(passing["CELLD.004.NO_LOSS"]);
  missingCrashTrial.cases.pop();
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.004.NO_LOSS"](missingCrashTrial).passed, false);

  const duplicateResponseIdentity = structuredClone(passing["CELLD.005.ORIGINAL_ID"]);
  duplicateResponseIdentity.cases[1].operation_id_sha256 = duplicateResponseIdentity.cases[0].operation_id_sha256;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.005.ORIGINAL_ID"](duplicateResponseIdentity).passed, false);

  const amplifiedResponseLoss = structuredClone(passing["CELLD.005.NO_SECOND_EFFECT"]);
  amplifiedResponseLoss.cases[0].attempts = 4;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.005.NO_SECOND_EFFECT"](amplifiedResponseLoss).passed, false);

  const unhealedPartition = structuredClone(passing["CELLD.006.ACTIVE_SAFE"]);
  unhealedPartition.cases[0].partition_healed = false;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.006.ACTIVE_SAFE"](unhealedPartition).passed, false);
});

test("credential evaluators reject incomplete, duplicate, prefix, and unproven reload evidence", () => {
  const incompleteLeakScan = structuredClone(passing["CELLD.013.NO_LEAK"]);
  incompleteLeakScan.scans.pop();
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.NO_LEAK"](incompleteLeakScan).passed, false);

  const vacuousCanary = structuredClone(passing["CELLD.013.NO_LEAK"]);
  vacuousCanary.protected_credentials[0].canary_matches_in_reference = 0;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.NO_LEAK"](vacuousCanary).passed, false);

  const duplicateCanary = structuredClone(passing["CELLD.013.NO_LEAK"]);
  duplicateCanary.protected_credentials[1].canary_id_sha256 = duplicateCanary.protected_credentials[0].canary_id_sha256;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.NO_LEAK"](duplicateCanary).passed, false);

  const collidingCanary = structuredClone(passing["CELLD.013.NO_LEAK"]);
  collidingCanary.protected_credentials[1].canary_id_sha256 = collidingCanary.protected_credentials[0].credential_ref_sha256;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.NO_LEAK"](collidingCanary).passed, false);

  const incompleteInventory = structuredClone(passing["CELLD.013.NO_LEAK"]);
  incompleteInventory.scans[0].artifacts_scanned -= 1;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.NO_LEAK"](incompleteInventory).passed, false);

  const unboundInventory = structuredClone(passing["CELLD.013.NO_LEAK"]);
  unboundInventory.scans[0].scanned_inventory_sha256 = uniqueDigest(9_000);
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.NO_LEAK"](unboundInventory).passed, false);

  const duplicateScope = structuredClone(passing["CELLD.013.SCOPE"]);
  duplicateScope.cross_scope_cases[1].target_bucket_sha256 = duplicateScope.cross_scope_cases[0].target_bucket_sha256;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.SCOPE"](duplicateScope).passed, false);

  const prefixClaim = structuredClone(passing["CELLD.013.SCOPE"]);
  prefixClaim.shared_prefix_claimed = true;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.SCOPE"](prefixClaim).passed, false);

  const unprovenReload = structuredClone(passing["CELLD.013.SCOPE"]);
  const s3 = unprovenReload.lifecycles.find((entry) => entry.secret_kind === "s3_access_identity");
  s3.activation_method = "hot_reload";
  s3.reload_proven = false;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.SCOPE"](unprovenReload).passed, false);

  const wrongOwner = structuredClone(passing["CELLD.013.SCOPE"]);
  wrongOwner.lifecycles.find((entry) => entry.secret_kind === "fixture_administrator").owner = "celld_fleet";
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.SCOPE"](wrongOwner).passed, false);

  const zeroHmacOverlap = structuredClone(passing["CELLD.013.SCOPE"]);
  zeroHmacOverlap.lifecycles.find((entry) => entry.secret_kind === "request_hmac").overlap_ms = 0;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.SCOPE"](zeroHmacOverlap).passed, false);

  const deliveredFixtureAdmin = structuredClone(passing["CELLD.013.SCOPE"]);
  const fixtureAdmin = deliveredFixtureAdmin.lifecycles.find((entry) => entry.secret_kind === "fixture_administrator");
  fixtureAdmin.consumer = "celld_fleet";
  fixtureAdmin.delivered_to_runtime = true;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.SCOPE"](deliveredFixtureAdmin).passed, false);

  const unprovenRestore = structuredClone(passing["CELLD.013.SCOPE"]);
  unprovenRestore.restored_config_sha256 = unprovenRestore.candidate_config_sha256;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.SCOPE"](unprovenRestore).passed, false);

  const duplicateCandidate = structuredClone(passing["CELLD.013.PROVENANCE"]);
  duplicateCandidate.cases[1].candidate_identity_sha256 = duplicateCandidate.cases[0].candidate_identity_sha256;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.PROVENANCE"](duplicateCandidate).passed, false);

  const unchangedSelectedField = structuredClone(passing["CELLD.013.PROVENANCE"]);
  const digestMismatch = unchangedSelectedField.cases.find((entry) => entry.mismatch_field === "digest");
  digestMismatch.candidate_fields_sha256.digest = digestMismatch.approved_fields_sha256.digest;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.PROVENANCE"](unchangedSelectedField).passed, false);

  const changedControlField = structuredClone(passing["CELLD.013.PROVENANCE"]);
  const versionMismatch = changedControlField.cases.find((entry) => entry.mismatch_field === "version");
  versionMismatch.candidate_fields_sha256.commit = uniqueDigest(9_001);
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.PROVENANCE"](changedControlField).passed, false);

  const unexecutedVerifier = structuredClone(passing["CELLD.013.PROVENANCE"]);
  unexecutedVerifier.cases.find((entry) => entry.mismatch_field === "signature").verifier_executed = false;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.PROVENANCE"](unexecutedVerifier).passed, false);

  const mislabeledVerifier = structuredClone(passing["CELLD.013.PROVENANCE"]);
  mislabeledVerifier.cases.find((entry) => entry.mismatch_field === "commit").verifier = "digest_pin";
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.013.PROVENANCE"](mislabeledVerifier).passed, false);
});

test("observability formulas derive cross-surface agreement, repair honesty, correlation, redaction, and alert lifecycle", () => {
  const disagreement = structuredClone(passing["CELLD.014.CLASSIFICATION"]);
  disagreement.cases[0].surfaces[0].classification = "management";
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.014.CLASSIFICATION"](disagreement).passed, false);

  const claimedRepair = structuredClone(passing["CELLD.014.CLASSIFICATION"]);
  claimedRepair.cases[0].repairs[0].effect_claimed = true;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.014.CLASSIFICATION"](claimedRepair).passed, false);

  const duplicateBoundary = structuredClone(passing["CELLD.014.CLASSIFICATION"]);
  duplicateBoundary.cases[0].boundary = duplicateBoundary.cases[1].boundary;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.014.CLASSIFICATION"](duplicateBoundary).passed, false);

  const missingIdentity = structuredClone(passing["CELLD.014.CORRELATION"]);
  missingIdentity.records[0].identities.operation_id = "";
  assert.throws(() => TEST_LIVE_EVALUATORS["CELLD.014.CORRELATION"](missingIdentity), /non-empty string/);

  const redactionFailure = structuredClone(passing["CELLD.014.CORRELATION"]);
  redactionFailure.redaction.secret_findings = 1;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.014.CORRELATION"](redactionFailure).passed, false);

  const uncorrelated = structuredClone(passing["CELLD.014.CORRELATION"]);
  uncorrelated.records[1].identities.trace_id = "f".repeat(32);
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.014.CORRELATION"](uncorrelated).passed, false);

  const unresolved = structuredClone(passing["CELLD.014.ALERTS"]);
  unresolved.alerts[0].resolved_at_ms = unresolved.alerts[0].healed_at_ms - 1;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.014.ALERTS"](unresolved).passed, false);

  const missingBelowReserve = structuredClone(passing["CELLD.014.ALERTS"]);
  missingBelowReserve.alerts = missingBelowReserve.alerts.filter((alert) => alert.boundary !== "below_reserve");
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.014.ALERTS"](missingBelowReserve).passed, false);
});

test("recovery formulas derive two isolated restores, five repeated runbooks, and independent artifact readability", () => {
  const staleGeneration = structuredClone(passing["CELLD.015.OBJECTIVES"]);
  staleGeneration.restores[0].generation_manifest_after_sha256 = digest("f");
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.015.OBJECTIVES"](staleGeneration).passed, false);

  const sharedPrefix = structuredClone(passing["CELLD.015.OBJECTIVES"]);
  sharedPrefix.restores[0].restore_prefix = sharedPrefix.restores[0].source_prefix;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.015.OBJECTIVES"](sharedPrefix).passed, false);

  const replayEffect = structuredClone(passing["CELLD.015.IDEMPOTENT"]);
  replayEffect.runbooks[0].executions[1].lifecycle_effect_ids.push("unexpected-second-effect");
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.015.IDEMPOTENT"](replayEffect).passed, false);

  const missingRunbook = structuredClone(passing["CELLD.015.IDEMPOTENT"]);
  missingRunbook.runbooks = missingRunbook.runbooks.filter((runbook) => runbook.runbook !== "credential_rotation");
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.015.IDEMPOTENT"](missingRunbook).passed, false);

  const sameStore = structuredClone(passing["CELLD.015.EVIDENCE"]);
  sameStore.external_evidence_store_id = sameStore.affected_fleet_store_id;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.015.EVIDENCE"](sameStore).passed, false);

  const unreadable = structuredClone(passing["CELLD.015.EVIDENCE"]);
  unreadable.artifacts[0].downloaded_sha256 = digest("f");
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.015.EVIDENCE"](unreadable).passed, false);

  const dishonestClaim = structuredClone(passing["CELLD.015.EVIDENCE"]);
  dishonestClaim.malicious_runner_tamper_proof_claimed = true;
  assert.equal(TEST_LIVE_EVALUATORS["CELLD.015.EVIDENCE"](dishonestClaim).passed, false);
});
