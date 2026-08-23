import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";

import {
  CREDENTIAL_KINDS,
  CREDENTIAL_SCAN_SURFACES,
  CredentialCampaignCleanupError,
  PROVENANCE_MISMATCH_FIELDS,
  executeCredentialProvenanceCampaign,
} from "../../../scripts/celld-credential-provenance-controller.mjs";

function digest(value) {
  return createHash("sha256").update(String(value)).digest("hex");
}

const roles = Object.freeze({
  s3_access_identity: { owner: "fixture_controller", consumer: "celld_fleet", activation_method: "controlled_restart" },
  request_hmac: { owner: "management_adapter", consumer: "management_and_worker", activation_method: "dual_key_overlap" },
  mtls_identity: { owner: "project_ca", consumer: "management_and_callback_relay", activation_method: "controlled_restart" },
  celld_peer_secret: { owner: "celld_store_authority", consumer: "celld_fleet", activation_method: "controlled_restart" },
  fixture_administrator: { owner: "fixture_controller", consumer: "fixture_controller_only", activation_method: "fixture_controller_only" },
});

function passingAdapter(overrides = {}) {
  const events = [];
  const credentials = CREDENTIAL_KINDS.map((secretKind, index) => ({
    secret_kind: secretKind,
    credential_id_sha256: digest(`credential-${index}`),
    credential_ref_sha256: digest(`reference-${index}`),
    canary_id_sha256: digest(`canary-${index}`),
    delivery: "protected_tmpfs_file",
    mode_octal: "0600",
    regular_file: true,
    symlink: false,
    owner_only: true,
    canary_matches_in_reference: 1,
    canary_matches_outside_reference: 0,
  }));
  const source = digest("source-bucket");
  const other = [digest("other-bucket-a"), digest("other-bucket-b")];
  const approvedFields = Object.fromEntries(PROVENANCE_MISMATCH_FIELDS.map((field) => [field, digest(`approved-${field}`)]));
  const adapter = {
    events,
    inspectCredentials: async () => credentials,
    persistIntent: async (intent) => { events.push(`intent:${intent.kind}:${intent.secret_kind ?? intent.mismatch_field ?? "matrix"}`); },
    exerciseLifecycle: async (secretKind) => {
      assert.match(events.at(-1), new RegExp(`credential_lifecycle:${secretKind}$`));
      const role = roles[secretKind];
      return {
        secret_kind: secretKind,
        owner: role.owner,
        consumer: role.consumer,
        delivery: "protected_tmpfs_file",
        activation_method: role.activation_method,
        delivered_to_runtime: secretKind !== "fixture_administrator",
        activation_verified: true,
        reload_proven: false,
        overlap_ms: secretKind === "request_hmac" ? 600_000 : 0,
        revocation_verified: true,
        failure_recovered: true,
        evidence_id_sha256: digest(`lifecycle-${secretKind}`),
        cleanup_verified: true,
      };
    },
    exerciseCrossScope: async () => ({
      source_bucket_sha256: source,
      other_bucket_sha256: other,
      cases: other.flatMap((otherBucket) => [
        {
          case_id: `source_identity_to_other_bucket:${otherBucket}`,
          direction: "source_identity_to_other_bucket",
          other_bucket_sha256: otherBucket,
          credential_bucket_sha256: source,
          target_bucket_sha256: otherBucket,
          scope_kind: "other_fleet_bucket",
          attempts: 100,
          denied: 100,
          succeeded: 0,
          provider_effects: 0,
        },
        {
          case_id: `other_identity_to_source_bucket:${otherBucket}`,
          direction: "other_identity_to_source_bucket",
          other_bucket_sha256: otherBucket,
          credential_bucket_sha256: otherBucket,
          target_bucket_sha256: source,
          scope_kind: "other_fleet_bucket",
          attempts: 100,
          denied: 100,
          succeeded: 0,
          provider_effects: 0,
        },
      ]),
    }),
    exerciseHmacRotation: async () => ({
      hmac_canary_succeeded: true,
      old_hmac_revoked_after_canary: true,
      revoked_hmac_denied: true,
      failed_canary_restored_original: true,
      original_config_sha256: digest("original-hmac"),
      candidate_config_sha256: digest("candidate-hmac"),
      restored_config_sha256: digest("original-hmac"),
      active_path_healthy: true,
    }),
    verifyProvenanceMismatch: async (mismatchField) => ({
      mismatch_field: mismatchField,
      approved_identity_before_sha256: digest("approved-identity"),
      approved_identity_after_sha256: digest("approved-identity"),
      candidate_identity_sha256: digest(`candidate-${mismatchField}`),
      approved_fields_sha256: approvedFields,
      candidate_fields_sha256: Object.fromEntries(PROVENANCE_MISMATCH_FIELDS.map((field) => [field, field === mismatchField ? digest(`candidate-${field}`) : approvedFields[field]])),
      verifier: ({ version: "version_policy", commit: "commit_pin", digest: "digest_pin", signature: "signature_verification" })[mismatchField],
      verifier_executed: true,
      verifier_result: "mismatch",
      verifier_evidence_sha256: digest(`verifier-${mismatchField}`),
      install_attempts: 1,
      blocked_attempts: 1,
      install_effects: 0,
      response_code: "celld.provenance_mismatch",
      mismatch_detected: true,
    }),
    scanSurface: async (surface, { expectedInventorySha256, canaryIds }) => ({
      surface,
      expected_artifact_count: 2,
      artifacts_scanned: 2,
      expected_inventory_sha256: expectedInventorySha256,
      scanned_inventory_sha256: expectedInventorySha256,
      canaries_expected: canaryIds.length,
      canaries_scanned: canaryIds.length,
      canary_matches: 0,
    }),
    observeApprovedPins: async () => ({ approved_pin_count: 4, unapproved_pin_count: 0, only_approved_pins_remain: true }),
    cleanup: async () => { events.push("cleanup"); return { status: "passed", unprotected_secret_files: 0, evidence_secret_findings: 0, all_disposable_secrets_removed: true }; },
    ...overrides,
  };
  return adapter;
}

test("credential campaign binds every lifecycle, bidirectional scope case, verifier, scan, and cleanup", async () => {
  const adapter = passingAdapter();
  const result = await executeCredentialProvenanceCampaign({ runId: "test-run", adapter });
  assert.equal(result.mutation_started, true);
  assert.deepEqual(result.protected_credentials.map((entry) => entry.secret_kind), [...CREDENTIAL_KINDS]);
  assert.deepEqual(result.lifecycles.map((entry) => entry.secret_kind), [...CREDENTIAL_KINDS]);
  assert.equal(result.scope.cases.length, 4);
  assert.deepEqual(new Set(result.scope.cases.map((entry) => entry.direction)), new Set(["source_identity_to_other_bucket", "other_identity_to_source_bucket"]));
  assert.deepEqual(result.provenance.map((entry) => entry.mismatch_field), [...PROVENANCE_MISMATCH_FIELDS]);
  assert.deepEqual(result.scans.map((entry) => entry.surface), [...CREDENTIAL_SCAN_SURFACES]);
  assert.equal(result.timeline.length, 11);
  assert.equal(adapter.events.at(-1), "cleanup");
});

test("credential campaign rejects a substituted reverse scope and still cleans up", async () => {
  const adapter = passingAdapter();
  const original = adapter.exerciseCrossScope;
  adapter.exerciseCrossScope = async () => {
    const evidence = await original();
    evidence.cases.find((entry) => entry.direction === "other_identity_to_source_bucket").credential_bucket_sha256 = evidence.source_bucket_sha256;
    return evidence;
  };
  await assert.rejects(() => executeCredentialProvenanceCampaign({ runId: "test-run", adapter }), /direction is not bound/);
  assert.equal(adapter.events.at(-1), "cleanup");
});

test("credential campaign rejects unknown evidence fields before they can reach an artifact", async () => {
  const adapter = passingAdapter();
  const original = adapter.inspectCredentials;
  adapter.inspectCredentials = async () => {
    const records = await original();
    records[0].secret_value = "must-not-cross-evidence-boundary";
    return records;
  };
  await assert.rejects(() => executeCredentialProvenanceCampaign({ runId: "test-run", adapter }), /unknown fields: secret_value/);
  assert.equal(adapter.events.at(-1), "cleanup");
});

test("credential campaign cleanup failure outranks an operation failure with exit 4", async () => {
  const adapter = passingAdapter({
    exerciseLifecycle: async () => { throw new Error("mutation failed"); },
    cleanup: async () => { throw new Error("residue remains"); },
  });
  await assert.rejects(
    () => executeCredentialProvenanceCampaign({ runId: "test-run", adapter }),
    (error) => error instanceof CredentialCampaignCleanupError && error.exitCode === 4 && !error.message.includes("residue remains"),
  );
});

test("credential campaign requires every reviewed adapter boundary before inspection", async () => {
  const adapter = passingAdapter();
  delete adapter.verifyProvenanceMismatch;
  await assert.rejects(() => executeCredentialProvenanceCampaign({ runId: "test-run", adapter }), /verifyProvenanceMismatch is required/);
  assert.deepEqual(adapter.events, []);
});
