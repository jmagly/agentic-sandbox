#!/usr/bin/env bash
# Structured, redacted provider auth readiness probe.
set -euo pipefail

credential_dir="${AGENTIC_CREDENTIAL_DIR:-}"
timeout_s="${AGENTIC_PROVIDER_READINESS_TIMEOUT:-5}"
if [[ "$timeout_s" != *s ]]; then
  timeout_s="${timeout_s}s"
fi

safe_version() {
  local tool="$1"
  timeout "$timeout_s" "$tool" --version 2>&1 | head -n 1 | tr '\t\r' '  ' || true
}

file_or_dir_default() {
  local explicit_file="$1"
  local dir_name="$2"
  if [[ -n "$explicit_file" ]]; then
    printf '%s\n' "$explicit_file"
    return
  fi
  if [[ -n "$credential_dir" && -f "$credential_dir/$dir_name" ]]; then
    printf '%s\n' "$credential_dir/$dir_name"
  fi
}

emit() {
  local provider="$1"
  local cli="$2"
  local cli_status="$3"
  local version="$4"
  local auth_state="$5"
  local error_class="$6"
  version="${version//$'\t'/ }"
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$provider" "$cli" "$cli_status" "$version" "$auth_state" "$error_class"
}

probe_file_provider() {
  local provider="$1"
  local cli="$2"
  local credential_file="$3"
  local version=""
  local cli_status="present"
  if ! command -v "$cli" >/dev/null 2>&1; then
    cli_status="missing"
  else
    version="$(safe_version "$cli")"
    [[ -n "$version" ]] || version="present-version-empty"
  fi

  if [[ "$cli_status" == "missing" ]]; then
    emit "$provider" "$cli" "$cli_status" "" "unknown" "missing_cli"
  elif [[ -z "$credential_file" ]]; then
    emit "$provider" "$cli" "$cli_status" "$version" "missing" "missing_credential"
  elif [[ ! -f "$credential_file" ]]; then
    emit "$provider" "$cli" "$cli_status" "$version" "missing" "missing_credential_file"
  elif [[ ! -s "$credential_file" ]]; then
    emit "$provider" "$cli" "$cli_status" "$version" "invalid" "empty_credential_file"
  else
    emit "$provider" "$cli" "$cli_status" "$version" "present_unvalidated" "none"
  fi
}

probe_github() {
  local token_file
  token_file="$(file_or_dir_default "${GITHUB_TOKEN_FILE:-${GH_TOKEN_FILE:-}}" "github_token")"
  probe_file_provider "github" "gh" "$token_file"
}

probe_ssh() {
  local key_file
  key_file="$(file_or_dir_default "${SSH_PRIVATE_KEY_FILE:-}" "ssh_private_key")"
  probe_file_provider "ssh" "ssh" "$key_file"
}

if [[ $# -gt 0 ]]; then
  providers=("$@")
else
  providers=(codex claude github ssh)
fi

printf 'schema\tagentic.provider_readiness.v1\n'
printf 'provider\tcli\tcli_status\tversion\tauth_state\terror_class\n'
for provider in "${providers[@]}"; do
  case "$provider" in
    codex)
      key_file="$(file_or_dir_default "${OPENAI_API_KEY_FILE:-}" "openai_api_key")"
      probe_file_provider "codex" "codex" "$key_file"
      ;;
    claude)
      key_file="$(file_or_dir_default "${ANTHROPIC_API_KEY_FILE:-}" "anthropic_api_key")"
      probe_file_provider "claude" "claude" "$key_file"
      ;;
    github)
      probe_github
      ;;
    ssh)
      probe_ssh
      ;;
    *)
      emit "$provider" "" "unknown" "" "unknown" "unsupported_provider"
      ;;
  esac
done
