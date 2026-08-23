import { existsSync, writeFileSync } from "node:fs";

import { cleanupOrchestrationRoot } from "../../../../scripts/celld-live-orchestration.mjs";

const [configPath, readyPath, releasePath] = process.argv.slice(2);

try {
  cleanupOrchestrationRoot(configPath, {
    beforeRootDelete: () => {
      writeFileSync(readyPath, `${process.pid}\n`, { flag: "wx", mode: 0o600 });
      const deadline = Date.now() + 5_000;
      while (!existsSync(releasePath)) {
        if (Date.now() > deadline) throw new Error("cleanup test release deadline expired");
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 10);
      }
    },
  });
  process.stdout.write("CLEANED\n");
} catch (error) {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 12;
}
