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

test("disabled E2E uses a disposable VM contract and fails closed on incomplete cleanup", async () => {
  const source = `
    const valid = process.env.TEST_VM === ""
      && process.env.E2E_CLEANUP_VM === "1"
      && process.env.E2E_REUSE_VM === "0"
      && /^\\d+$/.test(process.env.GITHUB_RUN_ID)
      && process.env.GITHUB_RUN_ID === process.env.GITEA_RUN_ID;
    process.exit(valid ? 0 : 9);
  `;
  let cleanupVm;
  const result = await runDisabledCompatibility({
    commands: [nodeCommand("end-to-end-regression", source)],
    cleanupVerifier: (vmName) => {
      cleanupVm = vmName;
      return { status: "failed", disposable_vm_name: vmName, libvirt_domain_absent: false, storage_absent: true };
    },
    stdout: sink,
    stderr: sink,
  });
  assert.equal(result.commands[0].status, "PASS");
  assert.equal(result.status, "FAIL");
  assert.match(cleanupVm, /^agentic-e2e-\d+$/);
  assert.equal(result.cleanup.status, "failed");
});
