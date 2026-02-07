#!/usr/bin/env python3
"""
VM Event Bridge - Monitors libvirt VM lifecycle events and forwards to management server.

This script connects to libvirt, subscribes to domain lifecycle events, and POSTs
them to the management server's event API for broadcasting via WebSocket.

Usage:
    ./vm-event-bridge.py [--management-url URL] [--libvirt-uri URI]

Events captured:
    - vm.started    - VM powered on
    - vm.stopped    - VM powered off (graceful)
    - vm.crashed    - VM crashed (kernel panic, etc.)
    - vm.shutdown   - Shutdown initiated
    - vm.rebooted   - VM rebooting
    - vm.suspended  - VM suspended/paused
    - vm.resumed    - VM resumed from suspend
    - vm.defined    - VM created/defined
    - vm.undefined  - VM deleted
"""

import argparse
import json
import logging
import os
import signal
import sys
import time
from datetime import datetime, timezone
from typing import Optional

try:
    import libvirt
except ImportError:
    print("ERROR: libvirt-python not installed. Run: pip install libvirt-python", file=sys.stderr)
    sys.exit(1)

try:
    import requests
except ImportError:
    print("ERROR: requests not installed. Run: pip install requests", file=sys.stderr)
    sys.exit(1)

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s %(levelname)s %(name)s: %(message)s',
    datefmt='%Y-%m-%dT%H:%M:%S%z'
)
logger = logging.getLogger('vm-event-bridge')

# Event type mappings
LIFECYCLE_EVENTS = {
    libvirt.VIR_DOMAIN_EVENT_DEFINED: 'vm.defined',
    libvirt.VIR_DOMAIN_EVENT_UNDEFINED: 'vm.undefined',
    libvirt.VIR_DOMAIN_EVENT_STARTED: 'vm.started',
    libvirt.VIR_DOMAIN_EVENT_SUSPENDED: 'vm.suspended',
    libvirt.VIR_DOMAIN_EVENT_RESUMED: 'vm.resumed',
    libvirt.VIR_DOMAIN_EVENT_STOPPED: 'vm.stopped',
    libvirt.VIR_DOMAIN_EVENT_SHUTDOWN: 'vm.shutdown',
    libvirt.VIR_DOMAIN_EVENT_PMSUSPENDED: 'vm.pmsuspended',
    libvirt.VIR_DOMAIN_EVENT_CRASHED: 'vm.crashed',
}

# Detail mappings for stopped events
STOPPED_DETAILS = {
    libvirt.VIR_DOMAIN_EVENT_STOPPED_SHUTDOWN: 'shutdown',
    libvirt.VIR_DOMAIN_EVENT_STOPPED_DESTROYED: 'destroyed',
    libvirt.VIR_DOMAIN_EVENT_STOPPED_CRASHED: 'crashed',
    libvirt.VIR_DOMAIN_EVENT_STOPPED_MIGRATED: 'migrated',
    libvirt.VIR_DOMAIN_EVENT_STOPPED_SAVED: 'saved',
    libvirt.VIR_DOMAIN_EVENT_STOPPED_FAILED: 'failed',
    libvirt.VIR_DOMAIN_EVENT_STOPPED_FROM_SNAPSHOT: 'from_snapshot',
}

# Detail mappings for started events
STARTED_DETAILS = {
    libvirt.VIR_DOMAIN_EVENT_STARTED_BOOTED: 'booted',
    libvirt.VIR_DOMAIN_EVENT_STARTED_MIGRATED: 'migrated',
    libvirt.VIR_DOMAIN_EVENT_STARTED_RESTORED: 'restored',
    libvirt.VIR_DOMAIN_EVENT_STARTED_FROM_SNAPSHOT: 'from_snapshot',
    libvirt.VIR_DOMAIN_EVENT_STARTED_WAKEUP: 'wakeup',
}


class VMEventBridge:
    """Monitors libvirt events and forwards to management server."""

    def __init__(self, management_url: str, libvirt_uri: str = 'qemu:///system'):
        self.management_url = management_url.rstrip('/')
        self.libvirt_uri = libvirt_uri
        self.conn: Optional[libvirt.virConnect] = None
        self.running = True
        self.event_count = 0
        self.error_count = 0
        self.last_event_time: Optional[datetime] = None

        # VM start times for uptime calculation
        self.vm_start_times: dict[str, datetime] = {}

    def connect(self) -> bool:
        """Connect to libvirt."""
        try:
            # Register default event implementation
            libvirt.virEventRegisterDefaultImpl()

            self.conn = libvirt.open(self.libvirt_uri)
            if self.conn is None:
                logger.error(f"Failed to connect to {self.libvirt_uri}")
                return False

            logger.info(f"Connected to libvirt: {self.libvirt_uri}")
            return True
        except libvirt.libvirtError as e:
            logger.error(f"libvirt connection error: {e}")
            return False

    def register_callbacks(self):
        """Register event callbacks with libvirt."""
        if self.conn is None:
            raise RuntimeError("Not connected to libvirt")

        # Register lifecycle event callback
        self.conn.domainEventRegisterAny(
            None,  # All domains
            libvirt.VIR_DOMAIN_EVENT_ID_LIFECYCLE,
            self._lifecycle_callback,
            None
        )

        # Register reboot event callback
        self.conn.domainEventRegisterAny(
            None,
            libvirt.VIR_DOMAIN_EVENT_ID_REBOOT,
            self._reboot_callback,
            None
        )

        logger.info("Registered libvirt event callbacks")

    def _lifecycle_callback(self, conn, dom, event, detail, opaque):
        """Handle lifecycle events."""
        vm_name = dom.name()
        event_type = LIFECYCLE_EVENTS.get(event, f'vm.unknown_{event}')

        # Get detail string
        detail_str = None
        if event == libvirt.VIR_DOMAIN_EVENT_STOPPED:
            detail_str = STOPPED_DETAILS.get(detail, f'unknown_{detail}')
            # Override event type for crashes
            if detail == libvirt.VIR_DOMAIN_EVENT_STOPPED_CRASHED:
                event_type = 'vm.crashed'
        elif event == libvirt.VIR_DOMAIN_EVENT_STARTED:
            detail_str = STARTED_DETAILS.get(detail, f'unknown_{detail}')
            # Track start time
            self.vm_start_times[vm_name] = datetime.now(timezone.utc)

        # Calculate uptime for stop/crash events
        uptime_seconds = None
        if event in (libvirt.VIR_DOMAIN_EVENT_STOPPED, libvirt.VIR_DOMAIN_EVENT_CRASHED):
            start_time = self.vm_start_times.pop(vm_name, None)
            if start_time:
                uptime_seconds = int((datetime.now(timezone.utc) - start_time).total_seconds())

        self._emit_event(event_type, vm_name, {
            'reason': detail_str,
            'uptime_seconds': uptime_seconds,
        })

    def _reboot_callback(self, conn, dom, opaque):
        """Handle reboot events."""
        vm_name = dom.name()
        self._emit_event('vm.rebooted', vm_name, {'reason': 'reboot'})

    def _emit_event(self, event_type: str, vm_name: str, details: dict):
        """Send event to management server."""
        now = datetime.now(timezone.utc)
        self.last_event_time = now

        # Filter out None values from details
        details = {k: v for k, v in details.items() if v is not None}

        payload = {
            'event_type': event_type,
            'vm_name': vm_name,
            'timestamp': now.isoformat(),
            'details': details,
            'agent_id': vm_name,  # Assume agent_id matches vm_name
        }

        logger.info(f"Event: {event_type} vm={vm_name} details={details}")

        try:
            url = f"{self.management_url}/api/v1/events"
            response = requests.post(
                url,
                json=payload,
                timeout=5,
                headers={'Content-Type': 'application/json'}
            )

            if response.status_code == 200:
                self.event_count += 1
                logger.debug(f"Event delivered: {event_type}")
            else:
                self.error_count += 1
                logger.warning(f"Event delivery failed: {response.status_code} {response.text}")

        except requests.exceptions.RequestException as e:
            self.error_count += 1
            logger.warning(f"Event delivery error: {e}")

    def run(self):
        """Main event loop."""
        if not self.connect():
            return 1

        self.register_callbacks()

        logger.info(f"Listening for VM events, forwarding to {self.management_url}")
        logger.info("Press Ctrl+C to stop")

        # Set up signal handlers
        def signal_handler(sig, frame):
            logger.info("Shutting down...")
            self.running = False

        signal.signal(signal.SIGINT, signal_handler)
        signal.signal(signal.SIGTERM, signal_handler)

        # Event loop
        while self.running:
            try:
                libvirt.virEventRunDefaultImpl()
            except Exception as e:
                logger.error(f"Event loop error: {e}")
                time.sleep(1)

        # Cleanup
        if self.conn:
            self.conn.close()

        logger.info(f"Shutdown complete. Events: {self.event_count}, Errors: {self.error_count}")
        return 0

    def status(self) -> dict:
        """Return current status."""
        return {
            'connected': self.conn is not None,
            'running': self.running,
            'event_count': self.event_count,
            'error_count': self.error_count,
            'last_event_time': self.last_event_time.isoformat() if self.last_event_time else None,
            'tracked_vms': list(self.vm_start_times.keys()),
        }


def main():
    parser = argparse.ArgumentParser(
        description='VM Event Bridge - Forward libvirt events to management server'
    )
    parser.add_argument(
        '--management-url',
        default=os.environ.get('MANAGEMENT_URL', 'http://localhost:8122'),
        help='Management server URL (default: http://localhost:8122)'
    )
    parser.add_argument(
        '--libvirt-uri',
        default=os.environ.get('LIBVIRT_URI', 'qemu:///system'),
        help='libvirt connection URI (default: qemu:///system)'
    )
    parser.add_argument(
        '-v', '--verbose',
        action='store_true',
        help='Enable verbose logging'
    )

    args = parser.parse_args()

    if args.verbose:
        logging.getLogger().setLevel(logging.DEBUG)

    bridge = VMEventBridge(
        management_url=args.management_url,
        libvirt_uri=args.libvirt_uri
    )

    return bridge.run()


if __name__ == '__main__':
    sys.exit(main())
