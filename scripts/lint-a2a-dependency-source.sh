#!/usr/bin/env bash
# Enforce the A2A SDK fork-as-update-gate recorded in the reviewed baseline.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

baseline=ci/a2a-sdk-baseline.json
manifest=management/agentic-sandbox-executor/Cargo.toml
lockfile=management/Cargo.lock

command -v jq >/dev/null

jq -e '
  .schema_version == "agentic-sandbox.a2a-sdk-baseline/v1"
  and (.reviewed_at | type == "string")
  and (.upstream.url | type == "string")
  and (.upstream.commit | test("^[0-9a-f]{40}$"))
  and (.mirror.url | type == "string")
  and (.mirror.tag | test("^agentic-sandbox-v[0-9]+[.][0-9]+[.][0-9]+$"))
  and (.mirror.tag_object | test("^[0-9a-f]{40}$"))
  and (.mirror.commit | test("^[0-9a-f]{40}$"))
  and .upstream.commit == .mirror.commit
' "$baseline" >/dev/null

mirror_url="$(jq -r '.mirror.url' "$baseline")"
mirror_tag="$(jq -r '.mirror.tag' "$baseline")"
mirror_commit="$(jq -r '.mirror.commit' "$baseline")"

for dependency in a2a a2a_client a2a_server; do
  line="$(grep -E "^${dependency}[[:space:]]*=" "$manifest")"
  if [[ "$line" != *"git = \"${mirror_url}\""* ]] \
    || [[ "$line" != *"tag = \"${mirror_tag}\""* ]]; then
    echo "✗ lint-a2a-dependency-source: ${dependency} bypasses the reviewed Gitea tag" >&2
    echo "  expected mirror=${mirror_url} tag=${mirror_tag}" >&2
    exit 1
  fi
done

if grep -Fq 'github.com/jmagly/a2a-rs' "$manifest"; then
  echo "✗ lint-a2a-dependency-source: executor manifest bypasses the Gitea update gate" >&2
  exit 1
fi

lock_source="git+${mirror_url}?tag=${mirror_tag}#${mirror_commit}"
lock_count="$(grep -F -c "source = \"${lock_source}\"" "$lockfile" || true)"
if [ "$lock_count" -ne 4 ]; then
  echo "✗ lint-a2a-dependency-source: lockfile does not resolve all four A2A packages to the reviewed mirror commit" >&2
  echo "  observed entries: ${lock_count} (expected 4)" >&2
  exit 1
fi

if grep -Fq 'git+https://github.com/jmagly/a2a-rs.git' "$lockfile"; then
  echo "✗ lint-a2a-dependency-source: lockfile contains the bypassed GitHub source" >&2
  exit 1
fi

echo "✓ lint-a2a-dependency-source: ${mirror_tag} resolves through the reviewed Gitea update gate"
