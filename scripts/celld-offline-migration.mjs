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
  return {
    objects: manifest.length,
    bytes: manifest.reduce((sum, entry) => sum + entry.size, 0),
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
  return { objects: sourceManifest.length, bytes: sourceManifest.reduce((sum, entry) => sum + entry.size, 0), manifest_sha256: sha256(canonical(sourceManifest)) };
}

async function assertNoWriters(control, phase) {
  const writers = await control.listWriters();
  if (!Array.isArray(writers) || writers.length === 0 || writers.some((writer) => !writer || typeof writer.class !== "string" || typeof writer.running !== "boolean")) throw new Error(`${phase} writer inventory is incomplete`);
  const classes = writers.map((writer) => writer.class);
  if (new Set(classes).size !== classes.length || REQUIRED_WRITER_CLASSES.some((required) => !classes.includes(required))) throw new Error(`${phase} writer inventory is incomplete`);
  if (writers.some((writer) => writer.running)) throw new Error(`${phase} did not stop every writer class`);
  return classes.sort();
}

async function assertSingleAuthority(control, expected, phase) {
  const authorities = await control.activeAuthorities();
  if (!Array.isArray(authorities) || authorities.length > 1 || (expected === null ? authorities.length !== 0 : authorities.length !== 1 || authorities[0] !== expected)) throw new Error(`${phase} violated the single-authority invariant`);
}

export async function rehearseOfflineMigration({ source, destination, control }) {
  if (!source || !destination || !control || source.id === destination.id) throw new Error("offline migration requires distinct source and destination stores");
  if (source.scope !== SCOPE || destination.scope !== SCOPE) throw new Error("offline migration is limited to Celld object-store state");
  const timeline = [];
  let phase = "initial";
  try {
    await control.stopAllWriters();
    await control.setApplicationAuthority(null);
    const writerClasses = await assertNoWriters(control, "forward quiescence");
    await assertSingleAuthority(control, null, "forward quiescence");
    if (await control.probeWriteDenied(source.id) !== true) throw new Error("source application writes were not denied before copy");
    const sourceListing = await stableListing(source, "forward source");
    const forward = await synchronize(source, destination, sourceListing, "forward migration");
    timeline.push({ phase: "forward_copy", ...forward, writer_classes: writerClasses });

    phase = "destination_canary";
    await control.setApplicationAuthority(destination.id);
    await assertSingleAuthority(control, destination.id, phase);
    if (await control.runCanary(destination.id) !== true) throw new Error("destination canary failed");
    await control.setApplicationAuthority(null);

    phase = "pre_write_rollback";
    await control.setApplicationAuthority(source.id);
    await assertSingleAuthority(control, source.id, phase);
    if (await control.runCanary(source.id) !== true) throw new Error("pre-write direct rollback canary failed");
    await control.setApplicationAuthority(null);
    timeline.push({ phase, allowed: true });

    phase = "destination_cutover";
    const beforeApplicationWrite = await storeFingerprint(destination, "destination before application write");
    const cutover = await control.recordCutover(destination.id);
    await control.setApplicationAuthority(destination.id);
    await assertSingleAuthority(control, destination.id, phase);
    if (await control.probeWriteDenied(source.id) !== true) throw new Error("source application writes were not denied during destination authority");
    if (await control.createApplicationWrite(destination.id) !== true) throw new Error("destination application write was not recorded");
    await control.setApplicationAuthority(null);
    const afterApplicationWrite = await storeFingerprint(destination, "destination after application write");
    if (afterApplicationWrite.manifest_sha256 === beforeApplicationWrite.manifest_sha256) throw new Error("destination application write did not change durable state");
    timeline.push({ phase, cutover, source_role: "recovery_snapshot", direct_rollback_allowed: false, before_application_write: beforeApplicationWrite, after_application_write: afterApplicationWrite });

    phase = "reverse_quiescence";
    await assertNoWriters(control, phase);
    await assertSingleAuthority(control, null, phase);
    if (await control.probeWriteDenied(destination.id) !== true) throw new Error("destination application writes were not denied before reverse copy");
    const destinationListing = await stableListing(destination, "reverse source");
    const reverse = await synchronize(destination, source, destinationListing, "reverse migration");
    timeline.push({ phase: "reverse_copy", ...reverse });

    phase = "source_restore_canary";
    await control.setApplicationAuthority(source.id);
    await assertSingleAuthority(control, source.id, phase);
    if (await control.runCanary(source.id) !== true) throw new Error("reverse-migrated source canary failed");
    await control.setApplicationAuthority(null);
    return { schema_version: "agentic-sandbox.celld-offline-migration-evidence/v1", scope: SCOPE, source_id: source.id, destination_id: destination.id, forward, reverse, cutover, dual_authority_observed: false, local_storage_touched: false, timeline };
  } finally {
    await control.setApplicationAuthority(null);
    await control.stopAllWriters();
    await assertSingleAuthority(control, null, `${phase} cleanup`);
  }
}

export { SCOPE as CELLD_MIGRATION_SCOPE, REQUIRED_WRITER_CLASSES };
