#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin"

cat >"$TMP/bin/docker" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${FAKE_DOCKER_LOG:?}"

if [[ "$*" == "buildx imagetools create --prefer-index=false --tag "* ]]; then
  exit 0
fi

if [[ "$*" == "buildx imagetools inspect --raw "* ]]; then
  ref="${*: -1}"
  if [[ "${FAKE_DEST_MISSING_ARM64:-0}" == 1 && "$ref" == *destination* ]]; then
    cat <<'JSON'
{"manifests":[{"digest":"sha256:amd64","platform":{"os":"linux","architecture":"amd64"}}]}
JSON
  else
    cat <<'JSON'
{"manifests":[{"digest":"sha256:amd64","platform":{"os":"linux","architecture":"amd64"}},{"digest":"sha256:arm64","platform":{"os":"linux","architecture":"arm64","variant":"v8"}},{"digest":"sha256:attestation","platform":{"os":"unknown","architecture":"unknown"}}]}
JSON
  fi
  exit 0
fi

if [[ "$*" == "buildx imagetools inspect "* ]]; then
  echo "Digest: sha256:index"
  exit 0
fi

echo "unexpected docker invocation: $*" >&2
exit 1
FAKE
chmod +x "$TMP/bin/docker"

export PATH="$TMP/bin:$PATH"
export FAKE_DOCKER_LOG="$TMP/docker.log"
export OCI_MIRROR_ATTEMPTS=1
export OCI_MIRROR_DELAY_SECONDS=0

evidence="$TMP/evidence.jsonl"
"$ROOT/scripts/mirror-oci-index.sh" \
  --source registry.invalid/source:v1 \
  --destination registry.invalid/destination:v1 \
  --required-platform linux/amd64 \
  --required-platform linux/arm64 \
  --evidence "$evidence" >/dev/null

grep -F \
  "buildx imagetools create --prefer-index=false --tag registry.invalid/destination:v1 registry.invalid/source:v1" \
  "$FAKE_DOCKER_LOG" >/dev/null
if grep -Eq '(^| )(pull|tag|push)( |$)' "$FAKE_DOCKER_LOG"; then
  echo "mirror helper must not materialize a runner-native image" >&2
  exit 1
fi
jq -e '
  .source == "registry.invalid/source:v1" and
  .destination == "registry.invalid/destination:v1" and
  ([.platforms[].platform] | index("linux/amd64")) != null and
  ([.platforms[].platform] | index("linux/arm64")) != null
' "$evidence" >/dev/null

if FAKE_DEST_MISSING_ARM64=1 "$ROOT/scripts/mirror-oci-index.sh" \
  --source registry.invalid/source:v1 \
  --destination registry.invalid/destination:v1 \
  --required-platform linux/amd64 \
  --required-platform linux/arm64 >/dev/null 2>&1; then
  echo "mirror helper accepted a destination with a missing arm64 manifest" >&2
  exit 1
fi

echo "OCI index mirror test passed"
