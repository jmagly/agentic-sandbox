const encoder = new TextEncoder();

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const match = url.pathname.match(/^\/instance-cells\/([^/]+)(?:\/(commands|reconcile))?$/);
    if (!match) return response(404, { error: { code: "cell.not_found", detail: "unknown route" } });
    const body = request.method === "POST" ? await request.arrayBuffer() : new ArrayBuffer(0);
    const auth = await authenticate(request, env, url.pathname, body);
    if (!auth.ok) return auth.response;
    const stub = env.INSTANCE_CELLS.getByName(decodeURIComponent(match[1]));
    const headers = new Headers(request.headers);
    headers.set("x-agentic-authenticated", "true");
    return stub.fetch(new Request(`https://cell.internal/${match[2] || "state"}`, { method: request.method, headers, body: body.byteLength ? body : undefined }));
  },
};

export class InstanceCell {
  constructor(state, env) { this.state = state; this.env = env; }

  async fetch(request) {
    if (request.headers.get("x-agentic-authenticated") !== "true") return response(401, { error: { code: "cell.auth_required" } });
    if (!await this.consumeNonce(request.headers.get("x-agentic-nonce"), request.headers.get("x-agentic-timestamp"))) {
      return response(409, { error: { code: "cell.signature_replayed" } });
    }
    const route = new URL(request.url).pathname;
    if (route === "/state" && request.method === "GET") {
      const cell = await this.state.storage.get("cell");
      return cell ? response(200, cell) : response(404, { error: { code: "cell.missing" } });
    }
    if (route === "/commands" && request.method === "POST") return this.command(await request.json());
    if (route === "/reconcile" && request.method === "POST") return this.reconcile(await request.json());
    return response(405, { error: { code: "cell.method_not_allowed" } });
  }

  async consumeNonce(nonce, timestamp) {
    if (!nonce || !timestamp) return false;
    let accepted = false;
    await this.state.storage.transaction(async (tx) => {
      const key = `nonce:${nonce}`;
      if (await tx.get(key)) return;
      const cutoff = Date.now() - 120_000;
      const prior = await tx.get("nonce-index") || [];
      const expired = prior.filter((item) => item.timestamp < cutoff).map((item) => item.key);
      if (expired.length) await tx.delete(expired);
      const index = prior.filter((item) => item.timestamp >= cutoff);
      if (index.length >= 4096) return;
      await tx.put(key, timestamp);
      index.push({ key, timestamp: Date.parse(timestamp) });
      await tx.put("nonce-index", index);
      accepted = true;
    });
    return accepted;
  }

  async command(command) {
    if (command.document_type !== "instance-cell-command" || command.schema_version !== "1" || !command.operation_id || command.generation < 1) {
      return response(422, { error: { code: "cell.command_invalid" } });
    }
    let result;
    await this.state.storage.transaction(async (tx) => {
      let cell = await tx.get("cell") || freshCell(command.instance_id, command.generation);
      if (cell.instance_id !== command.instance_id) { result = [409, { error: { code: "cell.instance_mismatch" } }]; return; }
      const existing = cell.effects.find((effect) => effect.operation_id === command.operation_id);
      if (existing) {
        result = existing.request_hash === command.request_hash ? [200, cell] : [409, { error: { code: "cell.operation_collision" } }];
        return;
      }
      if (command.generation !== cell.generation) { result = [409, { error: { code: "cell.generation_fenced", current_generation: cell.generation } }]; return; }
      cell.effects.push({ operation_id: command.operation_id, request_hash: command.request_hash, action: command.action, generation: command.generation, status: "pending", attempts: 0 });
      cell.desired_state = desired(command.action);
      append(cell, "command_accepted", command.operation_id, { request_hash: command.request_hash });
      await tx.put("cell", cell);
      await tx.setAlarm(Date.now() + 1);
      result = [202, cell];
    });
    return response(result[0], result[1]);
  }

  async reconcile({ management_generation }) {
    const cell = await this.state.storage.get("cell");
    if (!cell) return response(404, { error: { code: "cell.missing" } });
    const pending = cell.effects.filter((effect) => !["succeeded", "failed", "rejected"].includes(effect.status));
    let classification = "converged", repair = null;
    if (cell.generation < management_generation) [classification, repair] = ["cell_generation_stale", "refresh_cell_binding_from_management"];
    else if (cell.generation > management_generation) [classification, repair] = ["observation_stale", "refresh_management_inventory_without_effect"];
    else if (pending.some((effect) => effect.status === "unknown")) [classification, repair] = ["outcome_unknown", "lookup_original_operations_and_inventory"];
    else if (pending.length) [classification, repair] = ["effect_pending", "dispatch_pending_effects_by_original_operation_id"];
    return response(200, { classification, instance_id: cell.instance_id, generation: cell.generation, desired_state: cell.desired_state, observed_runtime_state: cell.observation?.runtime_state || null, outstanding_operations: pending.map((effect) => effect.operation_id), repair });
  }

  async alarm() {
    const cell = await this.state.storage.get("cell");
    if (!cell) return;
    const pending = cell.effects.find((effect) => effect.status === "pending" || effect.status === "unknown");
    if (!pending) return;
    // The management callback binding owns effects. Reuse the original operation id;
    // a timeout is unknown, never permission to mint a second effect.
    try {
      pending.status = "dispatched"; pending.attempts += 1;
      const reply = await this.env.MANAGEMENT.fetch("https://management.internal/api/v2/celld/effects", { method: "POST", headers: { "content-type": "application/json", "idempotency-key": pending.operation_id }, body: JSON.stringify({ instance_id: cell.instance_id, generation: cell.generation, effect: pending }) });
      pending.status = reply?.ok ? "succeeded" : "unknown";
    } catch { pending.status = "unknown"; }
    await this.state.storage.put("cell", cell);
    if (pending.status === "unknown") await this.state.storage.setAlarm(Date.now() + Math.min(60_000, 1000 * 2 ** Math.min(pending.attempts, 6)));
  }
}

async function authenticate(request, env, path, body) {
  const fields = ["key-id", "timestamp", "nonce", "generation", "operation-id", "body-sha256", "signature"];
  const values = Object.fromEntries(fields.map((field) => [field, request.headers.get(`x-agentic-${field}`)]));
  if (fields.some((field) => !values[field])) return { ok: false, response: response(401, { error: { code: "cell.signature_missing" } }) };
  const timestamp = Date.parse(values.timestamp);
  if (!Number.isFinite(timestamp) || Math.abs(Date.now() - timestamp) > 120_000 || Number(values.generation) < 1) return { ok: false, response: response(401, { error: { code: "cell.signature_stale" } }) };
  const digest = hex(await crypto.subtle.digest("SHA-256", body));
  if (digest !== values["body-sha256"] || values["key-id"] !== env.CELL_AUTH_KEY_ID) return { ok: false, response: response(401, { error: { code: "cell.signature_invalid" } }) };
  const key = await crypto.subtle.importKey("raw", encoder.encode(env.CELL_AUTH_KEY), { name: "HMAC", hash: "SHA-256" }, false, ["verify"]);
  const canonical = [request.method.toUpperCase(), path, values["operation-id"], values.generation, values.timestamp, values.nonce, digest].join("\n");
  const ok = await crypto.subtle.verify("HMAC", key, unhex(values.signature), encoder.encode(canonical));
  return ok ? { ok: true } : { ok: false, response: response(401, { error: { code: "cell.signature_invalid" } }) };
}

function freshCell(instance_id, generation) { return { document_type: "instance-cell-state", schema_version: "1", instance_id, generation, desired_state: "requested", observation: null, effects: [], history_sequence: 0, tombstone_until: null, updated_at: new Date().toISOString(), history: [] }; }
function desired(action) { return ({ provision: "provisioning", start: "provisioning", stop: "stopping", destroy: "stopping", observe: "unknown", repair: "unknown" })[action] || "unknown"; }
function append(cell, kind, operation_id, evidence) { cell.history_sequence += 1; cell.updated_at = new Date().toISOString(); cell.history.push({ document_type: "instance-cell-event", schema_version: "1", event_id: crypto.randomUUID(), instance_id: cell.instance_id, operation_id, generation: cell.generation, sequence: cell.history_sequence, kind, recorded_at: cell.updated_at, evidence }); }
function response(status, value) { return Response.json(value, { status }); }
function hex(buffer) { return [...new Uint8Array(buffer)].map((byte) => byte.toString(16).padStart(2, "0")).join(""); }
function unhex(value) { return Uint8Array.from(value.match(/../g) || [], (byte) => parseInt(byte, 16)); }
