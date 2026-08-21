import assert from "node:assert/strict";
import test from "node:test";

import { CELLD_MIGRATION_SCOPE, rehearseOfflineMigration } from "../../../scripts/celld-offline-migration.mjs";

class MemoryStore {
  constructor(id, entries = {}) { this.id = id; this.scope = CELLD_MIGRATION_SCOPE; this.entries = new Map(Object.entries(entries).map(([key, value]) => [key, { body: Buffer.from(value.body), metadata: value.metadata ?? {} }])); }
  async list() { return [...this.entries].map(([key, value]) => ({ key, size: value.body.length, metadata: value.metadata })); }
  async get(key) { const value = this.entries.get(key); return value && { body: Buffer.from(value.body), metadata: structuredClone(value.metadata) }; }
  async put(key, body, metadata) { this.entries.set(key, { body: Buffer.from(body), metadata: structuredClone(metadata) }); }
  async delete(key) { this.entries.delete(key); }
}

class Control {
  constructor(source, destination) { this.source = source; this.destination = destination; this.authority = null; this.writers = ["celld_nodes", "deployment_cli", "management_reconciler"].map((value) => ({ class: value, running: true })); this.cutovers = 0; }
  async stopAllWriters() { this.writers.forEach((writer) => { writer.running = false; }); }
  async listWriters() { return structuredClone(this.writers); }
  async setApplicationAuthority(value) { this.authority = value; }
  async activeAuthorities() { return this.authority ? [this.authority] : []; }
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
  assert.equal(evidence.dual_authority_observed, false);
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

test("a dual-authority observation aborts before copying", async () => {
  const { source, destination } = stores(), control = new Control(source, destination);
  control.activeAuthorities = async () => [source.id, destination.id];
  await assert.rejects(rehearseOfflineMigration({ source, destination, control }), /single-authority invariant/);
});
