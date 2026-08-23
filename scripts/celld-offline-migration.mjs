import { createHash } from "node:crypto";

const SCOPE = "celld_object_store_only";
const REQUIRED_WRITER_CLASSES = Object.freeze(["celld_nodes", "deployment_cli", "management_reconciler", "worker_alarms"]);

function sha256(value) { return createHash("sha256").update(value).digest("hex"); }
function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;
  return JSON.stringify(value);
}

function validateEntry(entry, context) {
  if (!entry || typeof entry.key !== "string" || entry.key.length === 0 || entry.key.startsWith("/") || entry.key.includes("..")) throw new Error(`${context} contains an unsafe key`);
  if (!Number.isSafeInteger(entry.size) || entry.size < 0 || !entry.metadata || typeof entry.metadata !== "object" || Array.isArray(entry.metadata)) throw new Error(`${context} contains invalid object metadata`);
  return { key: entry.key, size: entry.size, metadata: entry.metadata };
}

async function stableListing(store, phase) {
  const first = (await store.list()).map((entry) => validateEntry(entry, phase)).sort((a, b) => a.key.localeCompare(b.key));
  const second = (await store.list()).map((entry) => validateEntry(entry, phase)).sort((a, b) => a.key.localeCompare(b.key));
  if (new Set(first.map((entry) => entry.key)).size !== first.length || new Set(second.map((entry) => entry.key)).size !== second.length) throw new Error(`${phase} listing contains duplicate keys`);
  if (canonical(first) !== canonical(second)) throw new Error(`${phase} listing changed while writers were quiesced`);
  return first;
}

async function contentManifest(store, listing, phase) {
  const result = [];
  for (const entry of listing) {
    const object = await store.get(entry.key);
    const body = Buffer.isBuffer(object?.body) ? object.body : Buffer.from(object?.body ?? "");
    if (body.length !== entry.size) throw new Error(`${phase} object size changed for ${entry.key}`);
    if (canonical(object.metadata ?? {}) !== canonical(entry.metadata)) throw new Error(`${phase} metadata changed for ${entry.key}`);
    result.push({ key: entry.key, size: entry.size, content_sha256: sha256(body), metadata_sha256: sha256(canonical(entry.metadata)) });
  }
  return result;
}

async function storeFingerprint(store, phase) {
  const listing = await stableListing(store, phase);
  const manifest = await contentManifest(store, listing, phase);
  return manifestEvidence(manifest);
}

function manifestEvidence(manifest) {
  const keySet = manifest.map((entry) => entry.key);
  const sizes = manifest.map(({ key, size }) => ({ key, size }));
  const content = manifest.map(({ key, content_sha256 }) => ({ key, content_sha256 }));
  const metadata = manifest.map(({ key, metadata_sha256 }) => ({ key, metadata_sha256 }));
  return {
    objects: manifest.length,
    bytes: manifest.reduce((sum, entry) => sum + entry.size, 0),
    key_set_sha256: sha256(canonical(keySet)),
    size_manifest_sha256: sha256(canonical(sizes)),
    content_manifest_sha256: sha256(canonical(content)),
    metadata_manifest_sha256: sha256(canonical(metadata)),
    manifest_sha256: sha256(canonical(manifest)),
  };
}

async function synchronize(source, target, sourceListing, phase) {
  const sourceKeys = new Set(sourceListing.map((entry) => entry.key));
  const targetListing = await target.list();
  for (const entry of targetListing) if (!sourceKeys.has(entry.key)) await target.delete(entry.key);
  for (const entry of sourceListing) {
    const object = await source.get(entry.key);
    await target.put(entry.key, object.body, entry.metadata);
  }
  const sourceManifest = await contentManifest(source, sourceListing, `${phase} source`);
  const targetStable = await stableListing(target, `${phase} target`);
  const targetManifest = await contentManifest(target, targetStable, `${phase} target`);
  if (canonical(sourceManifest) !== canonical(targetManifest)) throw new Error(`${phase} key, size, metadata, or content comparison failed`);
  return manifestEvidence(sourceManifest);
}

async function assertNoWriters(control, phase) {
  const writers = await control.listWriters();
  if (!Array.isArray(writers) || writers.length === 0 || writers.some((writer) => !writer || typeof writer.class !== "string" || typeof writer.running !== "boolean" || typeof writer.observed_at !== "string" || typeof writer.observation_source !== "string")) throw new Error(`${phase} writer inventory is incomplete`);
  const classes = writers.map((writer) => writer.class);
  if (new Set(classes).size !== classes.length || classes.length !== REQUIRED_WRITER_CLASSES.length || REQUIRED_WRITER_CLASSES.some((required) => !classes.includes(required))) throw new Error(`${phase} writer inventory is incomplete`);
  if (writers.some((writer) => writer.running)) throw new Error(`${phase} did not stop every writer class`);
  return writers.sort((left, right) => left.class.localeCompare(right.class));
}

async function assertSingleAuthority(control, expected, phase, authorityIds) {
  const observations = await control.observeAuthorities();
  if (!Array.isArray(observations) || observations.length !== authorityIds.length) throw new Error(`${phase} authority observation is incomplete`);
  const ids = observations.map((entry) => entry?.id);
  if (new Set(ids).size !== authorityIds.length || authorityIds.some((id) => !ids.includes(id))) throw new Error(`${phase} authority observation is incomplete`);
  for (const entry of observations) {
    if (typeof entry.policy_writable !== "boolean" || typeof entry.fleet_running !== "boolean" || typeof entry.observed_at !== "string" || !/^[0-9a-f]{64}$/.test(entry.policy_sha256 ?? "")
        || !Array.isArray(entry.running_writer_classes) || entry.running_writer_classes.some((writerClass) => !REQUIRED_WRITER_CLASSES.includes(writerClass))) throw new Error(`${phase} authority observation is invalid`);
    const active = entry.policy_writable || entry.fleet_running || entry.running_writer_classes.length > 0;
    if (expected === null ? active : entry.id === expected ? !entry.policy_writable || !entry.fleet_running : active) throw new Error(`${phase} violated the single-authority invariant`);
  }
  return observations.sort((left, right) => left.id.localeCompare(right.id));
}

async function journaledMutation(control, timeline, phase, mutation, details, action) {
  if (typeof control.planMigrationMutation !== "function" || typeof control.completeMigrationMutation !== "function") throw new Error("migration control lacks a durable mutation journal");
  const planned = await control.planMigrationMutation({ phase, mutation, details });
  if (!planned || planned.status !== "planned" || planned.phase !== phase || planned.mutation !== mutation || !/^[0-9a-f]{64}$/.test(planned.entry_sha256 ?? "")) throw new Error("migration mutation was not durably planned");
  timeline.push({ phase: "journal", event: "planned", journal: planned });
  const result = await action();
  const completed = await control.completeMigrationMutation(planned.id, { result: result ?? null, result_sha256: sha256(canonical(result ?? null)) });
  if (!completed || completed.status !== "completed" || completed.plan_id !== planned.id || !/^[0-9a-f]{64}$/.test(completed.entry_sha256 ?? "")) throw new Error("migration mutation completion was not durably recorded");
  timeline.push({ phase: "journal", event: "completed", journal: completed });
  return result;
}

export async function rehearseOfflineMigration({ source, destination, control }) {
  if (!source || !destination || !control || source.id === destination.id) throw new Error("offline migration requires distinct source and destination stores");
  if (source.scope !== SCOPE || destination.scope !== SCOPE) throw new Error("offline migration is limited to Celld object-store state");
  const timeline = [];
  const authorityIds = [source.id, destination.id].sort();
  let phase = "initial";
  try {
    await journaledMutation(control, timeline, "forward_quiescence", "stop_all_writers", { authority_ids: authorityIds }, () => control.stopAllWriters());
    await journaledMutation(control, timeline, "forward_quiescence", "deny_all_application_writes", { authority_ids: authorityIds }, () => control.setApplicationAuthority(null));
    const writerClasses = await assertNoWriters(control, "forward quiescence");
    const forwardQuiescenceAuthorities = await assertSingleAuthority(control, null, "forward quiescence", authorityIds);
    if (await control.probeWriteDenied(source.id) !== true) throw new Error("source application writes were not denied before copy");
    const sourceListing = await stableListing(source, "forward source");
    const forward = await journaledMutation(control, timeline, "forward_copy", "synchronize_namespace", { source_id: source.id, destination_id: destination.id, objects: sourceListing.length }, () => synchronize(source, destination, sourceListing, "forward migration"));
    timeline.push({ phase: "forward_copy", ...forward, writer_classes: writerClasses, authority_observations: forwardQuiescenceAuthorities });

    phase = "destination_canary";
    await journaledMutation(control, timeline, phase, "activate_destination_canary", { authority_id: destination.id }, () => control.setApplicationAuthority(destination.id));
    const destinationCanaryAuthorities = await assertSingleAuthority(control, destination.id, phase, authorityIds);
    if (await control.runCanary(destination.id) !== true) throw new Error("destination canary failed");
    await journaledMutation(control, timeline, phase, "stop_destination_canary", { authority_id: destination.id }, () => control.setApplicationAuthority(null));
    timeline.push({ phase, authority_observations: destinationCanaryAuthorities, canary: "passed" });

    phase = "pre_write_rollback";
    await journaledMutation(control, timeline, phase, "activate_source_rollback", { authority_id: source.id }, () => control.setApplicationAuthority(source.id));
    const preWriteRollbackAuthorities = await assertSingleAuthority(control, source.id, phase, authorityIds);
    if (await control.runCanary(source.id) !== true) throw new Error("pre-write direct rollback canary failed");
    await journaledMutation(control, timeline, phase, "stop_source_rollback", { authority_id: source.id }, () => control.setApplicationAuthority(null));
    timeline.push({ phase, allowed: true, authority_observations: preWriteRollbackAuthorities });

    phase = "destination_cutover";
    const beforeApplicationWrite = await storeFingerprint(destination, "destination before application write");
    const cutover = await journaledMutation(control, timeline, phase, "record_cutover", { authority_id: destination.id, before_application_write: beforeApplicationWrite }, () => control.recordCutover(destination.id));
    await journaledMutation(control, timeline, phase, "activate_destination", { authority_id: destination.id, cutover_sha256: sha256(canonical(cutover)) }, () => control.setApplicationAuthority(destination.id));
    const destinationCutoverAuthorities = await assertSingleAuthority(control, destination.id, phase, authorityIds);
    if (await control.probeWriteDenied(source.id) !== true) throw new Error("source application writes were not denied during destination authority");
    const applicationWrite = await journaledMutation(control, timeline, phase, "create_post_cutover_application_write", { authority_id: destination.id }, () => control.createApplicationWrite(destination.id));
    if (applicationWrite !== true) throw new Error("destination application write was not recorded");
    await journaledMutation(control, timeline, phase, "stop_destination_for_reverse", { authority_id: destination.id }, () => control.setApplicationAuthority(null));
    const afterApplicationWrite = await storeFingerprint(destination, "destination after application write");
    if (afterApplicationWrite.manifest_sha256 === beforeApplicationWrite.manifest_sha256) throw new Error("destination application write did not change durable state");
    timeline.push({ phase, cutover, source_role: "recovery_snapshot", direct_rollback_allowed: false, authority_observations: destinationCutoverAuthorities, before_application_write: beforeApplicationWrite, after_application_write: afterApplicationWrite });

    phase = "reverse_quiescence";
    await assertNoWriters(control, phase);
    const reverseQuiescenceAuthorities = await assertSingleAuthority(control, null, phase, authorityIds);
    if (await control.probeWriteDenied(destination.id) !== true) throw new Error("destination application writes were not denied before reverse copy");
    const destinationListing = await stableListing(destination, "reverse source");
    const reverse = await journaledMutation(control, timeline, "reverse_copy", "synchronize_namespace", { source_id: destination.id, destination_id: source.id, objects: destinationListing.length }, () => synchronize(destination, source, destinationListing, "reverse migration"));
    timeline.push({ phase: "reverse_copy", ...reverse, authority_observations: reverseQuiescenceAuthorities });

    phase = "source_restore_canary";
    await journaledMutation(control, timeline, phase, "activate_reverse_migrated_source", { authority_id: source.id }, () => control.setApplicationAuthority(source.id));
    const sourceRestoreAuthorities = await assertSingleAuthority(control, source.id, phase, authorityIds);
    if (await control.runCanary(source.id) !== true) throw new Error("reverse-migrated source canary failed");
    await journaledMutation(control, timeline, phase, "stop_reverse_migrated_source", { authority_id: source.id }, () => control.setApplicationAuthority(null));
    timeline.push({ phase, authority_observations: sourceRestoreAuthorities, canary: "passed" });
    return { schema_version: "agentic-sandbox.celld-offline-migration-evidence/v1", scope: SCOPE, source_id: source.id, destination_id: destination.id, forward, reverse, cutover, authority_observations: timeline.filter((entry) => Array.isArray(entry.authority_observations)).flatMap((entry) => entry.authority_observations.map((observation) => ({ phase: entry.phase, ...observation }))), dual_authority_observed: false, local_storage_touched: false, timeline };
  } finally {
    await control.setApplicationAuthority(null);
    await control.stopAllWriters();
    await assertSingleAuthority(control, null, `${phase} cleanup`, authorityIds);
  }
}

export { SCOPE as CELLD_MIGRATION_SCOPE, REQUIRED_WRITER_CLASSES };
