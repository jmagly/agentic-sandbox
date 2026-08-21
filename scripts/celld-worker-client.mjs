import { createHash, createHmac, randomBytes } from "node:crypto";
import { lstatSync, readFileSync } from "node:fs";

const OPERATION_ID = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;
const ACTIONS = new Set(["provision", "start", "stop", "destroy", "observe", "repair"]);

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function canonicalJson(value) {
  if (value === null || typeof value === "boolean" || typeof value === "string") return JSON.stringify(value);
  if (typeof value === "number") {
    if (!Number.isFinite(value)) throw new Error("non-finite numbers are not valid JSON");
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value && typeof value === "object") return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
  throw new Error("unsupported JSON value");
}

function readWorkerVars(path) {
  const metadata = lstatSync(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) throw new Error("Worker vars file must be a protected regular non-symlink file");
  const values = new Map();
  for (const line of readFileSync(path, "utf8").split(/\r?\n/)) {
    if (line === "") continue;
    const match = /^([A-Z][A-Z0-9_]*)=(.*)$/.exec(line);
    if (!match || values.has(match[1])) throw new Error("Worker vars file is malformed");
    values.set(match[1], match[2]);
  }
  if (values.size !== 3 || !/^run-[a-f0-9]{20}$/.test(values.get("CELL_AUTH_KEY_ID") ?? "") || Buffer.byteLength(values.get("CELL_AUTH_KEY") ?? "") < 32 || values.get("MANAGEMENT_URL") !== "https://management.internal/") {
    throw new Error("Worker vars file does not contain the exact required values");
  }
  return { keyId: values.get("CELL_AUTH_KEY_ID"), key: values.get("CELL_AUTH_KEY") };
}

function loopbackOrigin(value) {
  let endpoint;
  try { endpoint = new URL(value); }
  catch { throw new Error("Worker endpoint must be an exact loopback origin"); }
  if (endpoint.protocol !== "http:" || endpoint.hostname !== "127.0.0.1" || endpoint.username || endpoint.password || endpoint.pathname !== "/" || endpoint.search || endpoint.hash || endpoint.origin !== value) {
    throw new Error("Worker endpoint must be an exact loopback origin");
  }
  return endpoint;
}

function checkedPath(path) {
  if (typeof path !== "string" || !path.startsWith("/") || path.startsWith("//") || path.includes("?") || path.includes("#")) throw new Error("Worker request path is invalid");
  return path;
}

function checkedInstanceId(instanceId) {
  if (typeof instanceId !== "string" || instanceId.length === 0 || instanceId.length > 128 || instanceId.includes("/") || instanceId.includes("\\")) throw new Error("Worker instance ID is invalid");
  return instanceId;
}

async function boundedJson(response) {
  const declared = response.headers.get("content-length");
  if (declared !== null && (!/^\d+$/.test(declared) || Number(declared) > 4096)) throw new Error("Worker response exceeds the evidence bound");
  const bytes = Buffer.from(await response.arrayBuffer());
  if (bytes.length > 4096) throw new Error("Worker response exceeds the evidence bound");
  try { return JSON.parse(bytes.toString("utf8")); }
  catch { throw new Error("Worker response is not JSON"); }
}

function signedRequest({ method, path, operationId, generation, body, varsFile, now, nonce }) {
  if (!OPERATION_ID.test(operationId ?? "")) throw new Error("Worker operation ID is invalid");
  if (!Number.isSafeInteger(generation) || generation < 1) throw new Error("Worker generation is invalid");
  if (!/^[a-f0-9]{32}$/.test(nonce ?? "")) throw new Error("Worker request nonce is invalid");
  const { keyId, key } = readWorkerVars(varsFile);
  const digest = sha256(body);
  const canonical = [method, path, operationId, String(generation), now.toISOString(), nonce, digest].join("\n");
  const signature = createHmac("sha256", key).update(canonical).digest("hex");
  const headers = {
    "x-agentic-key-id": keyId,
    "x-agentic-timestamp": now.toISOString(),
    "x-agentic-nonce": nonce,
    "x-agentic-generation": String(generation),
    "x-agentic-operation-id": operationId,
    "x-agentic-body-sha256": digest,
    "x-agentic-signature": signature,
  };
  return { headers, restricted: [key, signature, nonce] };
}

async function request(fetcher, endpoint, path, init, restricted) {
  const response = await fetcher(new URL(path, endpoint), { ...init, redirect: "error", signal: AbortSignal.timeout(10_000) });
  const result = { status: response.status, body: await boundedJson(response) };
  const serialized = JSON.stringify(result);
  if (restricted.some((value) => serialized.includes(value))) throw new Error("Worker response contains authentication material");
  return result;
}

export async function probeWorkerAuthentication({ endpoint, varsFile, instanceId, operationId, fetcher = fetch, now = new Date(), nonce = randomBytes(16).toString("hex") }) {
  const origin = loopbackOrigin(endpoint);
  const path = checkedPath(`/instance-cells/${encodeURIComponent(checkedInstanceId(instanceId))}`);
  const signed = signedRequest({ method: "GET", path, operationId, generation: 1, body: "", varsFile, now, nonce });
  const forged = await request(fetcher, origin, path, { method: "GET", headers: { ...signed.headers, "x-agentic-signature": "0".repeat(64) } }, signed.restricted);
  const valid = await request(fetcher, origin, path, { method: "GET", headers: signed.headers }, signed.restricted);
  const replay = await request(fetcher, origin, path, { method: "GET", headers: signed.headers }, signed.restricted);
  const checks = {
    signed_missing_cell: { status: valid.status, code: valid.body?.error?.code },
    forged_signature: { status: forged.status, code: forged.body?.error?.code },
    nonce_replay: { status: replay.status, code: replay.body?.error?.code },
  };
  if (JSON.stringify(checks) !== JSON.stringify({
    signed_missing_cell: { status: 404, code: "cell.missing" },
    forged_signature: { status: 401, code: "cell.signature_invalid" },
    nonce_replay: { status: 409, code: "cell.signature_replayed" },
  })) throw new Error("deployed Worker auth probe did not return the exact expected outcomes");
  return checks;
}

export async function sendWorkerCommand({ endpoint, varsFile, instanceId, operationId, generation, action, payload, fetcher = fetch, now = new Date(), nonce = randomBytes(16).toString("hex") }) {
  const origin = loopbackOrigin(endpoint);
  checkedInstanceId(instanceId);
  if (!ACTIONS.has(action) || !payload || typeof payload !== "object" || Array.isArray(payload)) throw new Error("Worker command action or payload is invalid");
  const requestHash = sha256(canonicalJson({ operation_id: operationId, instance_id: instanceId, generation, action, payload }));
  const command = { document_type: "instance-cell-command", schema_version: "1", operation_id: operationId, instance_id: instanceId, generation, action, request_hash: requestHash, issued_at: now.toISOString(), payload };
  const body = JSON.stringify(command);
  const path = checkedPath(`/instance-cells/${encodeURIComponent(instanceId)}/commands`);
  const signed = signedRequest({ method: "POST", path, operationId, generation, body, varsFile, now, nonce });
  return request(fetcher, origin, path, { method: "POST", headers: { "content-type": "application/json", ...signed.headers }, body }, signed.restricted);
}

export async function getWorkerCell({ endpoint, varsFile, instanceId, operationId, generation, fetcher = fetch, now = new Date(), nonce = randomBytes(16).toString("hex") }) {
  const origin = loopbackOrigin(endpoint);
  const path = checkedPath(`/instance-cells/${encodeURIComponent(checkedInstanceId(instanceId))}`);
  const signed = signedRequest({ method: "GET", path, operationId, generation, body: "", varsFile, now, nonce });
  return request(fetcher, origin, path, { method: "GET", headers: signed.headers }, signed.restricted);
}

export async function reconcileWorkerCell({ endpoint, varsFile, instanceId, operationId, generation, managementGeneration, fetcher = fetch, now = new Date(), nonce = randomBytes(16).toString("hex") }) {
  const origin = loopbackOrigin(endpoint);
  checkedInstanceId(instanceId);
  if (!Number.isSafeInteger(managementGeneration) || managementGeneration < 1) throw new Error("management generation is invalid");
  const path = checkedPath(`/instance-cells/${encodeURIComponent(instanceId)}/reconcile`);
  const body = JSON.stringify({ management_generation: managementGeneration });
  const signed = signedRequest({ method: "POST", path, operationId, generation, body, varsFile, now, nonce });
  return request(fetcher, origin, path, { method: "POST", headers: { "content-type": "application/json", ...signed.headers }, body }, signed.restricted);
}
