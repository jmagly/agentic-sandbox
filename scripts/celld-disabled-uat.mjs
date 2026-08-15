#!/usr/bin/env node

import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
export const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");

export const DEFAULT_COMMANDS = Object.freeze([
  Object.freeze({
    id: "repository-regression",
    program: "make",
    args: Object.freeze(["test"]),
    timeout_ms: 20 * 60 * 1000,
  }),
]);

function write(stream, value) {
  if (stream && typeof stream.write === "function") stream.write(value);
}

function runCommand(definition, environment, stdout, stderr) {
  return new Promise((resolveResult) => {
    const startedAt = new Date();
    let settled = false;
    let timedOut = false;
    let timer;
    let child;

    const finish = (result) => {
      if (settled) return;
      settled = true;
      if (timer) clearTimeout(timer);
      resolveResult({
        id: definition.id,
        program: definition.program,
        args: [...definition.args],
        started_at: startedAt.toISOString(),
        ended_at: new Date().toISOString(),
        ...result,
      });
    };

    try {
      child = spawn(definition.program, definition.args, {
        cwd: REPO_ROOT,
        env: environment,
        shell: false,
        stdio: ["ignore", "pipe", "pipe"],
      });
    } catch (error) {
      finish({ status: "ERROR", exit_code: null, signal: null, reason: error.message });
      return;
    }

    child.stdout.on("data", (chunk) => write(stdout, chunk));
    child.stderr.on("data", (chunk) => write(stderr, chunk));
    child.on("error", (error) => finish({ status: "ERROR", exit_code: null, signal: null, reason: error.message }));
    child.on("close", (code, signal) => {
      const status = !timedOut && code === 0 ? "PASS" : "FAIL";
      const reason = timedOut ? `command exceeded ${definition.timeout_ms} ms` : code === 0 ? "command passed" : `command exited ${code ?? signal ?? "unknown"}`;
      finish({ status, exit_code: code, signal, reason });
    });

    timer = setTimeout(() => {
      timedOut = true;
      child.kill("SIGTERM");
      setTimeout(() => child.kill("SIGKILL"), 5_000).unref();
    }, definition.timeout_ms);
    timer.unref();
  });
}

async function startContactRecorder() {
  let connectionCount = 0;
  let requestCount = 0;
  const sockets = new Set();
  const server = createServer((_request, response) => {
    requestCount += 1;
    response.writeHead(503, { "content-type": "text/plain", connection: "close" });
    response.end("Celld disabled-path contact recorder\n");
  });
  server.on("connection", (socket) => {
    connectionCount += 1;
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
  });
  await new Promise((resolveListen, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("contact recorder did not bind a TCP address");
  return {
    endpoint: `http://127.0.0.1:${address.port}`,
    counts: () => ({ connection_count: connectionCount, request_count: requestCount }),
    close: async () => {
      for (const socket of sockets) socket.destroy();
      await new Promise((resolveClose) => server.close(resolveClose));
    },
  };
}

export async function runDisabledCompatibility({
  commands = DEFAULT_COMMANDS,
  environment = process.env,
  stdout = process.stdout,
  stderr = process.stderr,
} = {}) {
  const startedAt = new Date();
  const recorder = await startContactRecorder();
  const childEnvironment = {
    ...environment,
    AGENTIC_CELLD_ENABLED: "false",
    AGENTIC_CELLD_ENDPOINT: recorder.endpoint,
  };
  const commandResults = [];
  try {
    for (const command of commands) {
      const result = await runCommand(command, childEnvironment, stdout, stderr);
      commandResults.push(result);
      if (result.status !== "PASS") break;
    }
  } finally {
    await recorder.close();
  }

  const contact = recorder.counts();
  const commandsPassed = commandResults.length === commands.length && commandResults.every((result) => result.status === "PASS");
  const zeroContact = contact.connection_count === 0 && contact.request_count === 0;
  const status = commandsPassed && zeroContact ? "PASS" : "FAIL";
  const result = {
    schema_version: "agentic-sandbox.celld-disabled-uat/v1",
    status,
    started_at: startedAt.toISOString(),
    ended_at: new Date().toISOString(),
    configured_enabled: false,
    endpoint_scope: "loopback-contact-recorder",
    ...contact,
    commands: commandResults,
    assertions: {
      disabled_configuration: commandsPassed,
      zero_endpoint_contact: zeroContact,
      repository_regression: commandsPassed,
    },
  };
  write(stdout, `${JSON.stringify(result)}\n`);
  return result;
}

function usage() {
  return "Usage: node scripts/celld-disabled-uat.mjs\nRuns the full repository regression with Celld disabled and requires zero endpoint contacts.";
}

export async function main(argv = process.argv.slice(2)) {
  if (argv.length === 1 && argv[0] === "--help") {
    console.log(usage());
    return 0;
  }
  if (argv.length > 0) {
    console.error(usage());
    return 3;
  }
  try {
    const result = await runDisabledCompatibility();
    return result.status === "PASS" ? 0 : 1;
  } catch (error) {
    console.error(`disabled UAT failed: ${error.message}`);
    return 3;
  }
}

if (process.argv[1] && resolve(process.argv[1]) === SCRIPT_PATH) process.exitCode = await main();
