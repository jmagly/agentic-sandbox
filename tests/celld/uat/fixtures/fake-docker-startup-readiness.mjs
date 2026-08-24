#!/usr/bin/env node

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";

const args = process.argv.slice(2);
const statePath = process.env.CELLD_FAKE_DOCKER_STATE;
const configPath = process.env.CELLD_FAKE_FLEET_CONFIG;
const pinnedDiagnosisPath = process.env.CELLD_FAKE_PINNED_DIAGNOSIS;
const secret = process.env.CELLD_FAKE_DIAGNOSIS_SECRET ?? "fixture-secret";
const diagnosisMode = process.env.CELLD_FAKE_DIAGNOSIS_MODE ?? "partial";
if (!statePath || !configPath || !pinnedDiagnosisPath) throw new Error("fake Docker startup-readiness environment is incomplete");

const config = JSON.parse(readFileSync(configPath, "utf8"));
const storage = JSON.parse(readFileSync(config.storage_config_path, "utf8"));
const pinnedDiagnosis = JSON.parse(readFileSync(pinnedDiagnosisPath, "utf8"));
const state = existsSync(statePath)
  ? JSON.parse(readFileSync(statePath, "utf8"))
  : { containers: {}, commands: [], diagnosis_attempts: 0 };
state.port_attempts = state.port_attempts ?? 0;
state.commands.push(args);

function persist() {
  writeFileSync(statePath, `${JSON.stringify(state, null, 2)}\n`, { mode: 0o600 });
}

function labels() {
  return {
    "dev.agentic-sandbox.repository": "roctinam/agentic-sandbox",
    "dev.agentic-sandbox.workflow": "celld-qualification",
    "dev.agentic-sandbox.run": config.run_id,
    "dev.agentic-sandbox.scope": "celld-qualification",
  };
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function containerId(name) {
  return sha256(`${config.run_id}:${name}`);
}

function containerAddress(name) {
  const index = config.nodes.findIndex((node) => node.name === name);
  return `172.29.0.${20 + Math.max(index, 0)}`;
}

function output(value) {
  persist();
  process.stdout.write(`${value}\n`);
}

function diagnosisOutput(nodeIds) {
  return [
    pinnedDiagnosis.conditional_write_line,
    ...nodeIds.map((nodeId, index) => pinnedDiagnosis.signed_direct_peer_line_template
      .replace("{{NODE_ID}}", nodeId)
      .replace("{{ADDRESS}}", config.nodes[index % config.nodes.length].advertise)),
  ].join("\n");
}

if (args[0] === "network" && args[1] === "inspect") {
  const containers = {};
  for (const [name, container] of Object.entries(state.containers)) {
    if (!container.running || name.endsWith("-callback-relay")) continue;
    const id = containerId(name);
    const address = containerAddress(name);
    containers[id] = { Name: name, IPv4Address: `${address}/16` };
  }
  output(JSON.stringify([{
    Id: containerId(config.network.name),
    Name: config.network.name,
    Driver: "bridge",
    Scope: "local",
    Internal: true,
    Ingress: false,
    Labels: {
      "com.docker.compose.project": storage.project,
      "com.docker.compose.network": "storage-private",
      "dev.agentic-sandbox.run": storage.run_id,
      "dev.agentic-sandbox.scope": "celld-qualification",
    },
    IPAM: { Config: [{ Gateway: "172.29.0.1" }] },
    Containers: containers,
  }]));
} else if (args[0] === "pull") {
  output(config.pins.celld.image_ref);
} else if (args[0] === "image" && args[1] === "inspect") {
  output("[]");
} else if (args[0] === "inspect") {
  const container = state.containers[args[1]];
  if (!container) {
    persist();
    process.stderr.write("not found\n");
    process.exitCode = 1;
  } else {
    const id = containerId(args[1]);
    const address = containerAddress(args[1]);
    output(JSON.stringify([{
      Id: id,
      Name: `/${args[1]}`,
      Config: { Labels: container.labels, Image: container.image },
      HostConfig: { PortBindings: {} },
      NetworkSettings: {
        Ports: {},
        Networks: {
          [config.network.name]: {
            NetworkID: containerId(config.network.name),
            IPAddress: address,
            Aliases: [args[1]],
          },
        },
      },
      State: { Running: container.running },
    }]));
  }
} else if (args[0] === "create") {
  const name = args[args.indexOf("--name") + 1];
  state.containers[name] = { labels: labels(), image: config.pins.celld.image_ref, running: false };
  output(name);
} else if (args[0] === "start") {
  state.containers[args[1]].running = true;
  output(args[1]);
} else if (args[0] === "port") {
  state.port_attempts += 1;
  if (diagnosisMode === "unrelated-exit75") {
    persist();
    process.stderr.write(`${diagnosisOutput(config.nodes.map((node) => node.node_id))}\n${secret}\n`);
    process.exitCode = 75;
  } else if (diagnosisMode === "port-race-once" && state.port_attempts === 1) {
    persist();
    process.stderr.write(`transient port publication lag: ${secret}\n`);
    process.exitCode = 1;
  } else {
    const index = config.nodes.findIndex((node) => node.name === args[1]);
    output(`127.0.0.1:${18080 + index}`);
  }
} else if (args[0] === "exec") {
  state.diagnosis_attempts += 1;
  persist();
  const expectedNodeIds = config.nodes.map((node) => node.node_id);
  if (diagnosisMode === "stderr-spoof") {
    process.stderr.write(`${diagnosisOutput(expectedNodeIds)}\n${secret}\n`);
    process.exitCode = 1;
  } else if (diagnosisMode === "nontransient") {
    process.stdout.write(`${diagnosisOutput(expectedNodeIds.slice(0, 2))}\n`);
    process.stderr.write(`non-transient diagnostic failure: ${secret}\n`);
    process.exitCode = 42;
  } else if (diagnosisMode === "incomplete-converge") {
    const observedNodeIds = state.diagnosis_attempts === 1
      ? []
      : state.diagnosis_attempts === 2
        ? expectedNodeIds.slice(0, 2)
        : expectedNodeIds;
    process.stdout.write(`${diagnosisOutput(observedNodeIds)}\n`);
    if (state.diagnosis_attempts < 3) {
      process.stderr.write(`explicit peer readiness incomplete: ${secret}\n`);
      process.exitCode = 1;
    }
  } else if (diagnosisMode === "port-race-once") {
    process.stdout.write(`${diagnosisOutput(expectedNodeIds)}\n`);
  } else {
    process.stdout.write(`${diagnosisOutput(expectedNodeIds.slice(0, 2))}\n`);
    process.stderr.write(`explicit peer readiness incomplete: ${secret}\n`);
    process.exitCode = 1;
  }
} else if (args[0] === "rm") {
  delete state.containers[args.at(-1)];
  output(args.at(-1));
} else if (args[0] === "ps") {
  output(Object.keys(state.containers).join("\n"));
} else {
  persist();
  process.stderr.write(`unsupported fake Docker command: ${args.join(" ")}\n`);
  process.exitCode = 2;
}
