import { cloudflareTest } from "@cloudflare/vitest-pool-workers";
import { createHash, createHmac, timingSafeEqual } from "node:crypto";
import { defineConfig } from "vitest/config";

const callbackAttempts = new Map();
const rotationConfiguredAt = Date.now();

export default defineConfig({
  plugins: [
    cloudflareTest({
      wrangler: { configPath: "./wrangler.json" },
      miniflare: {
        bindings: {
          CELL_AUTH_KEY_ID: "test-active",
          CELL_AUTH_KEY: "01234567890123456789012345678901",
          CELL_AUTH_PREVIOUS_KEY_ID: "test-previous",
          CELL_AUTH_PREVIOUS_KEY: "abcdefghijklmnopqrstuvwxyzABCDEF",
          CELL_AUTH_PREVIOUS_VALID_FROM: new Date(rotationConfiguredAt - 30_000).toISOString(),
          CELL_AUTH_PREVIOUS_VALID_UNTIL: new Date(rotationConfiguredAt + 14 * 60_000).toISOString(),
        },
        serviceBindings: {
          MANAGEMENT: async (request) => {
            const rawBody = await request.text();
            const body = JSON.parse(rawBody);
            const operationId = body?.effect?.operation_id;
            const bodyDigest = createHash("sha256").update(rawBody).digest("hex");
            const canonical = [
              request.method,
              new URL(request.url).pathname,
              request.headers.get("x-agentic-operation-id"),
              request.headers.get("x-agentic-generation"),
              request.headers.get("x-agentic-timestamp"),
              request.headers.get("x-agentic-nonce"),
              bodyDigest,
            ].join("\n");
            const expected = Buffer.from(createHmac("sha256", "01234567890123456789012345678901").update(canonical).digest("hex"));
            const supplied = Buffer.from(request.headers.get("x-agentic-signature") || "");
            const signed = supplied.length === expected.length && timingSafeEqual(supplied, expected) &&
              request.headers.get("x-agentic-key-id") === "test-active" &&
              request.headers.get("x-agentic-body-sha256") === bodyDigest;
            const originalIdentity =
              typeof operationId === "string" &&
              request.headers.get("idempotency-key") === operationId && signed;
            const attempt = (callbackAttempts.get(operationId) || 0) + 1;
            callbackAttempts.set(operationId, attempt);
            const effectStatus = originalIdentity && operationId === "op-dispatched" && attempt === 1
              ? "dispatched"
              : originalIdentity ? "succeeded" : "rejected";
            return Response.json(
              {
                accepted: originalIdentity,
                operation_id: operationId,
                status: effectStatus,
                management_operation_id: originalIdentity ? `management-${operationId}` : null,
                terminal_code: originalIdentity ? "provider.effect_succeeded" : "provider.effect_rejected",
              },
              { status: originalIdentity ? 200 : 409 },
            );
          },
        },
      },
    }),
  ],
  test: {
    include: ["test/**/*.test.mjs"],
    maxWorkers: 1,
    testTimeout: 15_000,
  },
});
