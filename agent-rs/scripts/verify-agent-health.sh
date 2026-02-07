#!/bin/bash
#
# Health verification script for agent-client systemd service
# Run by ExecStartPost to verify the agent started successfully
#
# Exit codes:
#   0 - Health check passed
#   1 - Health check failed (will trigger restart)

set -euo pipefail

AGENT_ID="${AGENT_ID:-unknown}"
LOG_PREFIX="[health-check]"

# Configuration
MAX_RETRIES=10
RETRY_DELAY=1
HEALTH_CHECK_TIMEOUT=30

log() {
    echo "$LOG_PREFIX $*" >&2
}

check_process() {
    if ! pgrep -f agent-client >/dev/null; then
        log "ERROR: agent-client process not running"
        return 1
    fi
    log "Process check: OK"
    return 0
}

check_systemd_ready() {
    # Check if systemd received READY notification
    if systemctl show agent-client.service --property=ActiveState | grep -q "active"; then
        log "Systemd state check: OK (active)"
        return 0
    else
        log "WARNING: Service not yet active"
        return 1
    fi
}

check_restart_marker() {
    # Check restart marker to track restart frequency
    local marker="/tmp/agent-client-restart.marker"
    if [ -f "$marker" ]; then
        local restart_count
        restart_count=$(cat "$marker" 2>/dev/null || echo "0")
        log "Restart count: $restart_count"

        # Warn if too many restarts
        if [ "$restart_count" -gt 3 ]; then
            log "WARNING: High restart count detected ($restart_count)"
        fi
    fi
}

check_memory_pressure() {
    # Check for memory pressure that could cause OOM
    local mem_available
    mem_available=$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)
    local mem_total
    mem_total=$(awk '/MemTotal:/ {print $2}' /proc/meminfo)

    if [ "$mem_total" -gt 0 ]; then
        local mem_percent=$((100 * mem_available / mem_total))
        if [ "$mem_percent" -lt 10 ]; then
            log "WARNING: Low memory available (${mem_percent}%)"
        else
            log "Memory check: OK (${mem_percent}% available)"
        fi
    fi
}

check_disk_space() {
    # Check for disk space issues
    local disk_avail
    disk_avail=$(df / | awk 'NR==2 {print $4}')
    local disk_total
    disk_total=$(df / | awk 'NR==2 {print $2}')

    if [ "$disk_total" -gt 0 ]; then
        local disk_percent=$((100 * disk_avail / disk_total))
        if [ "$disk_percent" -lt 10 ]; then
            log "WARNING: Low disk space (${disk_percent}% available)"
        else
            log "Disk space check: OK (${disk_percent}% available)"
        fi
    fi
}

# Main health check with retries
main() {
    log "Starting health verification for agent $AGENT_ID"

    # Give the service a moment to fully start
    sleep 2

    local attempt=1
    while [ $attempt -le $MAX_RETRIES ]; do
        log "Health check attempt $attempt/$MAX_RETRIES"

        # Check if process is running
        if check_process; then
            # Check systemd state
            if check_systemd_ready; then
                log "All health checks passed"

                # Run optional diagnostic checks (warnings only)
                check_restart_marker
                check_memory_pressure
                check_disk_space

                log "Health verification PASSED"
                exit 0
            fi
        fi

        # Wait before retry
        if [ $attempt -lt $MAX_RETRIES ]; then
            log "Retrying in ${RETRY_DELAY}s..."
            sleep $RETRY_DELAY
        fi

        attempt=$((attempt + 1))
    done

    log "Health verification FAILED after $MAX_RETRIES attempts"
    exit 1
}

# Run with timeout
timeout $HEALTH_CHECK_TIMEOUT bash -c "$(declare -f main check_process check_systemd_ready check_restart_marker check_memory_pressure check_disk_space log); main" || {
    log "Health check timed out after ${HEALTH_CHECK_TIMEOUT}s"
    exit 1
}
