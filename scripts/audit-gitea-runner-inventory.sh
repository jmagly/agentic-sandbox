#!/usr/bin/env bash
# Print the non-secret repository Actions runner inventory from Gitea.
# This script deliberately has no mode that retrieves registration tokens.

set -euo pipefail

usage() {
  echo "Usage: $0 --login <tea-login> --owner <owner> --repo <repository>" >&2
}

login=""
owner=""
repo=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --login)
      login="${2:-}"
      shift 2
      ;;
    --owner)
      owner="${2:-}"
      shift 2
      ;;
    --repo)
      repo="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

if [ -z "$login" ] || [ -z "$owner" ] || [ -z "$repo" ]; then
  usage
  exit 2
fi

for value in "$owner" "$repo"; do
  if [[ ! "$value" =~ ^[[:alnum:]_.-]+$ ]]; then
    echo "owner and repository must use only letters, digits, '.', '_', or '-'" >&2
    exit 2
  fi
done

command -v tea >/dev/null
command -v jq >/dev/null

tea api --login "$login" \
  "repos/${owner}/${repo}/actions/runners?limit=100" |
  jq '{
    total_count,
    runners: [
      .runners[] | {
        id,
        name,
        status,
        busy,
        disabled,
        ephemeral,
        labels: [.labels[].name]
      }
    ]
  }'
