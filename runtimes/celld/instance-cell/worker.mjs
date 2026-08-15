const encoder = new TextEncoder();
const OPERATION_ID = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;
const REQUEST_HASH = /^[a-f0-9]{64}$/;
const ACTIONS = new Set(["provision", "start", "stop", "destroy", "observe", "repair"]);
const TERMINAL_EFFECTS = new Set(["succeeded", "failed", "rejected"]);
const MAX_EFFECTS = 1_000;

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const match = url.pathname.match(/^\/instance-cells\/([^/]+)(?:\/(commands|reconcile))?$/);
    if (!match) return response(404, { error: { code: "cell.not_found", detail: "unknown route" } });
    let routedInstanceId;
    try {
      routedInstanceId = decodeURIComponent(match[1]);
    } catch {
      return response(400, { error: { code: "cell.instance_invalid" } });
    }
    if (!validInstanceId(routedInstanceId)) {
      return response(400, { error: { code: "cell.instance_invalid" } });
    }
    const body = request.method === "POST" ? await request.arrayBuffer() : new ArrayBuffer(0);
    const auth = await authenticate(request, env, url.pathname, body);
    if (!auth.ok) return auth.response;
    const stub = env.INSTANCE_CELLS.getByName(routedInstanceId);
    const headers = new Headers(request.headers);
    headers.set("x-agentic-authenticated", "true");
    headers.set("x-agentic-instance-id", routedInstanceId);
    return stub.fetch(new Request(`https://cell.internal/${match[2] || "state"}`, { method: request.method, headers, body: body.byteLength ? body : undefined }));
  },
};

export class InstanceCell {
  constructor(state, env) { this.state = state; this.env = env; }

  async fetch(request) {
    if (request.headers.get("x-agentic-authenticated") !== "true") return response(401, { error: { code: "cell.auth_required" } });
    const routedInstanceId = request.headers.get("x-agentic-instance-id");
    if (!validInstanceId(routedInstanceId)) return response(400, { error: { code: "cell.instance_invalid" } });
    if (!await this.consumeNonce(request.headers.get("x-agentic-nonce"), request.headers.get("x-agentic-timestamp"))) {
      return response(409, { error: { code: "cell.signature_replayed" } });
    }
    const route = new URL(request.url).pathname;
    if (route === "/state" && request.method === "GET") {
      const cell = await this.state.storage.get("cell");
      return cell ? response(200, cell) : response(404, { error: { code: "cell.missing" } });
    }
    if (route === "/commands" && request.method === "POST") {
      return this.command(
        await request.json(),
        routedInstanceId,
        request.headers.get("x-agentic-operation-id"),
        Number(request.headers.get("x-agentic-generation")),
      );
    }
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

  async command(command, routedInstanceId, authenticatedOperationId, authenticatedGeneration) {
    if (!validCommand(command)) {
      return response(422, { error: { code: "cell.command_invalid" } });
    }
    if (command.instance_id !== routedInstanceId) {
      return response(409, { error: { code: "cell.instance_mismatch" } });
    }
    if (command.operation_id !== authenticatedOperationId || command.generation !== authenticatedGeneration) {
      return response(409, { error: { code: "cell.signature_context_mismatch" } });
    }
    if (await canonicalRequestHash(command) !== command.request_hash) {
      return response(422, { error: { code: "cell.request_hash_invalid" } });
    }
    let result;
    await this.state.storage.transaction(async (tx) => {
      let cell = await tx.get("cell") || freshCell(command.instance_id, command.generation);
      if (cell.instance_id !== command.instance_id) { result = [409, { error: { code: "cell.instance_mismatch" } }]; return; }
      if (command.generation !== cell.generation) { result = [409, { error: { code: "cell.generation_fenced", current_generation: cell.generation } }]; return; }
      const existing = cell.effects.find((effect) => effect.operation_id === command.operation_id);
      if (existing) {
        result = existing.request_hash === command.request_hash ? [200, cell] : [409, { error: { code: "cell.operation_collision" } }];
        return;
      }
      if (cell.effects.length >= MAX_EFFECTS) { result = [422, { error: { code: "cell.effect_limit" } }]; return; }
      const nextState = nextDesiredState(cell.desired_state, command.action);
      if (nextState === null) { result = [409, { error: { code: "cell.transition_invalid", state: cell.desired_state, action: command.action } }]; return; }
      cell.effects.push({ operation_id: command.operation_id, request_hash: command.request_hash, action: command.action, generation: command.generation, payload: command.payload, status: "pending", attempts: 0, retry_at: null, terminal_code: null, management_operation_id: null });
      cell.desired_state = nextState;
      append(cell, "command_accepted", command.operation_id, { request_hash: command.request_hash });
      await tx.put("cell", cell);
      // Leave a deterministic scheduling window so the command acknowledgement
      // is durable before an alarm runner can claim the effect.
      await tx.setAlarm(Date.now() + 1_000);
      result = [202, cell];
    });
    return response(result[0], result[1]);
  }

  async reconcile({ management_generation }) {
    const cell = await this.state.storage.get("cell");
    if (!cell) return response(404, { error: { code: "cell.missing" } });
    const pending = cell.effects.filter((effect) => !TERMINAL_EFFECTS.has(effect.status));
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
    const pending = cell.effects.find((effect) => effect.status === "pending" || effect.status === "dispatched" || effect.status === "unknown");
    if (!pending) return;
    // The management callback binding owns effects. Reuse the original operation id;
    // a timeout is unknown, never permission to mint a second effect.
    try {
      pending.status = "dispatched"; pending.attempts += 1;
      const path = "/api/v2/celld/effects";
      const body = JSON.stringify({ instance_id: cell.instance_id, generation: cell.generation, effect: pending });
      const signed = await signManagementCallback(this.env, path, pending.operation_id, cell.generation, body);
      const reply = await this.env.MANAGEMENT.fetch(`https://management.internal${path}`, {
        method: "POST",
        headers: { "content-type": "application/json", "idempotency-key": pending.operation_id, ...signed },
        body,
      });
      const result = reply?.ok ? await reply.json().catch(() => ({})) : {};
      pending.status = ["succeeded", "failed", "rejected", "dispatched", "unknown"].includes(result.status)
        ? result.status
        : reply?.ok ? "succeeded" : "unknown";
      pending.management_operation_id = typeof result.management_operation_id === "string" ? result.management_operation_id : pending.management_operation_id;
      pending.terminal_code = typeof result.terminal_code === "string" ? result.terminal_code : pending.terminal_code;
    } catch { pending.status = "unknown"; }
    await this.state.storage.put("cell", cell);
    if (pending.status === "unknown" || pending.status === "dispatched") await this.state.storage.setAlarm(Date.now() + Math.min(60_000, 1000 * 2 ** Math.min(pending.attempts, 6)));
  }
}

async function signManagementCallback(env, path, operationId, generation, body) {
  const timestamp = new Date().toISOString();
  const nonce = crypto.randomUUID().replaceAll("-", "");
  const digest = hex(await crypto.subtle.digest("SHA-256", encoder.encode(body)));
  const canonical = ["POST", path, operationId, String(generation), timestamp, nonce, digest].join("\n");
  const keyring = authKeyring(env, Date.now());
  const activeKey = keyring?.get(env.CELL_AUTH_KEY_ID);
  if (!activeKey) throw new Error("cell authentication configuration is invalid");
  const key = await crypto.subtle.importKey("raw", encoder.encode(activeKey), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const signature = hex(await crypto.subtle.sign("HMAC", key, encoder.encode(canonical)));
  return {
    "x-agentic-key-id": env.CELL_AUTH_KEY_ID,
    "x-agentic-timestamp": timestamp,
    "x-agentic-nonce": nonce,
    "x-agentic-generation": String(generation),
    "x-agentic-operation-id": operationId,
    "x-agentic-body-sha256": digest,
    "x-agentic-signature": signature,
  };
}

async function authenticate(request, env, path, body) {
  const fields = ["key-id", "timestamp", "nonce", "generation", "operation-id", "body-sha256", "signature"];
  const values = Object.fromEntries(fields.map((field) => [field, request.headers.get(`x-agentic-${field}`)]));
  if (fields.some((field) => !values[field])) return { ok: false, response: response(401, { error: { code: "cell.signature_missing" } }) };
  const timestamp = Date.parse(values.timestamp);
  const generation = Number(values.generation);
  const now = Date.now();
  if (!Number.isFinite(timestamp) || Math.abs(now - timestamp) > 120_000 || !Number.isSafeInteger(generation) || generation < 1) return { ok: false, response: response(401, { error: { code: "cell.signature_stale" } }) };
  const digest = hex(await crypto.subtle.digest("SHA-256", body));
  const verificationKey = authKeyring(env, now)?.get(values["key-id"]);
  if (digest !== values["body-sha256"] || !verificationKey) return { ok: false, response: response(401, { error: { code: "cell.signature_invalid" } }) };
  if (!/^[a-f0-9]{64}$/.test(values.signature)) return { ok: false, response: response(401, { error: { code: "cell.signature_invalid" } }) };
  const key = await crypto.subtle.importKey("raw", encoder.encode(verificationKey), { name: "HMAC", hash: "SHA-256" }, false, ["verify"]);
  const canonical = [request.method.toUpperCase(), path, values["operation-id"], values.generation, values.timestamp, values.nonce, digest].join("\n");
  const ok = await crypto.subtle.verify("HMAC", key, unhex(values.signature), encoder.encode(canonical));
  return ok ? { ok: true } : { ok: false, response: response(401, { error: { code: "cell.signature_invalid" } }) };
}

function authKeyring(env, now) {
  const activeId = typeof env.CELL_AUTH_KEY_ID === "string" ? env.CELL_AUTH_KEY_ID : "";
  const activeKey = typeof env.CELL_AUTH_KEY === "string" ? env.CELL_AUTH_KEY : "";
  if (!activeId || encoder.encode(activeKey).byteLength < 32) return null;

  const previous = [
    env.CELL_AUTH_PREVIOUS_KEY_ID,
    env.CELL_AUTH_PREVIOUS_KEY,
    env.CELL_AUTH_PREVIOUS_VALID_FROM,
    env.CELL_AUTH_PREVIOUS_VALID_UNTIL,
  ];
  const configured = previous.filter((value) => typeof value === "string" && value.length > 0).length;
  if (configured !== 0 && configured !== previous.length) return null;

  const keys = new Map([[activeId, activeKey]]);
  if (configured === previous.length) {
    const [previousId, previousKey, rawFrom, rawUntil] = previous;
    const validFrom = Date.parse(rawFrom);
    const validUntil = Date.parse(rawUntil);
    if (previousId === activeId || encoder.encode(previousKey).byteLength < 32 ||
        !Number.isFinite(validFrom) || !Number.isFinite(validUntil) ||
        validUntil <= validFrom || validUntil - validFrom > 15 * 60_000) return null;
    if (now >= validFrom && now <= validUntil) keys.set(previousId, previousKey);
  }
  return keys;
}

function freshCell(instance_id, generation) { return { document_type: "instance-cell-state", schema_version: "1", instance_id, generation, desired_state: "requested", observation: null, effects: [], history_sequence: 0, tombstone_until: null, updated_at: new Date().toISOString(), history: [] }; }
function validInstanceId(value) { return typeof value === "string" && value.length > 0 && value.length <= 128; }
function validCommand(command) {
  return command !== null && typeof command === "object" && !Array.isArray(command) &&
    command.document_type === "instance-cell-command" && command.schema_version === "1" &&
    OPERATION_ID.test(command.operation_id || "") && validInstanceId(command.instance_id) &&
    Number.isSafeInteger(command.generation) && command.generation > 0 && ACTIONS.has(command.action) &&
    REQUEST_HASH.test(command.request_hash || "") && typeof command.issued_at === "string" &&
    command.payload !== null && typeof command.payload === "object" && !Array.isArray(command.payload);
}
function nextDesiredState(current, action) {
  if (action === "provision" && ["requested", "stopped", "failed"].includes(current)) return "provisioning";
  if (action === "start" && ["stopped", "failed"].includes(current)) return "provisioning";
  if (action === "stop" && ["provisioning", "enrolling", "ready", "unknown"].includes(current)) return "stopping";
  if (action === "destroy" && current !== "destroyed") return "stopping";
  if (action === "observe") return current;
  if (action === "repair" && current !== "destroyed") return "unknown";
  return null;
}
async function canonicalRequestHash(command) {
  const canonical = canonicalJson({
    operation_id: command.operation_id,
    instance_id: command.instance_id,
    generation: command.generation,
    action: command.action,
    payload: command.payload,
  });
  return hex(await crypto.subtle.digest("SHA-256", encoder.encode(canonical)));
}
function canonicalJson(value) {
  if (value === null || typeof value === "boolean" || typeof value === "string") return JSON.stringify(value);
  if (typeof value === "number") {
    if (!Number.isFinite(value)) throw new TypeError("non-finite numbers are not valid JSON");
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
  }
  throw new TypeError("unsupported JSON value");
}
function append(cell, kind, operation_id, evidence) { cell.history_sequence += 1; cell.updated_at = new Date().toISOString(); cell.history.push({ document_type: "instance-cell-event", schema_version: "1", event_id: crypto.randomUUID(), instance_id: cell.instance_id, operation_id, generation: cell.generation, sequence: cell.history_sequence, kind, recorded_at: cell.updated_at, evidence }); }
function response(status, value) { return Response.json(value, { status }); }
function hex(buffer) { return [...new Uint8Array(buffer)].map((byte) => byte.toString(16).padStart(2, "0")).join(""); }
function unhex(value) { return Uint8Array.from(value.match(/../g) || [], (byte) => parseInt(byte, 16)); }
