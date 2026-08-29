# A2A push callback security

Agentic Sandbox accepts A2A push callbacks only as public Internet HTTPS
destinations. It does not provide a general-purpose webhook proxy or access to
private service networks.

## Application policy

The executor applies the following controls when a callback is registered and
again when it is delivered:

- the URL must parse canonically, use `https`, and use port 443;
- URL credentials and fragments are rejected;
- literal and resolved loopback, private, link-local, shared, documentation,
  benchmarking, multicast, unspecified, reserved, and cloud-metadata address
  ranges are rejected for IPv4 and IPv6;
- every DNS answer must be public; a mixed public/private answer set is denied;
- DNS is resolved before every attempt and the selected address is pinned into
  that attempt's HTTP client, while TLS SNI and the `Host` header retain the
  configured hostname;
- redirects are disabled;
- each task may register at most 16 callbacks, no more than four callbacks are
  delivered concurrently, and a failed callback receives at most three bounded
  attempts;
- response bodies are not consumed, and connect/overall request timeouts are
  five and fifteen seconds;
- logs contain task/config identifiers and policy categories, never callback
  URLs, authentication material, or transport errors that can embed a URL.

Task and instance authorization is checked before callback CRUD. A
client-supplied callback config ID cannot replace a config owned by another
task.

## Network egress control

Application validation is one layer. Production executor network namespaces
must also deny traffic to loopback, RFC 1918, link-local, carrier-grade NAT,
multicast, reserved/special-use, cluster/service, control-plane, and cloud
metadata ranges. Permit outbound TCP 443 only through the deployment's approved
Internet egress path. DNS should use a controlled resolver that cannot return
tenant-private views to the executor.

Where an installation needs a fixed partner set, enforce that hostname/IP set
at the egress proxy or firewall in addition to the executor's public-HTTPS
policy. Do not weaken the application address checks to accommodate an internal
callback; use an authenticated public relay with its own inbound policy.

## Verification

The executor unit suite covers malformed and credential-bearing URLs, encoded
IPv4 forms, IPv4-mapped IPv6, private and metadata addresses, mixed DNS answers,
DNS rebinding transitions, redirect refusal, public HTTPS callbacks, callback
count limits, retry bounds, and cross-task config ownership.

After changing deployment egress, verify one approved public callback and prove
that representative private IPv4, IPv6, and metadata destinations are denied.
Do not probe real internal services as part of that check.
