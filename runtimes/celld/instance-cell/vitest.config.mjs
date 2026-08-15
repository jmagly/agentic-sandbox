import { cloudflareTest } from "@cloudflare/vitest-pool-workers";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [
    cloudflareTest({
      wrangler: { configPath: "./wrangler.json" },
      miniflare: {
        bindings: {
          CELL_AUTH_KEY_ID: "test-active",
          CELL_AUTH_KEY: "01234567890123456789012345678901",
        },
        serviceBindings: {
          MANAGEMENT: async (request) => {
            const body = await request.json();
            const operationId = body?.effect?.operation_id;
            const originalIdentity =
              typeof operationId === "string" &&
              request.headers.get("idempotency-key") === operationId;
            return Response.json(
              { accepted: originalIdentity, operation_id: operationId },
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
