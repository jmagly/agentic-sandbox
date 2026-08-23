import assert from "node:assert/strict";
import test from "node:test";

import { CELLD_MIGRATION_SCOPE, REQUIRED_WRITER_CLASSES, rehearseOfflineMigration } from "../../../scripts/celld-offline-migration.mjs";

class MemoryStore {
  constructor(id, entries = {}) { this.id = id; this.scope = CELLD_MIGRATION_SCOPE; this.entries = new Map(Object.entries(entries).map(([key, value]) => [key, { body: Buffer.from(value.body), metadata: value.metadata ?? {} }])); }
  async list() { return [...this.entries].map(([key, value]) => ({ key, size: value.body.length, metadata: value.metadata })); }
  async get(key) { const value = this.entries.get(key); return value && { body: Buffer.from(value.body), metadata: structuredClone(value.metadata) }; }
  async put(key, body, metadata) { this.entries.set(key, { body: Buffer.from(body), metadata: structuredClone(metadata) }); }
  async delete(key) { this.entries.delete(key); }
}

class Control {
  constructor(source, destination) { this.source = source; this.destination = destination; this.authority = null; this.writers = ["celld_nodes", "deployment_cli", "management_reconciler", "worker_alarms"].map((value) => ({ class: value, running: true })); this.cutovers = 0; this.journal = []; }
  async stopAllWriters() { this.writers.forEach((writer) => { writer.running = false; }); }
  async listWriters() { return this.writers.map((writer) => ({ ...writer, observed_at: new Date().toISOString(), observation_source: "memory-control" })); }
  async setApplicationAuthority(value) { this.authority = value; this.writers.forEach((writer) => { writer.running = value !== null && ["celld_nodes", "worker_alarms"].includes(writer.class); }); }
  async activeAuthorities() { return this.authority ? [this.authority] : []; }
  async observeAuthorities() {
    return [this.source.id, this.destination.id].map((id) => ({
      id,
      policy_writable: this.authority === id,
      fleet_running: this.authority === id,
      running_writer_classes: this.authority === id ? ["celld_nodes", "worker_alarms"] : [],
      observed_at: new Date().toISOString(),
      policy_sha256: "a".repeat(64),
    }));
  }
  async planMigrationMutation({ phase, mutation, details }) {
    const entry = { id: `plan-${this.journal.length + 1}`, status: "planned", phase, mutation, details, entry_sha256: "b".repeat(64) };
    this.journal.push(entry); return entry;
  }
  async completeMigrationMutation(planId, details) {
    const entry = { id: `completion-${this.journal.length + 1}`, plan_id: planId, status: "completed", details, entry_sha256: "c".repeat(64) };
    this.journal.push(entry); return entry;
  }
  async probeWriteDenied(id) { return this.authority !== id; }
  async runCanary(id) { return this.authority === id; }
  async recordCutover(id) { this.cutovers += 1; return `cutover-${this.cutovers}-${id}`; }
  async createApplicationWrite(id) { if (this.authority !== id) return false; await this.destination.put("fleet/cells/new-generation", "generation-2", { generation: "2" }); return true; }
}

function stores() {
  const source = new MemoryStore("source", {
    "fleet/deployments/current": { body: "bundle-v1", metadata: { kind: "deployment" } },
    "fleet/cells/instance-a": { body: "generation-1", metadata: { generation: "1" } },
    "fleet/peer-secret": { body: "opaque-peer-material", metadata: { kind: "reserved" } },
  });
  return { source, destination: new MemoryStore("destination", { "stale/object": { body: "remove-me" } }) };
}

test("offline forward and reverse migration preserves all Celld bytes and metadata", async () => {
  const { source, destination } = stores(), control = new Control(source, destination);
  const evidence = await rehearseOfflineMigration({ source, destination, control });
  assert.equal(evidence.scope, CELLD_MIGRATION_SCOPE);
  assert.equal(evidence.forward.objects, 3);
  assert.equal(evidence.reverse.objects, 4);
  for (const result of [evidence.forward, evidence.reverse]) {
    for (const field of ["key_set_sha256", "size_manifest_sha256", "content_manifest_sha256", "metadata_manifest_sha256", "manifest_sha256"]) assert.match(result[field], /^[0-9a-f]{64}$/);
  }
  const cutover = evidence.timeline.find((entry) => entry.phase === "destination_cutover");
  assert.equal(cutover.direct_rollback_allowed, false);
  assert.notEqual(cutover.before_application_write.manifest_sha256, cutover.after_application_write.manifest_sha256);
  assert.equal(evidence.dual_authority_observed, false);
  assert.ok(evidence.authority_observations.length >= 10);
  assert.ok(control.journal.some((entry) => entry.status === "planned" && entry.mutation === "record_cutover"));
  assert.equal(evidence.local_storage_touched, false);
  const byKey = (left, right) => left.key.localeCompare(right.key);
  assert.deepEqual((await source.list()).sort(byKey), (await destination.list()).sort(byKey));
  assert.equal(control.authority, null);
});

test("migration fails closed when repeated quiesced listings differ", async () => {
  const { source, destination } = stores(), control = new Control(source, destination);
  const stable = source.list.bind(source); let calls = 0;
  source.list = async () => { const result = await stable(); calls += 1; if (calls === 2) result.push({ key: "fleet/late-write", size: 1, metadata: {} }); return result; };
  await assert.rejects(rehearseOfflineMigration({ source, destination, control }), /listing changed while writers were quiesced/);
  assert.equal(control.authority, null);
});

test("migration refuses incomplete writer inventories and non-Celld scopes", async () => {
  const { source, destination } = stores(), control = new Control(source, destination);
  control.listWriters = async () => [];
  await assert.rejects(rehearseOfflineMigration({ source, destination, control }), /writer inventory is incomplete/);
  source.scope = "sandbox_local_volumes";
  await assert.rejects(rehearseOfflineMigration({ source, destination, control }), /limited to Celld object-store state/);
});

test("offline migration fixes the complete writer-class inventory", () => {
  assert.deepEqual(REQUIRED_WRITER_CLASSES, ["celld_nodes", "deployment_cli", "management_reconciler", "worker_alarms"]);
});

test("a dual-authority observation aborts before copying", async () => {
  const { source, destination } = stores(), control = new Control(source, destination);
  control.observeAuthorities = async () => [source.id, destination.id].map((id) => ({ id, policy_writable: true, fleet_running: true, running_writer_classes: ["celld_nodes", "worker_alarms"], observed_at: new Date().toISOString(), policy_sha256: "d".repeat(64) }));
  await assert.rejects(rehearseOfflineMigration({ source, destination, control }), /single-authority invariant/);
});

test("migration refuses a control without durable mutation journaling", async () => {
  const { source, destination } = stores(), control = new Control(source, destination);
  delete control.planMigrationMutation;
  control.planMigrationMutation = undefined;
  await assert.rejects(rehearseOfflineMigration({ source, destination, control }), /durable mutation journal/);
});

test("an interrupted copy leaves its exact durable mutation plan incomplete", async () => {
  const { source, destination } = stores(), control = new Control(source, destination);
  destination.put = async () => { throw new Error("injected copy interruption"); };
  await assert.rejects(rehearseOfflineMigration({ source, destination, control }), /injected copy interruption/);
  const plan = control.journal.find((entry) => entry.status === "planned" && entry.mutation === "synchronize_namespace");
  assert.ok(plan);
  assert.equal(control.journal.some((entry) => entry.status === "completed" && entry.plan_id === plan.id), false);
  assert.equal(control.authority, null);
  assert.ok(control.writers.every((writer) => writer.running === false));
});

test("a controller cannot claim an application write without changing durable state", async () => {
  const { source, destination } = stores(), control = new Control(source, destination);
  control.createApplicationWrite = async (id) => control.authority === id;
  await assert.rejects(rehearseOfflineMigration({ source, destination, control }), /did not change durable state/);
});
