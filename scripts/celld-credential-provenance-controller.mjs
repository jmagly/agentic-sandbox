import { createHash } from "node:crypto";

export const CREDENTIAL_KINDS = Object.freeze([
  "s3_access_identity",
  "request_hmac",
  "mtls_identity",
  "celld_peer_secret",
  "fixture_administrator",
]);

export const CREDENTIAL_SCAN_SURFACES = Object.freeze([
  "argv",
  "captured_env",
  "shell_trace",
  "logs",
  "crash_artifacts",
  "persistent_scratch",
  "support_evidence",
]);

export const PROVENANCE_MISMATCH_FIELDS = Object.freeze([
  "version",
  "commit",
  "digest",
  "signature",
]);

export const CROSS_SCOPE_DIRECTIONS = Object.freeze([
  "source_identity_to_other_bucket",
  "other_identity_to_source_bucket",
]);

const SHA256 = /^[0-9a-f]{64}$/;
const PROVENANCE_VERIFIERS = Object.freeze({
  version: "version_policy",
  commit: "commit_pin",
  digest: "digest_pin",
  signature: "signature_verification",
});

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function object(value, name) {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`${name} must be an object`);
  return value;
}

function rejectUnknown(value, allowed, name) {
  const unknown = Object.keys(value).filter((key) => !allowed.includes(key));
  if (unknown.length > 0) throw new Error(`${name} contains unknown fields: ${unknown.join(",")}`);
}

function digest(value, name) {
  if (!SHA256.test(value ?? "")) throw new Error(`${name} must be a lowercase SHA-256 digest`);
  return value;
}

function exactKinds(records, field, expected, name) {
  if (!Array.isArray(records) || records.length !== expected.length) throw new Error(`${name} must contain the exact matrix`);
  const observed = records.map((record, index) => object(record, `${name}[${index}]`)[field]);
  if (new Set(observed).size !== expected.length || expected.some((value) => !observed.includes(value))) throw new Error(`${name} must contain the exact matrix`);
  return records;
}

function protectedCredentialRecords(records) {
  exactKinds(records, "secret_kind", CREDENTIAL_KINDS, "protected credential inventory");
  const identityHashes = new Set();
  const referenceHashes = new Set();
  const canaryHashes = new Set();
  for (const [index, record] of records.entries()) {
    rejectUnknown(record, [
      "secret_kind", "credential_id_sha256", "credential_ref_sha256", "canary_id_sha256", "delivery", "owner_only",
      "mode_octal", "regular_file", "symlink", "fd_number", "close_on_exec", "inherited_by_exact_consumer",
      "canary_matches_in_reference", "canary_matches_outside_reference",
    ], `protected credential ${index}`);
    const identity = digest(record.credential_id_sha256, `protected credential ${index} identity`);
    const reference = digest(record.credential_ref_sha256, `protected credential ${index} reference`);
    const canary = digest(record.canary_id_sha256, `protected credential ${index} canary`);
    if (identityHashes.has(identity) || referenceHashes.has(identity) || canaryHashes.has(identity)
        || identityHashes.has(reference) || referenceHashes.has(reference) || canaryHashes.has(reference)
        || identityHashes.has(canary) || referenceHashes.has(canary) || canaryHashes.has(canary)) {
      throw new Error("credential identity, reference, and canary hashes must be unique and disjoint");
    }
    identityHashes.add(identity);
    referenceHashes.add(reference);
    canaryHashes.add(canary);
    if (record.delivery === "protected_tmpfs_file") {
      if (!["0400", "0600"].includes(record.mode_octal) || record.regular_file !== true || record.symlink !== false || record.owner_only !== true) throw new Error("protected credential file evidence is invalid");
    } else if (record.delivery === "protected_inherited_fd") {
      if (!Number.isSafeInteger(record.fd_number) || record.fd_number < 3 || record.close_on_exec !== true || record.inherited_by_exact_consumer !== true || record.owner_only !== true) throw new Error("protected credential fd evidence is invalid");
    } else throw new Error("credential delivery is outside the reviewed boundary");
    if (record.canary_matches_in_reference !== 1 || record.canary_matches_outside_reference !== 0) throw new Error("credential canary confinement evidence is invalid");
  }
  return records.map((record) => ({ ...record }));
}

function lifecycleRecords(records) {
  exactKinds(records, "secret_kind", CREDENTIAL_KINDS, "credential lifecycle evidence");
  const evidenceIds = new Set();
  for (const [index, record] of records.entries()) {
    rejectUnknown(record, [
      "secret_kind", "owner", "consumer", "delivery", "activation_method", "delivered_to_runtime", "activation_verified",
      "reload_proven", "overlap_ms", "revocation_verified", "failure_recovered", "evidence_id_sha256", "cleanup_verified",
    ], `credential lifecycle ${index}`);
    const evidenceId = digest(record.evidence_id_sha256, `credential lifecycle ${index} evidence identity`);
    if (evidenceIds.has(evidenceId)) throw new Error("credential lifecycle evidence identities must be unique");
    evidenceIds.add(evidenceId);
    if (record.activation_verified !== true || record.revocation_verified !== true || record.failure_recovered !== true || record.cleanup_verified !== true) throw new Error("credential lifecycle did not prove activation, revocation, recovery, and cleanup");
  }
  return records.map((record) => ({ ...record }));
}

function scopeEvidence(value) {
  const evidence = object(value, "cross-scope evidence");
  rejectUnknown(evidence, ["source_bucket_sha256", "other_bucket_sha256", "cases"], "cross-scope evidence");
  const source = digest(evidence.source_bucket_sha256, "source bucket identity");
  if (!Array.isArray(evidence.other_bucket_sha256) || evidence.other_bucket_sha256.length < 2) throw new Error("cross-scope evidence requires at least two other fleet buckets");
  const other = evidence.other_bucket_sha256.map((value, index) => digest(value, `other bucket ${index}`));
  if (new Set(other).size !== other.length || other.includes(source)) throw new Error("cross-scope bucket identities must be unique");
  const cases = exactKinds(
    evidence.cases,
    "case_id",
    other.flatMap((bucket) => CROSS_SCOPE_DIRECTIONS.map((direction) => `${direction}:${bucket}`)),
    "cross-scope cases",
  );
  for (const [index, entry] of cases.entries()) {
    rejectUnknown(entry, [
      "case_id", "direction", "other_bucket_sha256", "credential_bucket_sha256", "target_bucket_sha256", "scope_kind",
      "attempts", "denied", "succeeded", "provider_effects",
    ], `cross-scope case ${index}`);
    if (!CROSS_SCOPE_DIRECTIONS.includes(entry.direction) || entry.case_id !== `${entry.direction}:${entry.other_bucket_sha256}` || !other.includes(digest(entry.other_bucket_sha256, `cross-scope case ${index} other bucket`))) throw new Error("cross-scope case identity is invalid");
    const credential = digest(entry.credential_bucket_sha256, `cross-scope case ${index} credential scope`);
    const target = digest(entry.target_bucket_sha256, `cross-scope case ${index} target scope`);
    if (entry.direction === "source_identity_to_other_bucket" ? credential !== source || target !== entry.other_bucket_sha256 : credential !== entry.other_bucket_sha256 || target !== source) throw new Error("cross-scope direction is not bound to credential and target scopes");
    if (!Number.isSafeInteger(entry.attempts) || entry.attempts < 1 || entry.denied !== entry.attempts || entry.succeeded !== 0 || entry.provider_effects !== 0 || entry.scope_kind !== "other_fleet_bucket") throw new Error("cross-scope denial evidence is invalid");
  }
  return { source_bucket_sha256: source, other_bucket_sha256: other, cases: cases.map((entry) => ({ ...entry })) };
}

function hmacEvidence(value) {
  const evidence = object(value, "request HMAC rotation evidence");
  rejectUnknown(evidence, [
    "hmac_canary_succeeded", "old_hmac_revoked_after_canary", "revoked_hmac_denied", "failed_canary_restored_original",
    "original_config_sha256", "candidate_config_sha256", "restored_config_sha256", "active_path_healthy",
  ], "request HMAC rotation evidence");
  for (const field of ["original_config_sha256", "candidate_config_sha256", "restored_config_sha256"]) digest(evidence[field], field);
  if (evidence.original_config_sha256 === evidence.candidate_config_sha256 || evidence.restored_config_sha256 !== evidence.original_config_sha256
      || evidence.hmac_canary_succeeded !== true || evidence.old_hmac_revoked_after_canary !== true || evidence.revoked_hmac_denied !== true
      || evidence.failed_canary_restored_original !== true || evidence.active_path_healthy !== true) throw new Error("request HMAC rotation evidence is invalid");
  return { ...evidence };
}

function provenanceRecords(records) {
  exactKinds(records, "mismatch_field", PROVENANCE_MISMATCH_FIELDS, "provenance mismatch evidence");
  for (const [index, record] of records.entries()) {
    rejectUnknown(record, [
      "mismatch_field", "approved_identity_before_sha256", "approved_identity_after_sha256", "candidate_identity_sha256",
      "approved_fields_sha256", "candidate_fields_sha256", "verifier", "verifier_executed", "verifier_result",
      "verifier_evidence_sha256", "install_attempts", "blocked_attempts", "install_effects", "response_code", "mismatch_detected",
    ], `provenance case ${index}`);
    for (const field of ["approved_identity_before_sha256", "approved_identity_after_sha256", "candidate_identity_sha256", "verifier_evidence_sha256"]) digest(record[field], `provenance case ${index} ${field}`);
    const approvedFields = object(record.approved_fields_sha256, `provenance case ${index} approved fields`);
    const candidateFields = object(record.candidate_fields_sha256, `provenance case ${index} candidate fields`);
    rejectUnknown(approvedFields, PROVENANCE_MISMATCH_FIELDS, `provenance case ${index} approved fields`);
    rejectUnknown(candidateFields, PROVENANCE_MISMATCH_FIELDS, `provenance case ${index} candidate fields`);
    if (Object.keys(approvedFields).length !== PROVENANCE_MISMATCH_FIELDS.length || Object.keys(candidateFields).length !== PROVENANCE_MISMATCH_FIELDS.length) throw new Error("provenance identity field matrix is incomplete");
    for (const identityField of PROVENANCE_MISMATCH_FIELDS) {
      const approved = digest(approvedFields[identityField], `provenance case ${index} approved ${identityField}`);
      const candidate = digest(candidateFields[identityField], `provenance case ${index} candidate ${identityField}`);
      if (identityField === record.mismatch_field ? candidate === approved : candidate !== approved) throw new Error("provenance case must change exactly its selected identity field");
    }
    if (record.approved_identity_before_sha256 !== record.approved_identity_after_sha256 || record.candidate_identity_sha256 === record.approved_identity_before_sha256
        || record.verifier !== PROVENANCE_VERIFIERS[record.mismatch_field] || record.verifier_executed !== true || record.verifier_result !== "mismatch" || record.blocked_attempts !== record.install_attempts
        || !Number.isSafeInteger(record.install_attempts) || record.install_attempts < 1 || record.install_effects !== 0
        || record.response_code !== "celld.provenance_mismatch" || record.mismatch_detected !== true) throw new Error("provenance mismatch case did not fail closed before installation");
  }
  return records.map((record) => ({ ...record }));
}

function scanRecords(records, expectedInventorySha256, canaryCount) {
  exactKinds(records, "surface", CREDENTIAL_SCAN_SURFACES, "credential scan evidence");
  for (const [index, record] of records.entries()) {
    rejectUnknown(record, [
      "surface", "expected_artifact_count", "artifacts_scanned", "expected_inventory_sha256", "scanned_inventory_sha256",
      "canaries_expected", "canaries_scanned", "canary_matches",
    ], `credential scan ${index}`);
    if (digest(record.expected_inventory_sha256, `scan ${index} expected inventory`) !== expectedInventorySha256
        || digest(record.scanned_inventory_sha256, `scan ${index} observed inventory`) !== expectedInventorySha256
        || !Number.isSafeInteger(record.expected_artifact_count) || record.expected_artifact_count < 1
        || record.artifacts_scanned !== record.expected_artifact_count || record.canaries_expected !== canaryCount
        || record.canaries_scanned !== canaryCount || record.canary_matches !== 0) throw new Error("credential scan evidence is incomplete or contains a canary");
  }
  return records.map((record) => ({ ...record }));
}

function requireAdapter(adapter) {
  object(adapter, "credential campaign adapter");
  for (const method of ["inspectCredentials", "persistIntent", "exerciseLifecycle", "exerciseCrossScope", "exerciseHmacRotation", "verifyProvenanceMismatch", "scanSurface", "observeApprovedPins", "cleanup"]) {
    if (typeof adapter[method] !== "function") throw new Error(`credential campaign adapter.${method} is required`);
  }
}

export class CredentialCampaignCleanupError extends Error {
  constructor(message) {
    super(message);
    this.name = "CredentialCampaignCleanupError";
    this.exitCode = 4;
  }
}

export async function executeCredentialProvenanceCampaign({ runId, adapter }) {
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(runId ?? "")) throw new Error("credential campaign run identity is invalid");
  requireAdapter(adapter);
  const timeline = [];
  let mutationStarted = false;
  let operationError = null;
  let result = null;
  let cleanup = null;
  try {
    const protectedCredentials = protectedCredentialRecords(await adapter.inspectCredentials());
    const expectedInventorySha256 = sha256(JSON.stringify(protectedCredentials.map((entry) => ({
      secret_kind: entry.secret_kind,
      credential_id_sha256: entry.credential_id_sha256,
      credential_ref_sha256: entry.credential_ref_sha256,
      canary_id_sha256: entry.canary_id_sha256,
    })).sort((left, right) => left.secret_kind.localeCompare(right.secret_kind))));
    const lifecycles = [];
    for (const credential of protectedCredentials) {
      const intent = { kind: "credential_lifecycle", secret_kind: credential.secret_kind, credential_ref_sha256: credential.credential_ref_sha256 };
      await adapter.persistIntent(intent);
      mutationStarted = true;
      timeline.push({ event: "intent_persisted", ...intent });
      lifecycles.push(await adapter.exerciseLifecycle(credential.secret_kind));
    }
    const scopeIntent = { kind: "cross_scope_matrix", run_id_sha256: sha256(runId) };
    await adapter.persistIntent(scopeIntent);
    timeline.push({ event: "intent_persisted", ...scopeIntent });
    const scope = scopeEvidence(await adapter.exerciseCrossScope());
    const hmacIntent = { kind: "request_hmac_rotation", run_id_sha256: sha256(runId) };
    await adapter.persistIntent(hmacIntent);
    timeline.push({ event: "intent_persisted", ...hmacIntent });
    const hmac = hmacEvidence(await adapter.exerciseHmacRotation());
    const provenance = [];
    for (const mismatchField of PROVENANCE_MISMATCH_FIELDS) {
      const intent = { kind: "provenance_mismatch", mismatch_field: mismatchField };
      await adapter.persistIntent(intent);
      timeline.push({ event: "intent_persisted", ...intent });
      provenance.push(await adapter.verifyProvenanceMismatch(mismatchField));
    }
    const scans = [];
    for (const surface of CREDENTIAL_SCAN_SURFACES) scans.push(await adapter.scanSurface(surface, { expectedInventorySha256, canaryIds: protectedCredentials.map((entry) => entry.canary_id_sha256) }));
    const pins = object(await adapter.observeApprovedPins(), "approved pin observation");
    rejectUnknown(pins, ["approved_pin_count", "unapproved_pin_count", "only_approved_pins_remain"], "approved pin observation");
    if (!Number.isSafeInteger(pins.approved_pin_count) || pins.approved_pin_count < 1 || pins.unapproved_pin_count !== 0 || pins.only_approved_pins_remain !== true) throw new Error("approved pin observation is invalid");
    result = {
      mutation_started: mutationStarted,
      protected_credentials: protectedCredentials,
      lifecycles: lifecycleRecords(lifecycles),
      scope,
      hmac,
      provenance: provenanceRecords(provenance),
      scans: scanRecords(scans, expectedInventorySha256, protectedCredentials.length),
      pins: { ...pins },
      expected_inventory_sha256: expectedInventorySha256,
      timeline,
    };
  } catch (error) {
    operationError = error;
  } finally {
    try {
      cleanup = object(await adapter.cleanup(), "credential campaign cleanup");
    } catch (error) {
      throw new CredentialCampaignCleanupError(`credential campaign cleanup failed: ${sha256(error.message)}`);
    }
  }
  try {
    rejectUnknown(cleanup, ["status", "unprotected_secret_files", "evidence_secret_findings", "all_disposable_secrets_removed"], "credential campaign cleanup");
    if (cleanup.status !== "passed" || cleanup.unprotected_secret_files !== 0 || cleanup.evidence_secret_findings !== 0 || cleanup.all_disposable_secrets_removed !== true) throw new Error("cleanup proof is incomplete");
  } catch {
    throw new CredentialCampaignCleanupError("credential campaign cleanup did not prove complete secret removal");
  }
  if (operationError) throw operationError;
  return { ...result, cleanup: { ...cleanup } };
}
