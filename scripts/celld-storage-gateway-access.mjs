#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createConnection, createServer } from "node:net";
import { basename } from "node:path";

import { fixtureEnvironment } from "./celld-seaweedfs-fixture.mjs";

const CONTAINER_ID = /^[0-9a-f]{64}$/;
const SERVICE = /^s3gateway[1-9][0-9]*$/;

function run(program, args, options = {}) {
  const result = spawnSync(program, args, { encoding: "utf8", shell: false, ...options });
  if (result.error || result.status !== 0) throw new Error(`${basename(program)} failed`);
  return result.stdout.trim();
}

function accessError(service, reasonCode) {
  const error = new Error(`could not establish protected access to ${service}`);
  error.reasonCode = reasonCode;
  return error;
}

function parseOne(value) {
  try {
    const parsed = JSON.parse(value);
    return Array.isArray(parsed) && parsed.length === 1 ? parsed[0] : null;
  } catch {
    return null;
  }
}

function ipv4Octets(value) {
  if (!/^(?:0|[1-9][0-9]{0,2})(?:\.(?:0|[1-9][0-9]{0,2})){3}$/.test(String(value ?? ""))) return null;
  const octets = value.split(".").map(Number);
  return octets.every((part) => part <= 255) ? octets : null;
}

export function isPrivateIpv4(value, { allowLoopback = false } = {}) {
  const octets = ipv4Octets(value);
  if (!octets) return false;
  if (allowLoopback && octets[0] === 127) return true;
  return octets[0] === 10
    || (octets[0] === 172 && octets[1] >= 16 && octets[1] <= 31)
    || (octets[0] === 192 && octets[1] === 168);
}

export function inspectGatewayTarget(config, service, { runner = run } = {}) {
  if (!SERVICE.test(service)) throw accessError("gateway", "gateway-service-unavailable");
  const environment = fixtureEnvironment(config);
  const ids = runner("docker", ["compose", "-f", config.compose_file, "-p", config.project, "ps", "-q", service], { env: environment, timeout: 30_000 })
    .split(/\r?\n/).filter(Boolean);
  if (ids.length !== 1 || !CONTAINER_ID.test(ids[0])) throw accessError(service, "gateway-service-unavailable");
  const container = parseOne(runner("docker", ["inspect", "--type", "container", ids[0]], { timeout: 30_000 }));
  if (!container || container.Id !== ids[0]) throw accessError(service, "gateway-service-unavailable");
  const labels = container.Config?.Labels ?? {};
  if (labels["com.docker.compose.project"] !== config.project
      || labels["com.docker.compose.service"] !== service
      || labels["dev.agentic-sandbox.run"] !== config.run_id
      || labels["dev.agentic-sandbox.scope"] !== "celld-qualification") {
    throw accessError(service, "gateway-ownership-invalid");
  }
  const engineBindings = container.NetworkSettings?.Ports?.["8334/tcp"];
  const requestedBindings = container.HostConfig?.PortBindings?.["8334/tcp"];
  if ((Array.isArray(engineBindings) && engineBindings.length > 0)
      || (Array.isArray(requestedBindings) && requestedBindings.length > 0)) {
    throw accessError(service, "gateway-unexpected-publication");
  }
  const expectedNetworkName = `${config.project}_storage-private`;
  const networks = container.NetworkSettings?.Networks ?? {};
  const attachment = networks[expectedNetworkName];
  if (Object.keys(networks).length !== 1 || !attachment || !CONTAINER_ID.test(attachment.NetworkID ?? "")) {
    throw accessError(service, "gateway-network-ambiguous");
  }
  const address = String(attachment.IPAddress ?? "");
  if (!isPrivateIpv4(address) || !Array.isArray(attachment.Aliases) || !attachment.Aliases.includes(service)) {
    throw accessError(service, "gateway-private-address-unavailable");
  }
  const network = parseOne(runner("docker", ["network", "inspect", attachment.NetworkID], { timeout: 30_000 }));
  const networkLabels = network?.Labels ?? {};
  const member = network?.Containers?.[container.Id];
  const containerName = String(container.Name ?? "").replace(/^\//, "");
  if (!network
      || network.Id !== attachment.NetworkID
      || network.Name !== expectedNetworkName
      || network.Driver !== "bridge"
      || network.Scope !== "local"
      || network.Internal !== true
      || network.Ingress === true
      || networkLabels["com.docker.compose.project"] !== config.project
      || networkLabels["com.docker.compose.network"] !== "storage-private"
      || networkLabels["dev.agentic-sandbox.run"] !== config.run_id
      || networkLabels["dev.agentic-sandbox.scope"] !== "celld-qualification"
      || member?.Name !== containerName
      || !String(member?.IPv4Address ?? "").startsWith(`${address}/`)) {
    throw accessError(service, "gateway-network-ownership-invalid");
  }
  return { host: address, port: 8334 };
}

export async function startLoopbackForwarder({ host, port }, {
  serverFactory = createServer,
  connectionFactory = createConnection,
  scheme = "https",
} = {}) {
  if (!isPrivateIpv4(host, { allowLoopback: true }) || !Number.isSafeInteger(port) || port < 1 || port > 65_535) {
    throw accessError("gateway", "gateway-private-address-unavailable");
  }
  if (!["http", "https"].includes(scheme)) throw accessError("gateway", "gateway-forwarder-unavailable");
  const sockets = new Set();
  let listenerError = null;
  const markListenerError = (error) => { listenerError ??= error instanceof Error ? error : new Error("gateway forwarder failed"); };
  const server = serverFactory((downstream) => {
    const upstream = connectionFactory({ host, port });
    sockets.add(downstream);
    sockets.add(upstream);
    downstream.setNoDelay?.(true);
    upstream.setNoDelay?.(true);
    const destroyPeer = (peer) => { if (!peer.destroyed) peer.destroy(); };
    downstream.on("error", () => destroyPeer(upstream));
    upstream.on("error", () => { destroyPeer(downstream); });
    downstream.on("close", () => { sockets.delete(downstream); destroyPeer(upstream); });
    upstream.on("close", () => { sockets.delete(upstream); destroyPeer(downstream); });
    downstream.pipe(upstream);
    upstream.pipe(downstream);
  });
  let endpoint;
  try {
    endpoint = await new Promise((resolveEndpoint, reject) => {
      const failed = (error) => reject(error);
      server.once("error", failed);
      server.listen({ host: "127.0.0.1", port: 0, exclusive: true }, () => {
        server.off("error", failed);
        const address = server.address();
        if (!address || typeof address === "string" || address.address !== "127.0.0.1" || address.port < 1) {
          reject(new Error("loopback listener identity is invalid"));
          return;
        }
        resolveEndpoint(`${scheme}://127.0.0.1:${address.port}`);
      });
    });
  } catch {
    if (server.listening) server.close();
    throw accessError("gateway", "gateway-forwarder-unavailable");
  }
  server.on("error", (error) => {
    markListenerError(error);
    for (const socket of sockets) socket.destroy();
  });
  let closePromise = null;
  return {
    endpoint,
    close() {
      closePromise ??= (async () => {
        for (const socket of sockets) socket.destroy();
        let closeError = null;
        if (server.listening) {
          await new Promise((resolveClose) => {
            server.close((error) => { closeError = error ?? null; resolveClose(); });
          });
        }
        if (listenerError || closeError) throw accessError("gateway", "gateway-forwarder-failed");
      })();
      return closePromise;
    },
  };
}

export async function closeGatewayForwarders(forwarders) {
  const failures = [];
  for (const forwarder of [...forwarders].reverse()) {
    try { await forwarder.close(); } catch (error) { failures.push(error); }
  }
  if (failures.length) throw accessError("gateway", "gateway-forwarder-cleanup-failed");
}

export async function openStorageGatewayAccess(config, {
  services = ["s3gateway1"],
  runner = run,
  forwarderFactory = startLoopbackForwarder,
} = {}) {
  if (!Array.isArray(services) || services.length === 0 || new Set(services).size !== services.length || services.some((service) => !SERVICE.test(service))) {
    throw accessError("gateway", "gateway-service-unavailable");
  }
  const forwarders = [];
  try {
    for (const service of services) {
      const target = inspectGatewayTarget(config, service, { runner });
      forwarders.push(await forwarderFactory(target));
    }
  } catch (error) {
    try { await closeGatewayForwarders(forwarders); } catch { throw accessError("gateway", "gateway-forwarder-cleanup-failed"); }
    throw error;
  }
  return {
    endpoints: forwarders.map((forwarder) => forwarder.endpoint),
    close: () => closeGatewayForwarders(forwarders),
  };
}
