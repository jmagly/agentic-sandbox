#!/usr/bin/env bash
# Copy an OCI manifest or index between registries without materializing one
# runner-native platform. Verify that every source child manifest survives.
set -euo pipefail

source_ref=""
destination_ref=""
evidence_file=""
declare -a required_platforms=()
attempts="${OCI_MIRROR_ATTEMPTS:-5}"
delay_seconds="${OCI_MIRROR_DELAY_SECONDS:-5}"

usage() {
  cat >&2 <<'USAGE'
usage: mirror-oci-index.sh --source REF --destination REF
                           [--required-platform OS/ARCH]...
                           [--evidence FILE]
USAGE
}

while (($#)); do
  case "$1" in
    --source) source_ref="${2:-}"; shift 2 ;;
    --destination) destination_ref="${2:-}"; shift 2 ;;
    --required-platform) required_platforms+=("${2:-}"); shift 2 ;;
    --evidence) evidence_file="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage; exit 2 ;;
  esac
done

[[ -n "$source_ref" && -n "$destination_ref" ]] || {
  usage
  exit 2
}
[[ "$attempts" =~ ^[1-9][0-9]*$ ]] || {
  echo "OCI_MIRROR_ATTEMPTS must be a positive integer" >&2
  exit 2
}
[[ "$delay_seconds" =~ ^[0-9]+$ ]] || {
  echo "OCI_MIRROR_DELAY_SECONDS must be a non-negative integer" >&2
  exit 2
}
for platform in "${required_platforms[@]}"; do
  [[ "$platform" =~ ^[a-z0-9._-]+/[a-z0-9._-]+$ ]] || {
    echo "invalid required platform: $platform" >&2
    exit 2
  }
done

retry() {
  local attempt status
  for ((attempt = 1; attempt <= attempts; attempt++)); do
    if "$@"; then
      return 0
    else
      status=$?
    fi
    if ((attempt == attempts)); then
      return "$status"
    fi
    echo "retrying OCI mirror operation (attempt ${attempt}/${attempts}): $*" >&2
    sleep "$delay_seconds"
  done
}

inspect() {
  local mode="$1" ref="$2"
  local -a command=(docker buildx imagetools inspect)
  [[ "$mode" == raw ]] && command+=(--raw)
  command+=("$ref")
  retry "${command[@]}"
}

digest_from_summary() {
  awk '/^Digest:/ {print $2; exit}'
}

echo "Mirroring OCI object without platform collapse:"
echo "  source:      $source_ref"
echo "  destination: $destination_ref"
retry docker buildx imagetools create \
  --prefer-index=false \
  --tag "$destination_ref" \
  "$source_ref"

source_raw="$(inspect raw "$source_ref")"
destination_raw="$(inspect raw "$destination_ref")"
source_digest="$(inspect summary "$source_ref" | digest_from_summary)"
destination_digest="$(inspect summary "$destination_ref" | digest_from_summary)"
[[ -n "$source_digest" && -n "$destination_digest" ]] || {
  echo "source or destination manifest digest is missing" >&2
  exit 1
}

source_is_index="$(jq -r 'if (.manifests | type) == "array" then "true" else "false" end' <<<"$source_raw")"
destination_is_index="$(jq -r 'if (.manifests | type) == "array" then "true" else "false" end' <<<"$destination_raw")"

if [[ "$source_is_index" == true ]]; then
  [[ "$destination_is_index" == true ]] || {
    echo "destination collapsed source OCI index to a single manifest" >&2
    exit 1
  }
  source_entries="$(jq -cS '
    [.manifests[] | {
      digest,
      platform: ((.platform.os // "unknown") + "/" + (.platform.architecture // "unknown")),
      variant: (.platform.variant // null)
    }] | sort_by(.digest, .platform, .variant)
  ' <<<"$source_raw")"
  destination_entries="$(jq -cS '
    [.manifests[] | {
      digest,
      platform: ((.platform.os // "unknown") + "/" + (.platform.architecture // "unknown")),
      variant: (.platform.variant // null)
    }] | sort_by(.digest, .platform, .variant)
  ' <<<"$destination_raw")"
  [[ "$source_entries" == "$destination_entries" ]] || {
    echo "destination OCI index does not preserve source child manifests" >&2
    exit 1
  }
else
  [[ "$destination_is_index" == false ]] || {
    echo "destination unexpectedly changed a single manifest into an index" >&2
    exit 1
  }
  [[ "$source_digest" == "$destination_digest" ]] || {
    echo "destination single-manifest digest differs from source" >&2
    exit 1
  }
fi

platforms="$(jq -c '
  if (.manifests | type) == "array" then
    [.manifests[]
      | select((.platform.os // "unknown") != "unknown")
      | {
          platform: (.platform.os + "/" + .platform.architecture),
          digest,
          variant: (.platform.variant // null)
        }]
      | sort_by(.platform, .variant, .digest)
  else
    []
  end
' <<<"$destination_raw")"

for platform in "${required_platforms[@]}"; do
  jq -e --arg platform "$platform" \
    'any(.[]; .platform == $platform)' <<<"$platforms" >/dev/null || {
      echo "destination is missing required platform: $platform" >&2
      exit 1
    }
done

record="$(
  jq -cn \
    --arg source "$source_ref" \
    --arg destination "$destination_ref" \
    --arg source_digest "$source_digest" \
    --arg destination_digest "$destination_digest" \
    --argjson platforms "$platforms" \
    '{
      source: $source,
      destination: $destination,
      source_digest: $source_digest,
      destination_digest: $destination_digest,
      platforms: $platforms
    }'
)"
printf '%s\n' "$record"
if [[ -n "$evidence_file" ]]; then
  printf '%s\n' "$record" >>"$evidence_file"
fi
