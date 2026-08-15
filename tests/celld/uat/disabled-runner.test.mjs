import assert from "node:assert/strict";
import test from "node:test";

import { runDisabledCompatibility } from "../../../scripts/celld-disabled-uat.mjs";

const sink = Object.freeze({ write: () => true });

function nodeCommand(id, source) {
  return { id, program: process.execPath, args: ["-e", source], timeout_ms: 5_000 };
}

test("disabled compatibility passes only when regression succeeds without endpoint contact", async () => {
  const result = await runDisabledCompatibility({
    commands: [nodeCommand("no-contact", "process.exit(0)")],
    stdout: sink,
    stderr: sink,
  });
  assert.equal(result.status, "PASS");
  assert.equal(result.connection_count, 0);
  assert.equal(result.request_count, 0);
  assert.deepEqual(result.assertions, {
    disabled_configuration: true,
    zero_endpoint_contact: true,
    repository_regression: true,
  });
});

test("disabled compatibility fails on any attempted Celld endpoint connection", async () => {
  const source = `
    const http = require("node:http");
    http.get(process.env.AGENTIC_CELLD_ENDPOINT, (response) => {
      response.resume();
      response.on("end", () => process.exit(0));
    }).on("error", () => process.exit(2));
  `;
  const result = await runDisabledCompatibility({
    commands: [nodeCommand("forbidden-contact", source)],
    stdout: sink,
    stderr: sink,
  });
  assert.equal(result.status, "FAIL");
  assert.ok(result.connection_count > 0);
  assert.equal(result.assertions.zero_endpoint_contact, false);
});

test("disabled compatibility fails when the repository regression command fails", async () => {
  const result = await runDisabledCompatibility({
    commands: [nodeCommand("failed-regression", "process.exit(7)")],
    stdout: sink,
    stderr: sink,
  });
  assert.equal(result.status, "FAIL");
  assert.equal(result.connection_count, 0);
  assert.equal(result.assertions.repository_regression, false);
  assert.equal(result.commands[0].exit_code, 7);
});
