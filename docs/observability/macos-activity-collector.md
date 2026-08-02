# macOS activity collector

The macOS collector normalizes host observations into `activity.event/v1`
metadata. It does not retain file content, paths, command arguments,
environment values, or Unified Logging messages. Endpoint Security supplies
observed process and selected file events; an allowlisted Unified Logging
adapter supplies self-reported service events.

## Coverage and trust

| Source | Events | Trust and privacy |
| --- | --- | --- |
| Endpoint Security | exec, exit, modified close, unlink | Observed host metadata. Paths, audit tokens, argv, and environment are SHA-256 digested; only counts and safe executable basenames remain. |
| Unified Logging | allowlisted subsystem/category records | Self-reported. The message is digested and never retained. |
| Network Extension | none by default | Explicit `telemetry.unsupported` coverage; Endpoint Security is not treated as network evidence. |
| Docker Desktop | host events plus Linux VM telemetry | Labeled `docker-desktop-linux-vm`, never presented as native macOS guest coverage. |

Every event carries tenant, host, instance, agent, process (when available),
collector sequence, runtime, clock, and collector identifiers. Bounded queue
overflow produces `telemetry.loss`; restart and clock uncertainty have their
own integrity events. The durable activity spool preserves monotonic sequence
and resumes after an acknowledgement.

## Build and external Apple gate

Run this on an Apple Silicon host with the full Xcode application installed:

```bash
scripts/build-macos-activity-collector.sh
```

The source type-checks with Swift 6.3.2 on Mutsu. Mutsu currently has only
Command Line Tools, whose SDK lacks the linkable `EndpointSecurity.framework`,
so a linked binary cannot yet be produced there. Production activation also
requires Apple approval for the Endpoint Security entitlement, a matching
Developer ID Application identity, notarization credentials, and explicit user
approval in System Settings. These are external trust-chain gates; the build
must fail closed when any is absent.

Signing uses hardened runtime and exactly the entitlement set in
`native/macos-activity-collector/EndpointSecurity.entitlements.plist`. Verify
the extracted signature entitlements before packaging. The existing macOS
release ceremony must compare the credential-free preview payload, sign the
binary and installer, notarize, staple, and run Gatekeeper checks. A release
must not claim Endpoint Security coverage until the signed binary actually
starts on an approved host.

## Install, upgrade, disable, and uninstall

Installation is deliberately not automatic:

1. Install the verified binary at
   `/usr/local/bin/agentic-macos-activity-collector` with mode `0755` and the
   LaunchDaemon template at
   `/Library/LaunchDaemons/io.aiwg.agentic-sandbox.activity-collector.plist`
   with root ownership and mode `0644`.
2. Obtain operator approval, then use `launchctl bootstrap system` and verify
   the collector emits an observed exec fixture and a health record.
3. For upgrade, `bootout` the old daemon, atomically replace the verified
   binary and plist, then `bootstrap`; the restart event must bridge the gap.
4. To disable, `launchctl bootout system/...` and retain the spool for the
   configured retention interval. Emit or record an intentional coverage gap.
5. To uninstall, boot out the daemon and remove only the two paths above after
   the retention/hold decision. Never delete a spool under legal hold.

The repository template is inert: packaging or copying it does not invoke
`launchctl`, request consent, or weaken System Integrity Protection.

## Unified Logging and network limits

The adapter consumes JSON from an allowlisted stream, for example:

```bash
/usr/bin/log stream --style json \
  --predicate 'subsystem == "io.aiwg.agentic-sandbox"' |
  scripts/macos-unified-log-adapter.py
```

Unknown subsystems/categories are rejected. Private interpolation behavior is
not a sufficient privacy boundary, so raw messages are always discarded after
digesting.

Network Extension capture is not enabled. Encrypted payloads, loopback gaps,
VPN/provider ordering, DNS-over-HTTPS, and flows inside the Docker Desktop VM
must be reported as unsupported unless an approved Network Extension or the
Linux guest collector produces direct evidence. No payload capture is planned.

## Tamper and performance posture

Endpoint Security requires root, Apple entitlement approval, and user consent;
root or kernel compromise can still blind or terminate it. Queue loss,
collector restart, stale heartbeat, missing source classes, and clock
uncertainty are therefore first-class coverage signals. The management plane
must never infer completeness solely from the presence of some events.

On Mutsu (Apple Silicon), the metadata-only Unified Logging adapter normalized
10,000 synthetic records in 0.08 seconds (125,000 records/second), used 11.6
MiB maximum resident memory, and emitted 270 bytes/record. At 100 records/second
the measured user time projects to 0.07% of one core. These results meet #707's
2% CPU, 128 MiB host-memory, and 1 KiB/event M0 budgets. See
`evidence/macos-activity-collector-benchmark-716.json`. This benchmark covers
normalization, not privileged Endpoint Security delivery latency; that
measurement remains gated on the approved, linked binary.
