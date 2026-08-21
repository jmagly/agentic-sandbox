import assert from "node:assert/strict";
import { createHash, createHmac, randomUUID } from "node:crypto";
import { chmodSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";

import { sendWorkerCommand } from "../../../scripts/celld-worker-client.mjs";

const root = join("/dev/shm", `agentic-worker-client-${randomUUID()}`);
const varsFile = join(root, "worker-vars");
const key = "k".repeat(43);

test.before(() => {
  mkdirSync(root, { mode: 0o700 });
  writeFileSync(varsFile, `CELL_AUTH_KEY_ID=run-${"a".repeat(20)}\nCELL_AUTH_KEY=${key}\nMANAGEMENT_URL=https://management.internal/\n`, { mode: 0o600 });
  chmodSync(varsFile, 0o600);
});

test.after(() => rmSync(root, { recursive: true, force: false }));

test("Worker command client binds canonical payload and returns no authentication material", async () => {
  let capturedSignature;
  const result = await sendWorkerCommand({
    endpoint: "http://127.0.0.1:18080",
    varsFile,
    instanceId: "instance-a",
    operationId: "operation-a",
    generation: 1,
    action: "provision",
    payload: { runtime: "docker", name: "instance-a" },
    now: new Date("2026-08-21T09:00:00Z"),
    nonce: "b".repeat(32),
    fetcher: async (url, init) => {
      const body = JSON.parse(init.body);
      assert.equal(url.href, "http://127.0.0.1:18080/instance-cells/instance-a/commands");
      assert.equal(body.request_hash, createHash("sha256").update('{"action":"provision","generation":1,"instance_id":"instance-a","operation_id":"operation-a","payload":{"name":"instance-a","runtime":"docker"}}').digest("hex"));
      const headers = init.headers;
      const canonical = ["POST", url.pathname, "operation-a", "1", "2026-08-21T09:00:00.000Z", "b".repeat(32), createHash("sha256").update(init.body).digest("hex")].join("\n");
      capturedSignature = headers["x-agentic-signature"];
      assert.equal(capturedSignature, createHmac("sha256", key).update(canonical).digest("hex"));
      return Response.json({ operation_id: body.operation_id, status: "accepted" }, { status: 202 });
    },
  });
  assert.deepEqual(result, { status: 202, body: { operation_id: "operation-a", status: "accepted" } });
  assert.ok(!JSON.stringify(result).includes(key));
  assert.ok(!JSON.stringify(result).includes(capturedSignature));
});

test("Worker command client rejects unsafe origins, weak files, and oversized evidence", async () => {
  await assert.rejects(sendWorkerCommand({ endpoint: "http://worker.internal:8080", varsFile, instanceId: "a", operationId: "a", generation: 1, action: "observe", payload: {}, fetcher: async () => Response.json({}) }), /exact loopback origin/);
  chmodSync(varsFile, 0o644);
  await assert.rejects(sendWorkerCommand({ endpoint: "http://127.0.0.1:18080", varsFile, instanceId: "a", operationId: "a", generation: 1, action: "observe", payload: {}, fetcher: async () => Response.json({}) }), /protected regular/);
  chmodSync(varsFile, 0o600);
  await assert.rejects(sendWorkerCommand({ endpoint: "http://127.0.0.1:18080", varsFile, instanceId: "a", operationId: "a", generation: 1, action: "observe", payload: {}, fetcher: async () => new Response(JSON.stringify({ value: "x".repeat(5000) }), { status: 200 }) }), /evidence bound/);
  await assert.rejects(sendWorkerCommand({ endpoint: "http://127.0.0.1:18080", varsFile, instanceId: "a", operationId: "a", generation: 1, action: "observe", payload: {}, fetcher: async (_url, init) => Response.json({ echoed: init.headers["x-agentic-signature"] }) }), /contains authentication material/);
});
