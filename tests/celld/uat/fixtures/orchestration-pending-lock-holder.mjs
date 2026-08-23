#!/usr/bin/env node

import { closeSync, fsyncSync, mkdirSync, openSync, readFileSync } from "node:fs";
import { join } from "node:path";

const [root, nonce] = process.argv.slice(2);
if (!root || !/^[0-9a-f]{16}$/.test(nonce ?? "")) throw new Error("root and a 16-hex nonce are required");

const stat = readFileSync(`/proc/${process.pid}/stat`, "utf8");
const processStartTimeTicks = stat.slice(stat.lastIndexOf(")") + 2).trim().split(/\s+/)[19];
if (!/^[0-9]+$/.test(processStartTimeTicks ?? "")) throw new Error("pending lock holder process identity is unavailable");

const candidate = join(root, `.orchestration-inventory.lock.pending-${process.pid}-${nonce}`);
mkdirSync(candidate, { mode: 0o700 });
const candidateDescriptor = openSync(candidate, "r");
try { fsyncSync(candidateDescriptor); } finally { closeSync(candidateDescriptor); }
const rootDescriptor = openSync(root, "r");
try { fsyncSync(rootDescriptor); } finally { closeSync(rootDescriptor); }

process.stdout.write(`${JSON.stringify({ pid: process.pid, process_start_time_ticks: processStartTimeTicks, candidate })}\n`);
setInterval(() => {}, 60_000);
