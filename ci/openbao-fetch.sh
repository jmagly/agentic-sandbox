#!/usr/bin/env bash
# Fetch CI secrets from OpenBao at job runtime (#635).
#
# CI stores only the AppRole "secret zero" (BAO_CI_ROLE_ID / BAO_CI_SECRET_ID).
# This helper logs in once, reads one or more KV fields, MASKS every value in
# the job log, and hands them to later steps without ever placing a value on
# argv or printing it to stdout:
#   - tokens  -> `NAME=value` appended to $GITHUB_ENV (single-line values only)
#   - keys    -> written to a mode-600 mktemp file; the FILE PATH (never the
#                value) is appended to $GITHUB_ENV as NAME
#
# Directives are read from stdin, one per line (blank lines / `#` comments
# ignored):
#   env      <ENV_NAME>       <kv-data-path>   <field>
#   keyfile  <ENV_NAME>       <kv-data-path>   <field>
#
# Example (in a workflow step):
#   env:
#     BAO_CI_ROLE_ID: ${{ secrets.BAO_CI_ROLE_ID }}
#     BAO_CI_SECRET_ID: ${{ secrets.BAO_CI_SECRET_ID }}
#   run: |
#     ci/openbao-fetch.sh <<'SPEC'
#     env      GHCR_TOKEN      kv_internal/data/ci/shared/ghcr-token          token
#     keyfile  DEPLOY_KEY_FILE kv_internal/data/ci/agentic-sandbox/docsite-deploy private_key
#     SPEC
#
# Clean up any keyfile with an `if: always()` step: `rm -f "$DEPLOY_KEY_FILE"`.
#
# rca-g2 OpenBao is reached by IP (CI job containers lack .s9.internal DNS);
# the listener cert is for the hostname, hence -k / skip-verify. See itops
# docs/security/secret-management-sop.md.
set -euo pipefail

BAO_ADDR="${BAO_ADDR:-https://10.0.42.106:8200}"

die() { echo "::error::openbao-fetch: $*" >&2; exit 1; }

command -v jq   >/dev/null 2>&1 || die "jq is required"
command -v curl >/dev/null 2>&1 || die "curl is required"
[ -n "${GITHUB_ENV:-}" ] || die "GITHUB_ENV is not set (run inside Gitea/GitHub Actions)"

# --- Log in once via AppRole; role-id/secret-id go through a stdin JSON body,
#     never argv. Token stays in a variable, never printed. --------------------
if [ -z "${BAO_CI_ROLE_ID:-}" ] || [ -z "${BAO_CI_SECRET_ID:-}" ]; then
  die "BAO_CI_ROLE_ID / BAO_CI_SECRET_ID are required"
fi

VAULT_TOKEN="$(
  jq -nc --arg r "$BAO_CI_ROLE_ID" --arg s "$BAO_CI_SECRET_ID" '{role_id:$r, secret_id:$s}' \
    | curl -sk --max-time 15 -X POST --data @- "$BAO_ADDR/v1/auth/approle/login" \
    | jq -r '.auth.client_token // empty'
)"
[ -n "$VAULT_TOKEN" ] || die "AppRole login failed"

_revoke() {
  [ -n "${VAULT_TOKEN:-}" ] || return 0
  curl -sk --max-time 10 -H "X-Vault-Token: $VAULT_TOKEN" \
    -X POST "$BAO_ADDR/v1/auth/token/revoke-self" >/dev/null 2>&1 || true
  VAULT_TOKEN=""
}
trap _revoke EXIT

# --- Read one KV field. Value only ever lives in a local; caller decides how
#     to surface it. ------------------------------------------------------------
_read_field() {  # <data-path> <field>  -> value on stdout
  local path="$1" field="$2" resp val
  resp="$(curl -sk --max-time 15 -H "X-Vault-Token: $VAULT_TOKEN" "$BAO_ADDR/v1/$path")"
  [ "$(printf '%s' "$resp" | jq -r 'has("data")')" = "true" ] \
    || die "read $path failed — check the ci-agentic-sandbox AppRole policy scope"
  val="$(printf '%s' "$resp" | jq -r --arg f "$field" '.data.data[$f] // empty')"
  [ -n "$val" ] || die "field '$field' absent at $path"
  printf '%s' "$val"
}

while read -r kind name path field _rest; do
  case "$kind" in
    ''|'#'*) continue ;;
  esac
  if [ -z "$name" ] || [ -z "$path" ] || [ -z "$field" ]; then
    die "malformed directive: '$kind $name $path $field'"
  fi

  value="$(_read_field "$path" "$field")"

  case "$kind" in
    env)
      # Single-line values only (tokens/usernames). Reject embedded newlines
      # so we never emit a malformed / partially-masked GITHUB_ENV entry.
      case "$value" in
        *$'\n'*) die "'$name' from $path has newlines; use 'keyfile', not 'env'" ;;
      esac
      # Register the mask before the value can appear anywhere in the log.
      echo "::add-mask::$value"
      printf '%s=%s\n' "$name" "$value" >> "$GITHUB_ENV"
      echo "fetched $name (env) from $path"
      ;;
    keyfile)
      # Key material goes straight to a mode-600 file and is NEVER printed
      # (multi-line ::add-mask:: is unreliable and would echo the key), so we
      # only surface the file path. Caller removes it in an `if: always()` step.
      f="$(mktemp)"
      chmod 600 "$f"
      printf '%s\n' "$value" > "$f"
      printf '%s=%s\n' "$name" "$f" >> "$GITHUB_ENV"
      echo "fetched $name (mode-600 keyfile $f) from $path"
      ;;
    *)
      die "unknown directive kind '$kind' (expected env|keyfile)"
      ;;
  esac
  value=""
done
