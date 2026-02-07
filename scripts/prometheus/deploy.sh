#!/bin/bash
# Deploy Prometheus observability stack for Agentic Sandbox
#
# This script installs and configures:
# - Prometheus server
# - Alertmanager
# - Grafana
#
# Usage: sudo ./deploy.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log() {
    echo -e "${GREEN}[INFO]${NC} $*"
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $*"
}

error() {
    echo -e "${RED}[ERROR]${NC} $*" >&2
}

check_root() {
    if [[ $EUID -ne 0 ]]; then
        error "This script must be run as root (use sudo)"
        exit 1
    fi
}

install_prometheus() {
    log "Installing Prometheus..."
    apt update
    apt install -y prometheus prometheus-alertmanager

    log "Installing Grafana..."
    wget -q -O - https://packages.grafana.com/gpg.key | apt-key add -
    echo "deb https://packages.grafana.com/oss/deb stable main" > /etc/apt/sources.list.d/grafana.list
    apt update
    apt install -y grafana

    log "Enabling services..."
    systemctl enable prometheus alertmanager grafana-server
}

deploy_config() {
    log "Deploying Prometheus configuration..."

    # Backup existing configs
    if [[ -f /etc/prometheus/prometheus.yml ]]; then
        cp /etc/prometheus/prometheus.yml /etc/prometheus/prometheus.yml.backup.$(date +%Y%m%d-%H%M%S)
    fi

    if [[ -f /etc/alertmanager/alertmanager.yml ]]; then
        cp /etc/alertmanager/alertmanager.yml /etc/alertmanager/alertmanager.yml.backup.$(date +%Y%m%d-%H%M%S)
    fi

    # Copy configurations
    cp "$SCRIPT_DIR/prometheus.yml" /etc/prometheus/prometheus.yml
    mkdir -p /etc/prometheus/rules
    cp "$SCRIPT_DIR/rules/agentic-sandbox.yml" /etc/prometheus/rules/agentic-sandbox.yml
    cp "$SCRIPT_DIR/alertmanager.yml" /etc/alertmanager/alertmanager.yml

    # Set permissions
    chown prometheus:prometheus /etc/prometheus/prometheus.yml
    chown prometheus:prometheus /etc/prometheus/rules/agentic-sandbox.yml
    chown prometheus:prometheus /etc/alertmanager/alertmanager.yml

    log "Configuration deployed"
}

configure_slack() {
    warn "Alertmanager requires a Slack webhook URL for notifications"
    echo ""
    echo "To configure Slack integration:"
    echo "1. Create a Slack webhook at https://api.slack.com/apps"
    echo "2. Edit /etc/alertmanager/alertmanager.yml"
    echo "3. Replace 'YOUR/WEBHOOK/URL' with your actual webhook URL"
    echo "4. Restart alertmanager: sudo systemctl restart alertmanager"
    echo ""
    read -p "Press Enter to continue..."
}

configure_pagerduty() {
    warn "Alertmanager requires a PagerDuty service key for critical alerts"
    echo ""
    echo "To configure PagerDuty integration:"
    echo "1. Create a PagerDuty service integration"
    echo "2. Copy the integration key (service key)"
    echo "3. Edit /etc/alertmanager/alertmanager.yml"
    echo "4. Replace 'YOUR_PAGERDUTY_SERVICE_KEY' with your actual key"
    echo "5. Restart alertmanager: sudo systemctl restart alertmanager"
    echo ""
    read -p "Press Enter to continue..."
}

validate_config() {
    log "Validating Prometheus configuration..."
    promtool check config /etc/prometheus/prometheus.yml || {
        error "Prometheus configuration validation failed"
        exit 1
    }

    log "Validating alert rules..."
    promtool check rules /etc/prometheus/rules/agentic-sandbox.yml || {
        error "Alert rules validation failed"
        exit 1
    }

    log "Configuration validation passed"
}

start_services() {
    log "Starting services..."
    systemctl restart prometheus
    systemctl restart alertmanager
    systemctl restart grafana-server

    sleep 3

    log "Checking service status..."
    systemctl is-active --quiet prometheus && log "Prometheus: RUNNING" || error "Prometheus: FAILED"
    systemctl is-active --quiet alertmanager && log "Alertmanager: RUNNING" || error "Alertmanager: FAILED"
    systemctl is-active --quiet grafana-server && log "Grafana: RUNNING" || error "Grafana: FAILED"
}

verify_targets() {
    log "Verifying Prometheus targets..."
    sleep 5

    curl -s http://localhost:9090/api/v1/targets | jq -r '.data.activeTargets[] | "\(.job): \(.health)"' || {
        warn "Failed to verify targets (is jq installed?)"
    }
}

print_summary() {
    echo ""
    echo "========================================="
    log "Deployment Complete!"
    echo "========================================="
    echo ""
    echo "Access Points:"
    echo "  Prometheus:    http://localhost:9090"
    echo "  Alertmanager:  http://localhost:9093"
    echo "  Grafana:       http://localhost:3000 (admin/admin)"
    echo ""
    echo "Next Steps:"
    echo "  1. Configure Slack webhook in /etc/alertmanager/alertmanager.yml"
    echo "  2. Configure PagerDuty service key in /etc/alertmanager/alertmanager.yml"
    echo "  3. Add Prometheus data source to Grafana (http://localhost:9090)"
    echo "  4. Import pre-built dashboards"
    echo "  5. Verify agent VMs are scraped: http://localhost:9090/targets"
    echo ""
    echo "Documentation: scripts/prometheus/README.md"
    echo "========================================="
}

main() {
    check_root

    log "Starting Agentic Sandbox observability deployment..."

    install_prometheus
    deploy_config
    configure_slack
    configure_pagerduty
    validate_config
    start_services
    verify_targets
    print_summary
}

main "$@"
