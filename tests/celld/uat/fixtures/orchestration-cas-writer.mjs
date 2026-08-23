import { existsSync, writeFileSync } from "node:fs";

import {
  commitOrchestrationInventory,
  loadProtectedOrchestrationInventory,
  planOrchestrationMutation,
} from "../../../../scripts/celld-orchestration-inventory.mjs";

const [configJson, instanceId, readyPath, releasePath, loadedPath] = process.argv.slice(2);

try {
  const config = JSON.parse(configJson);
  const inventory = loadProtectedOrchestrationInventory(config.inventory_path, config);
  const expectedJournalHeadSha256 = inventory.journal_head_sha256;
  if (loadedPath) writeFileSync(loadedPath, `${process.pid}\n`, { flag: "wx", mode: 0o600 });
  planOrchestrationMutation(inventory, {
    mutation: "provider_action",
    scenarioId: "UAT-CELLD-003",
    subjectType: "provider_resource",
    subject: {
      instance_id: instanceId,
      name: `celld-cas-${instanceId.slice(-4)}`,
      substrate: "docker",
      operation_id: `cas-${instanceId}`,
      generation: 1,
      action: "provision",
      request_sha256: "a".repeat(64),
    },
  });
  commitOrchestrationInventory(config.inventory_path, inventory, {
    config,
    expectedJournalHeadSha256,
    afterCompareBeforeReplace: () => {
      writeFileSync(readyPath, `${process.pid}\n`, { flag: "wx", mode: 0o600 });
      const deadline = Date.now() + 5_000;
      while (!existsSync(releasePath)) {
        if (Date.now() > deadline) throw new Error("CAS test release deadline expired");
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 10);
      }
    },
  });
  process.stdout.write("COMMITTED\n");
} catch (error) {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = /stale|compare-and-swap/.test(error.message) ? 10 : 11;
}
