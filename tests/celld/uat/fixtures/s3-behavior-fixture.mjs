import { createHash } from "node:crypto";
import http from "node:http";

const VARIANTS = new Set([
  "correct",
  "ignored-if-none-match",
  "ignored-or-stale-if-match",
  "gateway-local-locking",
  "stale-first-read",
  "misleading-outcome",
]);

function etag(bytes) {
  return `"${createHash("sha256").update(bytes).digest("hex")}"`;
}

function error(response, status, code) {
  const body = Buffer.from(`<Error><Code>${code}</Code></Error>`);
  response.writeHead(status, { "content-type": "application/xml", "content-length": body.length });
  response.end(body);
}

function handler(store, variant) {
  const staleReads = new Set();
  return (request, response) => {
    const key = new URL(request.url, "http://fixture").pathname;
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.on("end", () => {
      const current = store.get(key);
      if (request.method === "PUT") {
        const body = Buffer.concat(chunks);
        if (request.headers["if-none-match"] === "*" && current && variant !== "ignored-if-none-match") return error(response, 412, "PreconditionFailed");
        if (request.headers["if-match"] && request.headers["if-match"] !== current?.etag && variant !== "ignored-or-stale-if-match") return error(response, 412, "PreconditionFailed");
        store.set(key, { bytes: body, etag: etag(body), previous: current });
        if (variant === "stale-first-read") staleReads.add(key);
        if (variant === "misleading-outcome") return error(response, 500, "InternalError");
        response.writeHead(200, { etag: etag(body) });
        return response.end();
      }
      if (request.method === "GET") {
        if (!current) return error(response, 404, "NoSuchKey");
        if (variant === "stale-first-read" && staleReads.delete(key)) {
          if (!current.previous) return error(response, 404, "NoSuchKey");
          response.writeHead(200, { etag: current.previous.etag, "content-length": current.previous.bytes.length });
          return response.end(current.previous.bytes);
        }
        response.writeHead(200, { etag: current.etag, "content-length": current.bytes.length });
        return response.end(current.bytes);
      }
      if (request.method === "HEAD") {
        if (!current) return error(response, 404, "NoSuchKey");
        response.writeHead(200, { etag: current.etag, "content-length": current.bytes.length });
        return response.end();
      }
      if (request.method === "DELETE") {
        store.delete(key);
        response.writeHead(204);
        return response.end();
      }
      return error(response, 405, "MethodNotAllowed");
    });
  };
}

export async function startS3BehaviorFixture(variant = "correct") {
  if (!VARIANTS.has(variant)) throw new Error(`unsupported fixture variant: ${variant}`);
  const gatewayCount = variant === "gateway-local-locking" ? 2 : 1;
  const shared = new Map();
  const servers = [];
  const endpoints = [];
  for (let index = 0; index < gatewayCount; index += 1) {
    const store = variant === "gateway-local-locking" ? new Map() : shared;
    const server = http.createServer(handler(store, variant));
    await new Promise((resolve, reject) => {
      server.once("error", reject);
      server.listen(0, "127.0.0.1", resolve);
    });
    const address = server.address();
    endpoints.push(`http://127.0.0.1:${address.port}`);
    servers.push(server);
  }
  return {
    endpoints,
    async close() {
      await Promise.all(servers.map((server) => new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()))));
    },
  };
}
