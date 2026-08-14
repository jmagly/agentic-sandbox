#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

jq empty "$repo_root"/docs/contracts/celld/*.json
jq empty "$repo_root"/runtimes/celld/instance-cell/{bundle,wrangler}.json
jq empty "$repo_root"/deploy/celld/*.json

cargo test --manifest-path "$repo_root/management/Cargo.toml" celld --lib
cargo check --manifest-path "$repo_root/cli/Cargo.toml"

worker_digest=$(sha256sum "$repo_root/runtimes/celld/instance-cell/worker.mjs" | awk '{print $1}')
manifest_digest=$(jq -r '.digest' "$repo_root/runtimes/celld/instance-cell/bundle.json")
test "sha256:$worker_digest" = "$manifest_digest"
