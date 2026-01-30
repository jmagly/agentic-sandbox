#!/bin/bash
# logging.sh - Shared bash logging library for agentic-sandbox scripts
#
# Provides consistent logging across all bash scripts with:
# - JSON output when LOG_FORMAT=json
# - File output when LOG_FILE is set
# - Timestamps, hostname, script name in all entries
# - Colored console output for interactive use
#
# Usage:
#   source /path/to/scripts/lib/logging.sh
#   log_info "Starting process"
#   log_error "Something failed"
#
# Environment variables:
#   LOG_LEVEL   - trace, debug, info, warn, error (default: info)
#   LOG_FORMAT  - pretty, json, compact (default: pretty)
#   LOG_FILE    - Optional file path for log output

# Determine script name and hostname for log entries
LOG_SCRIPT_NAME="${LOG_SCRIPT_NAME:-$(basename "${BASH_SOURCE[1]:-$0}" .sh)}"
LOG_HOSTNAME="${LOG_HOSTNAME:-$(hostname -s 2>/dev/null || echo 'unknown')}"

# Log level constants (lower = more verbose)
declare -A LOG_LEVELS=(
    [trace]=0
    [debug]=1
    [info]=2
    [warn]=3
    [error]=4
)

# Default settings
: "${LOG_LEVEL:=info}"
: "${LOG_FORMAT:=pretty}"
: "${LOG_FILE:=}"

# Colors for pretty output (only when connected to terminal)
if [[ -t 1 ]]; then
    LOG_COLOR_RESET='\033[0m'
    LOG_COLOR_RED='\033[0;31m'
    LOG_COLOR_GREEN='\033[0;32m'
    LOG_COLOR_YELLOW='\033[1;33m'
    LOG_COLOR_BLUE='\033[0;34m'
    LOG_COLOR_CYAN='\033[0;36m'
    LOG_COLOR_GRAY='\033[0;90m'
else
    LOG_COLOR_RESET=''
    LOG_COLOR_RED=''
    LOG_COLOR_GREEN=''
    LOG_COLOR_YELLOW=''
    LOG_COLOR_BLUE=''
    LOG_COLOR_CYAN=''
    LOG_COLOR_GRAY=''
fi

# Check if a log level should be logged based on current LOG_LEVEL
_log_should_log() {
    local level="$1"
    local current_level="${LOG_LEVELS[${LOG_LEVEL,,}]:-2}"
    local target_level="${LOG_LEVELS[${level,,}]:-2}"
    [[ $target_level -ge $current_level ]]
}

# Get ISO8601 timestamp
_log_timestamp() {
    date -u +"%Y-%m-%dT%H:%M:%S.%3NZ"
}

# Get epoch milliseconds
_log_timestamp_ms() {
    date +%s%3N
}

# Format a log entry as JSON
_log_format_json() {
    local level="$1"
    local message="$2"
    shift 2
    local extra_fields=("$@")

    local timestamp
    timestamp=$(_log_timestamp)

    # Build JSON object
    local json="{"
    json+="\"timestamp\":\"${timestamp}\","
    json+="\"level\":\"${level}\","
    json+="\"message\":\"$(echo "$message" | sed 's/"/\\"/g' | sed 's/\\/\\\\/g')\","
    json+="\"script\":\"${LOG_SCRIPT_NAME}\","
    json+="\"hostname\":\"${LOG_HOSTNAME}\""

    # Add extra fields if provided
    for field in "${extra_fields[@]}"; do
        if [[ "$field" == *"="* ]]; then
            local key="${field%%=*}"
            local value="${field#*=}"
            json+=",\"${key}\":\"$(echo "$value" | sed 's/"/\\"/g')\""
        fi
    done

    json+="}"
    echo "$json"
}

# Format a log entry for pretty output
_log_format_pretty() {
    local level="$1"
    local color="$2"
    local message="$3"

    local timestamp
    timestamp=$(date +"%H:%M:%S")

    echo -e "${LOG_COLOR_GRAY}${timestamp}${LOG_COLOR_RESET} ${color}[${level^^}]${LOG_COLOR_RESET} ${message}"
}

# Format a log entry for compact output
_log_format_compact() {
    local level="$1"
    local message="$2"

    local timestamp
    timestamp=$(date +"%H:%M:%S")

    echo "${timestamp} ${level^^}: ${message}"
}

# Write to file if LOG_FILE is set
_log_write_file() {
    local message="$1"
    if [[ -n "$LOG_FILE" ]]; then
        # Ensure directory exists
        local dir
        dir=$(dirname "$LOG_FILE")
        [[ -d "$dir" ]] || mkdir -p "$dir"

        # Append to file (always JSON format for machine parsing)
        echo "$message" >> "$LOG_FILE"
    fi
}

# Core logging function
_log() {
    local level="$1"
    local color="$2"
    local message="$3"
    shift 3
    local extra_fields=("$@")

    # Check if we should log this level
    _log_should_log "$level" || return 0

    # Format and output based on LOG_FORMAT
    case "${LOG_FORMAT,,}" in
        json)
            local json_entry
            json_entry=$(_log_format_json "$level" "$message" "${extra_fields[@]}")
            echo "$json_entry"
            _log_write_file "$json_entry"
            ;;
        compact)
            _log_format_compact "$level" "$message"
            if [[ -n "$LOG_FILE" ]]; then
                local json_entry
                json_entry=$(_log_format_json "$level" "$message" "${extra_fields[@]}")
                _log_write_file "$json_entry"
            fi
            ;;
        pretty|*)
            _log_format_pretty "$level" "$color" "$message"
            if [[ -n "$LOG_FILE" ]]; then
                local json_entry
                json_entry=$(_log_format_json "$level" "$message" "${extra_fields[@]}")
                _log_write_file "$json_entry"
            fi
            ;;
    esac
}

# Public logging functions

log_trace() {
    _log "trace" "$LOG_COLOR_GRAY" "$1" "${@:2}"
}

log_debug() {
    _log "debug" "$LOG_COLOR_CYAN" "$1" "${@:2}"
}

log_info() {
    _log "info" "$LOG_COLOR_BLUE" "$1" "${@:2}"
}

log_success() {
    _log "info" "$LOG_COLOR_GREEN" "$1" "${@:2}"
}

log_warn() {
    _log "warn" "$LOG_COLOR_YELLOW" "$1" "${@:2}"
}

log_error() {
    _log "error" "$LOG_COLOR_RED" "$1" "${@:2}" >&2
}

# Log with custom fields (for structured logging)
# Usage: log_with_fields info "User logged in" user_id=123 ip=192.168.1.1
log_with_fields() {
    local level="$1"
    local message="$2"
    shift 2
    local fields=("$@")

    case "$level" in
        trace) _log "trace" "$LOG_COLOR_GRAY" "$message" "${fields[@]}" ;;
        debug) _log "debug" "$LOG_COLOR_CYAN" "$message" "${fields[@]}" ;;
        info)  _log "info" "$LOG_COLOR_BLUE" "$message" "${fields[@]}" ;;
        warn)  _log "warn" "$LOG_COLOR_YELLOW" "$message" "${fields[@]}" ;;
        error) _log "error" "$LOG_COLOR_RED" "$message" "${fields[@]}" >&2 ;;
        *)     _log "info" "$LOG_COLOR_BLUE" "$message" "${fields[@]}" ;;
    esac
}

# Log script start with metadata
log_script_start() {
    local description="${1:-}"
    log_with_fields info "Script started${description:+: $description}" \
        "pid=$$" \
        "user=$(whoami)" \
        "cwd=$(pwd)"
}

# Log script end with duration
log_script_end() {
    local start_time="${1:-}"
    local exit_code="${2:-0}"

    if [[ -n "$start_time" ]]; then
        local end_time
        end_time=$(date +%s)
        local duration=$((end_time - start_time))
        log_with_fields info "Script completed" \
            "duration_seconds=$duration" \
            "exit_code=$exit_code"
    else
        log_with_fields info "Script completed" "exit_code=$exit_code"
    fi
}

# Timer functions for tracking operation duration
declare -A LOG_TIMERS=()

log_timer_start() {
    local name="$1"
    LOG_TIMERS[$name]=$(_log_timestamp_ms)
}

log_timer_end() {
    local name="$1"
    local message="${2:-Operation completed}"

    local start_ms="${LOG_TIMERS[$name]:-}"
    if [[ -n "$start_ms" ]]; then
        local end_ms
        end_ms=$(_log_timestamp_ms)
        local duration_ms=$((end_ms - start_ms))
        log_with_fields info "$message" "duration_ms=$duration_ms" "operation=$name"
        unset "LOG_TIMERS[$name]"
    else
        log_warn "Timer '$name' was not started"
    fi
}

# Progress indicator for long operations
log_progress() {
    local current="$1"
    local total="$2"
    local message="${3:-Progress}"

    local percent=$((current * 100 / total))

    case "${LOG_FORMAT,,}" in
        json)
            log_with_fields info "$message" "current=$current" "total=$total" "percent=$percent"
            ;;
        *)
            # Overwrite line for terminal progress
            if [[ -t 1 ]]; then
                printf "\r${LOG_COLOR_BLUE}[INFO]${LOG_COLOR_RESET} %s: %d/%d (%d%%)" "$message" "$current" "$total" "$percent"
                [[ $current -eq $total ]] && echo
            else
                log_info "$message: $current/$total ($percent%)"
            fi
            ;;
    esac
}

# Export functions for subshells
export -f log_trace log_debug log_info log_success log_warn log_error
export -f log_with_fields log_script_start log_script_end
export -f log_timer_start log_timer_end log_progress
export -f _log _log_format_json _log_format_pretty _log_format_compact
export -f _log_should_log _log_timestamp _log_timestamp_ms _log_write_file

# Export variables
export LOG_SCRIPT_NAME LOG_HOSTNAME LOG_LEVEL LOG_FORMAT LOG_FILE
export LOG_COLOR_RESET LOG_COLOR_RED LOG_COLOR_GREEN LOG_COLOR_YELLOW
export LOG_COLOR_BLUE LOG_COLOR_CYAN LOG_COLOR_GRAY
