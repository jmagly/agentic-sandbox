import { createHash } from "node:crypto";

export const RECOVERY_RUNBOOKS = Object.freeze([
  "node_loss",
  "full_restart",
  "authorization_loss",
  "snapshot_restore",
  "credential_rotation",
]);

export const RECOVERY_EVIDENCE_KINDS = Object.freeze([
  "snapshot_identity",
  "restore_timeline",
  "generation_comparison",
  "evidence_manifest",
]);

const REQUIRED_ADAPTER_METHODS = Object.freeze([
  "captureBaseline",
  "observeStores",
  "persistIntent",
  "stopSourceWriters",
  "observeSourceWriters",
  "acquireRestoreAuthority",
  "observeRestoreAuthority",
  "createSnapshot",
  "observeSnapshot",
  "restoreSnapshot",
  "observeRestore",
  "releaseRestoreAuthority",
  "startSourceWriters",
  "executeRunbook",
  "observeRunbook",
  "cleanupRunbook",
  "uploadEvidence",
  "observeEvidenceUpload",
  "makeAffectedFleetUnavailable",
  "observeFleetAvailability",
  "readExternalEvidence",
  "probeEvidenceCorruption",
  "verifyEvidenceManifest",
  "restoreAffectedFleet",
  "cleanupRestore",
  "observeRestoreCleanup",
  "cleanupSnapshot",
  "observeSnapshotCleanup",
  "verifyBaseline",
]);
const RUN_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const SHA256 = /^[0-9a-f]{64}$/;

export class RecoveryCleanupError extends Error {
  constructor(operationError, cleanupErrors) {
    super(`recovery cleanup failed after ${operationError?.message ?? "campaign failure"}: ${cleanupErrors.join(";")}`);
    this.name = "RecoveryCleanupError";
    this.operationError = operationError;
    this.cleanupErrors = [...cleanupErrors];
  }
}

function canonicalJson(value) {
  if (value === null || typeof value === "boolean" || typeof value === "string") return JSON.stringify(value);
  if (typeof value === "number" && Number.isFinite(value)) return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value && typeof value === "object") return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
  throw new Error("recovery evidence contains a non-JSON value");
}

function sha256(value) { return createHash("sha256").update(value).digest("hex"); }

function object(value, name) {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`${name} must be an object`);
  return value;
}

function exactObject(value, fields, name) {
  const observed = object(value, name);
  const unknown = Object.keys(observed).filter((field) => !fields.includes(field));
  if (unknown.length > 0) throw new Error(`${name} contains unknown fields: ${unknown.join(",")}`);
  if (Object.keys(observed).length !== fields.length || fields.some((field) => !Object.hasOwn(observed, field))) throw new Error(`${name} must contain the exact field inventory`);
  return observed;
}

function string(value, name) {
  if (typeof value !== "string" || value.length === 0) throw new Error(`${name} must be a non-empty string`);
  return value;
}

function digest(value, name) {
  if (!SHA256.test(value ?? "")) throw new Error(`${name} must be a lowercase SHA-256 digest`);
  return value;
}

function timestamp(value, name) {
  if (!Number.isSafeInteger(value) || value < 0) throw new Error(`${name} must be a non-negative integer timestamp`);
  return value;
}

function exactStringArray(value, name) {
  if (!Array.isArray(value) || value.length < 1 || value.some((entry) => typeof entry !== "string" || entry.length === 0) || new Set(value).size !== value.length) throw new Error(`${name} must contain unique non-empty strings`);
  return [...value];
}

function sameStrings(left, right) {
  return left.length === right.length && left.every((value) => right.includes(value));
}

function validateAdapter(adapter) {
  for (const method of REQUIRED_ADAPTER_METHODS) if (typeof adapter?.[method] !== "function") throw new Error(`recovery adapter.${method} is required`);
}

function writerObservation(value, expected, name) {
  const observation = exactObject(value, ["stopped"], name);
  if (observation.stopped !== expected) throw new Error(`${name} did not prove the expected writer state`);
}

function authorityObservation(value, expected, name) {
  const observation = exactObject(value, ["exclusive", "authority_id"], name);
  if (observation.exclusive !== expected || (expected ? typeof observation.authority_id !== "string" || observation.authority_id.length === 0 : observation.authority_id !== null)) throw new Error(`${name} did not prove the expected exclusive authority state`);
  return observation;
}

function fleetObservation(value, unavailable, name) {
  const observation = exactObject(value, ["affected_fleet_unavailable", "external_evidence_store_reachable"], name);
  if (observation.affected_fleet_unavailable !== unavailable || observation.external_evidence_store_reachable !== true) throw new Error(`${name} did not prove affected-fleet isolation and external-store reachability`);
  return observation;
}

export async function executeRecoveryCampaign({ runId, adapter }) {
  if (!RUN_ID.test(runId ?? "")) throw new Error("recovery runId is invalid");
  validateAdapter(adapter);
  const baseline = exactObject(await adapter.captureBaseline(), ["baseline_sha256"], "recovery baseline");
  digest(baseline.baseline_sha256, "recovery baseline digest");
  const stores = exactObject(await adapter.observeStores(), ["affected_fleet_store_id", "external_evidence_store_id"], "recovery store authorities");
  string(stores.affected_fleet_store_id, "affected fleet store identity");
  string(stores.external_evidence_store_id, "external evidence store identity");
  if (stores.affected_fleet_store_id === stores.external_evidence_store_id) throw new Error("recovery evidence store must be independent of the affected fleet store");

  const timeline = [];
  const restores = [];
  const runbooks = [];
  const artifacts = [];
  const snapshotVersions = new Set();
  const restorePrefixes = new Set();
  const createdSnapshots = [];
  const startedRestores = [];
  let intentSequence = 0;
  let mutationStarted = false;
  let writersStopped = false;
  let authorityActive = false;
  let affectedFleetUnavailable = false;
  let activeRunbook = null;

  const persist = async (action, target) => {
    intentSequence += 1;
    const intent = { schema_version: "agentic-sandbox.celld-recovery-intent/v1", run_id: runId, sequence: intentSequence, action, target };
    const expectedDigest = sha256(canonicalJson(intent));
    const acknowledgment = exactObject(await adapter.persistIntent(structuredClone(intent)), ["intent_sha256", "persisted"], `recovery intent ${intentSequence} acknowledgment`);
    if (acknowledgment.persisted !== true || digest(acknowledgment.intent_sha256, `recovery intent ${intentSequence} digest`) !== expectedDigest) throw new Error(`recovery ${action} intent was not durably acknowledged`);
    timeline.push({ sequence: timeline.length + 1, phase: "intent_persisted", intent, intent_sha256: expectedDigest });
  };

  const cleanupRestore = async (execution, action = "cleanup_restore") => {
    await persist(action, `restore:${execution}`);
    await adapter.cleanupRestore(execution);
    const observation = exactObject(await adapter.observeRestoreCleanup(execution), ["execution", "removed"], `restore ${execution} cleanup`);
    if (observation.execution !== execution || observation.removed !== true) throw new Error(`restore ${execution} cleanup was not verified`);
    const index = startedRestores.lastIndexOf(execution);
    if (index >= 0) startedRestores.splice(index, 1);
  };

  const cleanupSnapshot = async (execution, action = "cleanup_snapshot") => {
    await persist(action, `snapshot:${execution}`);
    await adapter.cleanupSnapshot(execution);
    const observation = exactObject(await adapter.observeSnapshotCleanup(execution), ["execution", "removed"], `snapshot ${execution} cleanup`);
    if (observation.execution !== execution || observation.removed !== true) throw new Error(`snapshot ${execution} cleanup was not verified`);
    const index = createdSnapshots.lastIndexOf(execution);
    if (index >= 0) createdSnapshots.splice(index, 1);
  };

  try {
    await persist("stop_source_writers", "source");
    await adapter.stopSourceWriters();
    mutationStarted = true;
    writersStopped = true;
    writerObservation(await adapter.observeSourceWriters(), true, "stopped source writers");

    await persist("acquire_restore_authority", "source");
    await adapter.acquireRestoreAuthority();
    authorityActive = true;
    authorityObservation(await adapter.observeRestoreAuthority(), true, "active restore authority");

    for (const execution of [1, 2]) {
      await persist("create_versioned_snapshot", `snapshot:${execution}`);
      createdSnapshots.push(execution);
      await adapter.createSnapshot(execution);
      const snapshot = exactObject(
        await adapter.observeSnapshot(execution),
        ["execution", "snapshot_version_id", "source_prefix", "latest_acknowledged_at_ms", "snapshot_captured_at_ms", "generation_manifest_sha256", "tombstone_manifest_sha256"],
        `snapshot ${execution}`,
      );
      if (snapshot.execution !== execution) throw new Error(`snapshot ${execution} execution identity is wrong`);
      string(snapshot.snapshot_version_id, `snapshot ${execution} version identity`);
      string(snapshot.source_prefix, `snapshot ${execution} source prefix`);
      digest(snapshot.generation_manifest_sha256, `snapshot ${execution} generation manifest`);
      digest(snapshot.tombstone_manifest_sha256, `snapshot ${execution} tombstone manifest`);
      const latestAcknowledgedAt = timestamp(snapshot.latest_acknowledged_at_ms, `snapshot ${execution} latest acknowledgment`);
      const snapshotCapturedAt = timestamp(snapshot.snapshot_captured_at_ms, `snapshot ${execution} capture`);
      if (snapshotVersions.has(snapshot.snapshot_version_id) || snapshotCapturedAt < latestAcknowledgedAt || snapshotCapturedAt - latestAcknowledgedAt > 300_000) throw new Error(`snapshot ${execution} is not a distinct version within the RPO objective`);
      snapshotVersions.add(snapshot.snapshot_version_id);

      await persist("restore_snapshot", `restore:${execution}`);
      startedRestores.push(execution);
      await adapter.restoreSnapshot(execution, snapshot.snapshot_version_id);
      const restored = exactObject(
        await adapter.observeRestore(execution),
        ["execution", "snapshot_version_id", "source_prefix", "restore_prefix", "restore_started_at_ms", "restore_ready_at_ms", "isolated_restore", "quarantined", "source_writers_stopped", "restore_authority_exclusive", "generation_manifest_sha256", "tombstone_manifest_sha256"],
        `restore ${execution}`,
      );
      const restoreStartedAt = timestamp(restored.restore_started_at_ms, `restore ${execution} start`);
      const restoreReadyAt = timestamp(restored.restore_ready_at_ms, `restore ${execution} ready`);
      string(restored.restore_prefix, `restore ${execution} prefix`);
      if (restored.execution !== execution || restored.snapshot_version_id !== snapshot.snapshot_version_id || restored.source_prefix !== snapshot.source_prefix || restored.restore_prefix === snapshot.source_prefix || restorePrefixes.has(restored.restore_prefix)
          || restored.isolated_restore !== true || restored.quarantined !== true || restored.source_writers_stopped !== true || restored.restore_authority_exclusive !== true
          || restoreReadyAt < restoreStartedAt || restoreReadyAt - restoreStartedAt > 1_800_000
          || digest(restored.generation_manifest_sha256, `restore ${execution} generation manifest`) !== snapshot.generation_manifest_sha256
          || digest(restored.tombstone_manifest_sha256, `restore ${execution} tombstone manifest`) !== snapshot.tombstone_manifest_sha256) {
        throw new Error(`restore ${execution} did not prove isolation, authority, RTO, and complete state`);
      }
      restorePrefixes.add(restored.restore_prefix);
      restores.push({
        execution,
        snapshot_version_id: snapshot.snapshot_version_id,
        source_prefix: snapshot.source_prefix,
        restore_prefix: restored.restore_prefix,
        isolated_restore: true,
        quarantined: true,
        source_writers_stopped: true,
        restore_authority_exclusive: true,
        latest_acknowledged_at_ms: latestAcknowledgedAt,
        snapshot_captured_at_ms: snapshotCapturedAt,
        restore_started_at_ms: restoreStartedAt,
        restore_ready_at_ms: restoreReadyAt,
        generation_manifest_before_sha256: snapshot.generation_manifest_sha256,
        generation_manifest_after_sha256: restored.generation_manifest_sha256,
        tombstone_manifest_before_sha256: snapshot.tombstone_manifest_sha256,
        tombstone_manifest_after_sha256: restored.tombstone_manifest_sha256,
      });
    }

    await persist("release_restore_authority", "source");
    await adapter.releaseRestoreAuthority();
    authorityObservation(await adapter.observeRestoreAuthority(), false, "released restore authority");
    authorityActive = false;
    await persist("start_source_writers", "source");
    await adapter.startSourceWriters();
    writerObservation(await adapter.observeSourceWriters(), false, "restarted source writers");
    writersStopped = false;

    for (const runbook of RECOVERY_RUNBOOKS) {
      const executions = [];
      for (const ordinal of [1, 2]) {
        await persist("execute_runbook", `${runbook}:${ordinal}`);
        activeRunbook = runbook;
        await adapter.executeRunbook(runbook, ordinal);
        const observation = exactObject(
          await adapter.observeRunbook(runbook, ordinal),
          ["runbook", "ordinal", "operation_ids", "lifecycle_effect_ids", "state_sha256_after", "healed", "cleanup_verified"],
          `${runbook} runbook execution ${ordinal}`,
        );
        const operationIds = exactStringArray(observation.operation_ids, `${runbook} execution ${ordinal} operation ids`);
        const effectIds = exactStringArray(observation.lifecycle_effect_ids, `${runbook} execution ${ordinal} effect ids`);
        if (observation.runbook !== runbook || observation.ordinal !== ordinal || observation.healed !== true || observation.cleanup_verified !== true) throw new Error(`${runbook} execution ${ordinal} did not heal and clean up`);
        const state = digest(observation.state_sha256_after, `${runbook} execution ${ordinal} state`);
        executions.push({ ordinal, operation_ids: operationIds, lifecycle_effect_ids: effectIds, state_sha256_after: state });
        activeRunbook = null;
      }
      if (!sameStrings(executions[0].operation_ids, executions[1].operation_ids) || !sameStrings(executions[0].lifecycle_effect_ids, executions[1].lifecycle_effect_ids) || executions[0].state_sha256_after !== executions[1].state_sha256_after) throw new Error(`${runbook} second execution created additional or divergent effects`);
      runbooks.push({ runbook, executions, healed: true, cleanup_verified: true });
    }

    for (const kind of RECOVERY_EVIDENCE_KINDS) {
      await persist("upload_external_evidence", kind);
      await adapter.uploadEvidence(kind, stores.external_evidence_store_id);
      const uploaded = exactObject(await adapter.observeEvidenceUpload(kind), ["kind", "storage_authority_id", "bytes", "sha256", "retained"], `${kind} evidence upload`);
      if (uploaded.kind !== kind || uploaded.storage_authority_id !== stores.external_evidence_store_id || !Number.isSafeInteger(uploaded.bytes) || uploaded.bytes < 1 || uploaded.retained !== true) throw new Error(`${kind} evidence was not retained by the external authority`);
      artifacts.push({ kind, storage_authority_id: uploaded.storage_authority_id, bytes: uploaded.bytes, sha256: digest(uploaded.sha256, `${kind} evidence digest`), retained: true });
    }

    await persist("make_affected_fleet_unavailable", "affected_fleet");
    await adapter.makeAffectedFleetUnavailable();
    affectedFleetUnavailable = true;
    const unavailable = fleetObservation(await adapter.observeFleetAvailability(), true, "affected fleet loss observation");

    for (const artifact of artifacts) {
      const downloaded = exactObject(await adapter.readExternalEvidence(artifact.kind), ["kind", "downloaded_sha256", "read_after_fleet_loss"], `${artifact.kind} evidence download`);
      if (downloaded.kind !== artifact.kind || downloaded.read_after_fleet_loss !== true || digest(downloaded.downloaded_sha256, `${artifact.kind} downloaded digest`) !== artifact.sha256) throw new Error(`${artifact.kind} evidence was not independently readable after fleet loss`);
      const corruption = exactObject(await adapter.probeEvidenceCorruption(artifact.kind), ["kind", "tampered_sha256", "detected"], `${artifact.kind} corruption probe`);
      const tampered = digest(corruption.tampered_sha256, `${artifact.kind} tampered digest`);
      if (corruption.kind !== artifact.kind || tampered === artifact.sha256 || corruption.detected !== true) throw new Error(`${artifact.kind} corruption probe did not fail closed`);
      artifact.downloaded_sha256 = downloaded.downloaded_sha256;
      artifact.corruption_probe = { tampered_sha256: tampered, detected: true };
      artifact.read_after_fleet_loss = true;
    }
    const manifest = exactObject(await adapter.verifyEvidenceManifest(), ["manifest_verified", "malicious_runner_tamper_proof_claimed"], "external evidence manifest");
    if (manifest.manifest_verified !== true || manifest.malicious_runner_tamper_proof_claimed !== false) throw new Error("external evidence manifest is invalid or overclaims its trust boundary");

    await persist("restore_affected_fleet", "affected_fleet");
    await adapter.restoreAffectedFleet();
    fleetObservation(await adapter.observeFleetAvailability(), false, "restored affected fleet observation");
    affectedFleetUnavailable = false;
    for (const execution of [...startedRestores].reverse()) await cleanupRestore(execution);
    for (const execution of [...createdSnapshots].reverse()) await cleanupSnapshot(execution);

    const restoredBaseline = exactObject(await adapter.verifyBaseline(structuredClone(baseline)), ["baseline_sha256", "restored"], "recovery restored baseline");
    if (digest(restoredBaseline.baseline_sha256, "recovery restored baseline digest") !== baseline.baseline_sha256 || restoredBaseline.restored !== true) throw new Error("recovery campaign did not restore its preflight baseline");
    return {
      mutation_started: mutationStarted,
      restores,
      runbooks,
      evidence: {
        affected_fleet_store_id: stores.affected_fleet_store_id,
        external_evidence_store_id: stores.external_evidence_store_id,
        artifacts,
        affected_fleet_unavailable: unavailable.affected_fleet_unavailable,
        external_evidence_store_reachable: unavailable.external_evidence_store_reachable,
        manifest_verified: true,
        malicious_runner_tamper_proof_claimed: false,
      },
      baseline: { baseline_sha256: baseline.baseline_sha256, restored: true },
      timeline,
      cleanup: { status: "passed", active_restore_authority: false, source_writers_stopped: false, affected_fleet_unavailable: false, restore_fixtures_removed: true, snapshot_resources_removed: true, external_evidence_retained: true },
    };
  } catch (operationError) {
    const cleanupErrors = [];
    if (activeRunbook !== null) {
      try {
        await persist("emergency_cleanup_runbook", activeRunbook);
        const cleanup = exactObject(await adapter.cleanupRunbook(activeRunbook), ["runbook", "healed", "cleanup_verified"], `${activeRunbook} emergency runbook cleanup`);
        if (cleanup.runbook !== activeRunbook || cleanup.healed !== true || cleanup.cleanup_verified !== true) throw new Error("runbook cleanup was not verified");
      } catch (cleanupError) { cleanupErrors.push(`runbook:${cleanupError.message}`); }
    }
    if (affectedFleetUnavailable) {
      try {
        await persist("emergency_restore_affected_fleet", "affected_fleet");
        await adapter.restoreAffectedFleet();
        fleetObservation(await adapter.observeFleetAvailability(), false, "emergency affected fleet restoration");
        affectedFleetUnavailable = false;
      } catch (cleanupError) { cleanupErrors.push(`affected_fleet:${cleanupError.message}`); }
    }
    for (const execution of [...startedRestores].reverse()) {
      try { await cleanupRestore(execution, "emergency_cleanup_restore"); }
      catch (cleanupError) { cleanupErrors.push(`restore:${execution}:${cleanupError.message}`); }
    }
    for (const execution of [...createdSnapshots].reverse()) {
      try { await cleanupSnapshot(execution, "emergency_cleanup_snapshot"); }
      catch (cleanupError) { cleanupErrors.push(`snapshot:${execution}:${cleanupError.message}`); }
    }
    if (authorityActive) {
      try {
        await persist("emergency_release_restore_authority", "source");
        await adapter.releaseRestoreAuthority();
        authorityObservation(await adapter.observeRestoreAuthority(), false, "emergency authority release");
        authorityActive = false;
      } catch (cleanupError) { cleanupErrors.push(`authority:${cleanupError.message}`); }
    }
    if (writersStopped) {
      try {
        await persist("emergency_start_source_writers", "source");
        await adapter.startSourceWriters();
        writerObservation(await adapter.observeSourceWriters(), false, "emergency source writer restart");
        writersStopped = false;
      } catch (cleanupError) { cleanupErrors.push(`writers:${cleanupError.message}`); }
    }
    try {
      const restored = exactObject(await adapter.verifyBaseline(structuredClone(baseline)), ["baseline_sha256", "restored"], "recovery emergency baseline verification");
      if (restored.baseline_sha256 !== baseline.baseline_sha256 || restored.restored !== true) throw new Error("campaign baseline did not match preflight");
    } catch (cleanupError) { cleanupErrors.push(`baseline:${cleanupError.message}`); }
    if (cleanupErrors.length > 0) throw new RecoveryCleanupError(operationError, cleanupErrors);
    throw operationError;
  }
}
