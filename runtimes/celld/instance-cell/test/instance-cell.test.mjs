import { env, exports as workerExports } from "cloudflare:workers";
import {
  abortAllDurableObjects,
  runDurableObjectAlarm,
} from "cloudflare:test";
import { sendManagementCallback } from "../worker.mjs";
import { describe, it } from "vitest";

const KEY = "01234567890123456789012345678901";
const KEY_ID = "test-active";
const PREVIOUS_KEY = "abcdefghijklmnopqrstuvwxyzABCDEF";
const PREVIOUS_KEY_ID = "test-previous";
const encoder = new TextEncoder();
let nonceSequence = 0;

describe("signed InstanceCell behavior", () => {
  it("uses a strict HTTPS management origin when no service binding is present", async ({ expect }) => {
    let observed;
    const response = await sendManagementCallback(
      { MANAGEMENT_URL: "https://management.internal/" },
      "/api/v2/celld/effects",
      { method: "POST", body: "{}" },
      async (url, init) => { observed = { url: String(url), init }; return new Response(null, { status: 202 }); },
    );
    expect(response.status).toBe(202);
    expect(observed.url).toBe("https://management.internal/api/v2/celld/effects");
    expect(observed.init.method).toBe("POST");
  });

  it("rejects credential-bearing and cleartext remote management origins", async ({ expect }) => {
    for (const MANAGEMENT_URL of [
      "http://management.internal/",
      "https://user:pass@management.internal/",
      "https://management.internal/base/",
      "https://management.internal/?token=hidden",
    ]) {
      await expect(sendManagementCallback({ MANAGEMENT_URL }, "/api/v2/celld/effects", {}, async () => new Response())).rejects.toThrow("not an approved origin");
    }
  });

  it("rejects callback paths that can escape or alter the signed route", async ({ expect }) => {
    for (const path of [
      "https://attacker.invalid/effects",
      "//attacker.invalid/effects",
      "/api/v2/celld/effects?token=hidden",
      "/api/v2/celld/effects#fragment",
    ]) {
      await expect(sendManagementCallback({ MANAGEMENT_URL: "https://management.internal/" }, path, {}, async () => new Response())).rejects.toThrow("callback path is invalid");
    }
  });
  it("preserves cell and replay state across a forced Durable Object restart", async ({ expect }) => {
    const instanceId = "instance-eviction";
    const command = await makeCommand(instanceId, "op-eviction", 1, "provision");
    const request = await signedRequest(`/instance-cells/${instanceId}/commands`, {
      body: command,
      generation: 1,
      operationId: "op-eviction",
    });
    const replay = request.clone();
    expect((await workerExports.default.fetch(request)).status).toBe(202);

    const stub = env.INSTANCE_CELLS.get(env.INSTANCE_CELLS.idFromName(instanceId));
    expect(await runDurableObjectAlarm(stub)).toBe(true);
    await abortAllDurableObjects();
    const state = await getCell(instanceId, 1);
    expect(state.instance_id).toBe(instanceId);
    expect(state.effects).toHaveLength(1);

    const replayResponse = await workerExports.default.fetch(replay);
    expect(replayResponse.status).toBe(409);
    expect((await replayResponse.json()).error.code).toBe("cell.signature_replayed");
  });

  it("accepts a valid HMAC and rejects body tampering", async ({ expect }) => {
    const command = await makeCommand("instance-auth", "op-auth", 1, "provision");
    const valid = await signedRequest("/instance-cells/instance-auth/commands", {
      body: command,
      generation: 1,
      operationId: "op-auth",
    });
    const response = await workerExports.default.fetch(valid);
    expect(response.status).toBe(202);

    const original = await signedRequest("/instance-cells/instance-tamper/commands", {
      body: await makeCommand("instance-tamper", "op-tamper", 1, "provision"),
      generation: 1,
      operationId: "op-tamper",
    });
    const changedBody = JSON.stringify({ unexpected: true });
    const tampered = new Request(original.url, {
      method: original.method,
      headers: original.headers,
      body: changedBody,
    });
    const rejected = await workerExports.default.fetch(tampered);
    expect(rejected.status).toBe(401);
    expect((await rejected.json()).error.code).toBe("cell.signature_invalid");
  });

  it("accepts the previous HMAC key during the bounded rotation window", async ({ expect }) => {
    const command = await makeCommand("instance-rotation", "op-rotation", 1, "provision");
    const request = await signedRequest("/instance-cells/instance-rotation/commands", {
      body: command,
      generation: 1,
      operationId: "op-rotation",
      key: PREVIOUS_KEY,
      keyId: PREVIOUS_KEY_ID,
    });
    const response = await workerExports.default.fetch(request);
    expect(response.status).toBe(202);
  });

  it("rejects stale signed requests before state mutation", async ({ expect }) => {
    const command = await makeCommand("instance-stale", "op-stale", 1, "provision");
    const request = await signedRequest("/instance-cells/instance-stale/commands", {
      body: command,
      generation: 1,
      operationId: "op-stale",
      timestamp: new Date(Date.now() - 121_000).toISOString(),
    });
    const response = await workerExports.default.fetch(request);
    expect(response.status).toBe(401);
    expect((await response.json()).error.code).toBe("cell.signature_stale");
  });

  it("persists and rejects a replayed nonce", async ({ expect }) => {
    const command = await makeCommand("instance-replay", "op-replay", 1, "provision");
    const request = await signedRequest("/instance-cells/instance-replay/commands", {
      body: command,
      generation: 1,
      operationId: "op-replay",
    });
    const replay = request.clone();
    expect((await workerExports.default.fetch(request)).status).toBe(202);
    const response = await workerExports.default.fetch(replay);
    expect(response.status).toBe(409);
    expect((await response.json()).error.code).toBe("cell.signature_replayed");
  });

  it("binds the signed route identity to the command identity", async ({ expect }) => {
    const command = await makeCommand("body-instance", "op-binding", 1, "provision");
    const request = await signedRequest("/instance-cells/path-instance/commands", {
      body: command,
      generation: 1,
      operationId: "op-binding",
    });
    const response = await workerExports.default.fetch(request);
    expect(response.status).toBe(409);
    expect((await response.json()).error.code).toBe("cell.instance_mismatch");
  });

  it("binds the signed operation and generation to the command", async ({ expect }) => {
    const command = await makeCommand("instance-context", "op-body", 2, "provision");
    const request = await signedRequest("/instance-cells/instance-context/commands", {
      body: command,
      generation: 1,
      operationId: "op-header",
    });
    const response = await workerExports.default.fetch(request);
    expect(response.status).toBe(409);
    expect((await response.json()).error.code).toBe("cell.signature_context_mismatch");
  });

  it("recomputes the canonical request hash", async ({ expect }) => {
    const command = await makeCommand("instance-hash", "op-hash", 1, "provision");
    command.request_hash = "0".repeat(64);
    const request = await signedRequest("/instance-cells/instance-hash/commands", {
      body: command,
      generation: 1,
      operationId: "op-hash",
    });
    const response = await workerExports.default.fetch(request);
    expect(response.status).toBe(422);
    expect((await response.json()).error.code).toBe("cell.request_hash_invalid");
  });

  it("matches the Rust JCS request-hash vector", async ({ expect }) => {
    const command = {
      document_type: "instance-cell-command",
      schema_version: "1",
      operation_id: "op-vector",
      instance_id: "instance-vector",
      generation: 7,
      action: "provision",
      request_hash: "3570919d5b0f942f23a288d92611914d91d3472f10e31a44c76359f940936636",
      issued_at: new Date().toISOString(),
      payload: { z: 1, a: { b: true, a: "x" } },
    };
    const request = await signedRequest("/instance-cells/instance-vector/commands", {
      body: command,
      generation: 7,
      operationId: "op-vector",
    });
    expect((await workerExports.default.fetch(request)).status).toBe(202);
  });

  it("rejects unknown actions and invalid lifecycle transitions", async ({ expect }) => {
    const unknown = await makeCommand("instance-transition", "op-unknown", 1, "launch");
    let request = await signedRequest("/instance-cells/instance-transition/commands", {
      body: unknown,
      generation: 1,
      operationId: "op-unknown",
    });
    let response = await workerExports.default.fetch(request);
    expect(response.status).toBe(422);
    expect((await response.json()).error.code).toBe("cell.command_invalid");

    const provision = await makeCommand("instance-transition", "op-first", 1, "provision");
    request = await signedRequest("/instance-cells/instance-transition/commands", {
      body: provision,
      generation: 1,
      operationId: "op-first",
    });
    expect((await workerExports.default.fetch(request)).status).toBe(202);

    const repeatedTransition = await makeCommand("instance-transition", "op-second", 1, "provision");
    request = await signedRequest("/instance-cells/instance-transition/commands", {
      body: repeatedTransition,
      generation: 1,
      operationId: "op-second",
    });
    response = await workerExports.default.fetch(request);
    expect(response.status).toBe(409);
    expect((await response.json()).error.code).toBe("cell.transition_invalid");
  });

  it("replays an identical operation and rejects an operation hash collision", async ({ expect }) => {
    const command = await makeCommand("instance-idem", "op-idem", 1, "provision", { runtime: "qemu" });
    let request = await signedRequest("/instance-cells/instance-idem/commands", {
      body: command,
      generation: 1,
      operationId: "op-idem",
    });
    expect((await workerExports.default.fetch(request)).status).toBe(202);

    request = await signedRequest("/instance-cells/instance-idem/commands", {
      body: command,
      generation: 1,
      operationId: "op-idem",
    });
    let response = await workerExports.default.fetch(request);
    expect(response.status).toBe(200);
    expect((await response.json()).effects).toHaveLength(1);

    const collision = await makeCommand("instance-idem", "op-idem", 1, "provision", { runtime: "docker" });
    request = await signedRequest("/instance-cells/instance-idem/commands", {
      body: collision,
      generation: 1,
      operationId: "op-idem",
    });
    response = await workerExports.default.fetch(request);
    expect(response.status).toBe(409);
    expect((await response.json()).error.code).toBe("cell.operation_collision");
  });

  it("returns bounded per-operation projections after cell history grows", async ({ expect }) => {
    const instanceId = "instance-bounded-projection";
    let latestOperationId;
    for (let index = 0; index < 40; index += 1) {
      latestOperationId = `op-bounded-${index}`;
      const command = await makeCommand(instanceId, latestOperationId, 1, "observe", {
        marker: "x".repeat(64),
      });
      const response = await workerExports.default.fetch(await signedRequest(`/instance-cells/${instanceId}/commands`, {
        body: command,
        generation: 1,
        operationId: latestOperationId,
      }));
      expect(response.status).toBe(202);
      const bytes = await response.clone().arrayBuffer();
      expect(bytes.byteLength).toBeLessThanOrEqual(4096);
      const body = await response.json();
      expect(body.effects).toHaveLength(1);
      expect(body.effects[0].operation_id).toBe(latestOperationId);
      expect(body.history.every((event) => event.operation_id === latestOperationId)).toBe(true);
    }

    const lookup = await workerExports.default.fetch(await signedRequest(`/instance-cells/${instanceId}/operation`, {
      generation: 1,
      operationId: latestOperationId,
    }));
    expect(lookup.status).toBe(200);
    expect((await lookup.clone().arrayBuffer()).byteLength).toBeLessThanOrEqual(4096);
    const projected = await lookup.json();
    expect(projected.effects).toHaveLength(1);
    expect(projected.effects[0].operation_id).toBe(latestOperationId);

    const missing = await workerExports.default.fetch(await signedRequest(`/instance-cells/${instanceId}/operation`, {
      generation: 1,
      operationId: "op-not-present",
    }));
    expect(missing.status).toBe(404);
    expect((await missing.json()).error.code).toBe("cell.operation_missing");
  });

  it("fences an older generation before operation replay", async ({ expect }) => {
    const current = await makeCommand("instance-fence", "op-current", 2, "provision");
    let request = await signedRequest("/instance-cells/instance-fence/commands", {
      body: current,
      generation: 2,
      operationId: "op-current",
    });
    expect((await workerExports.default.fetch(request)).status).toBe(202);

    const stale = await makeCommand("instance-fence", "op-stale", 1, "stop");
    request = await signedRequest("/instance-cells/instance-fence/commands", {
      body: stale,
      generation: 1,
      operationId: "op-stale",
    });
    const response = await workerExports.default.fetch(request);
    expect(response.status).toBe(409);
    const body = await response.json();
    expect(body.error.code).toBe("cell.generation_fenced");
    expect(body.error.current_generation).toBe(2);
  });

  it("dispatches an alarm with the original operation idempotency key", async ({ expect }) => {
    const instanceId = "instance-alarm";
    const operationId = "op-original";
    const command = await makeCommand(instanceId, operationId, 1, "provision", {
      name: "instance-alarm",
      runtime: "docker",
    });
    const request = await signedRequest(`/instance-cells/${instanceId}/commands`, {
      body: command,
      generation: 1,
      operationId,
    });
    expect((await workerExports.default.fetch(request)).status).toBe(202);

    const stub = env.INSTANCE_CELLS.get(env.INSTANCE_CELLS.idFromName(instanceId));
    expect(await runDurableObjectAlarm(stub)).toBe(true);

    const state = await getCell(instanceId, 1);
    expect(state.effects).toHaveLength(1);
    expect(state.effects[0]).toMatchObject({
      operation_id: operationId,
      status: "succeeded",
      attempts: 1,
      payload: { name: "instance-alarm", runtime: "docker" },
      management_operation_id: `management-${operationId}`,
      terminal_code: "provider.effect_succeeded",
    });
  });

  it("rechecks a dispatched management operation without changing effect identity", async ({ expect }) => {
    const instanceId = "instance-dispatched";
    const operationId = "op-dispatched";
    const command = await makeCommand(instanceId, operationId, 1, "provision", {
      name: instanceId,
      runtime: "docker",
    });
    const request = await signedRequest(`/instance-cells/${instanceId}/commands`, {
      body: command,
      generation: 1,
      operationId,
    });
    expect((await workerExports.default.fetch(request)).status).toBe(202);

    const stub = env.INSTANCE_CELLS.get(env.INSTANCE_CELLS.idFromName(instanceId));
    expect(await runDurableObjectAlarm(stub)).toBe(true);
    let state = await getCell(instanceId, 1);
    expect(state.effects[0]).toMatchObject({
      operation_id: operationId,
      status: "dispatched",
      attempts: 1,
      management_operation_id: `management-${operationId}`,
    });

    expect(await runDurableObjectAlarm(stub)).toBe(true);
    state = await getCell(instanceId, 1);
    expect(state.effects).toHaveLength(1);
    expect(state.effects[0]).toMatchObject({
      operation_id: operationId,
      status: "succeeded",
      attempts: 2,
      management_operation_id: `management-${operationId}`,
    });
  });

  it("advances successful lifecycle state and reprovisions only from a retained tombstone", async ({ expect }) => {
    const instanceId = "instance-lifecycle";
    const stub = env.INSTANCE_CELLS.get(env.INSTANCE_CELLS.idFromName(instanceId));
    const deliver = async (operationId, generation, action, payload = {}) => {
      const command = await makeCommand(instanceId, operationId, generation, action, payload);
      const response = await workerExports.default.fetch(await signedRequest(`/instance-cells/${instanceId}/commands`, {
        body: command,
        generation,
        operationId,
      }));
      expect(response.status).toBe(202);
      expect(await runDurableObjectAlarm(stub)).toBe(true);
      return getCell(instanceId, generation);
    };

    let state = await deliver("op-lifecycle-provision", 1, "provision", { name: instanceId, runtime: "docker", start: false });
    expect(state.desired_state).toBe("stopped");
    state = await deliver("op-lifecycle-start", 1, "start");
    expect(state.desired_state).toBe("ready");
    state = await deliver("op-lifecycle-stop", 1, "stop");
    expect(state.desired_state).toBe("stopped");
    state = await deliver("op-lifecycle-destroy", 1, "destroy");
    expect(state.desired_state).toBe("destroyed");
    expect(Date.parse(state.tombstone_until)).toBeGreaterThan(Date.now() + 29 * 24 * 60 * 60 * 1_000);

    const future = await makeCommand(instanceId, "op-lifecycle-future", 3, "provision", { name: instanceId, runtime: "docker" });
    let response = await workerExports.default.fetch(await signedRequest(`/instance-cells/${instanceId}/commands`, {
      body: future,
      generation: 3,
      operationId: "op-lifecycle-future",
    }));
    expect(response.status).toBe(409);
    expect((await response.json()).error.code).toBe("cell.generation_fenced");

    state = await deliver("op-lifecycle-reprovision", 2, "provision", { name: instanceId, runtime: "docker", start: false });
    expect(state.generation).toBe(2);
    expect(state.desired_state).toBe("stopped");
    expect(state.effects).toHaveLength(5);
    expect(state.history.some((event) => event.kind === "tombstoned" && event.generation === 1)).toBe(true);

    const stale = await makeCommand(instanceId, "op-lifecycle-stale", 1, "destroy");
    response = await workerExports.default.fetch(await signedRequest(`/instance-cells/${instanceId}/commands`, {
      body: stale,
      generation: 1,
      operationId: "op-lifecycle-stale",
    }));
    expect(response.status).toBe(409);
    expect((await response.json()).error.current_generation).toBe(2);
  });

});

async function getCell(instanceId, generation) {
  const request = await signedRequest(`/instance-cells/${instanceId}`, {
    generation,
    operationId: "observe",
  });
  const response = await workerExports.default.fetch(request);
  if (!response.ok) throw new Error(`get cell failed with ${response.status}`);
  return response.json();
}

async function makeCommand(instanceId, operationId, generation, action, payload = {}) {
  const command = {
    document_type: "instance-cell-command",
    schema_version: "1",
    operation_id: operationId,
    instance_id: instanceId,
    generation,
    action,
    request_hash: "",
    issued_at: new Date().toISOString(),
    payload,
  };
  command.request_hash = await requestHash(command);
  return command;
}

async function requestHash(command) {
  const canonical = canonicalJson({
    operation_id: command.operation_id,
    instance_id: command.instance_id,
    generation: command.generation,
    action: command.action,
    payload: command.payload,
  });
  return hex(await crypto.subtle.digest("SHA-256", encoder.encode(canonical)));
}

async function signedRequest(path, options) {
  const method = options.body === undefined ? "GET" : "POST";
  const body = options.body === undefined ? "" : JSON.stringify(options.body);
  const timestamp = options.timestamp ?? new Date().toISOString();
  const nonce = `nonce-${++nonceSequence}`;
  const bodyHash = hex(await crypto.subtle.digest("SHA-256", encoder.encode(body)));
  const canonical = [
    method,
    path,
    options.operationId,
    String(options.generation),
    timestamp,
    nonce,
    bodyHash,
  ].join("\n");
  const key = await crypto.subtle.importKey(
    "raw",
    encoder.encode(options.key ?? KEY),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const signature = hex(await crypto.subtle.sign("HMAC", key, encoder.encode(canonical)));
  const headers = {
    "x-agentic-key-id": options.keyId ?? KEY_ID,
    "x-agentic-timestamp": timestamp,
    "x-agentic-nonce": nonce,
    "x-agentic-generation": String(options.generation),
    "x-agentic-operation-id": options.operationId,
    "x-agentic-body-sha256": bodyHash,
    "x-agentic-signature": signature,
  };
  if (body) headers["content-type"] = "application/json";
  return new Request(`https://cell.test${path}`, {
    method,
    headers,
    body: body || undefined,
  });
}

function canonicalJson(value) {
  if (value === null || typeof value === "boolean" || typeof value === "string" || typeof value === "number") {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
}

function hex(buffer) {
  return [...new Uint8Array(buffer)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}
