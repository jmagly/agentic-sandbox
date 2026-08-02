# Network activity metadata

The Linux activity collector emits metadata-only `network.dns`, `network.flow`,
and `network.policy` events. Enforced resolver and egress-gateway decisions are
preferred evidence; conntrack, nftables, or eBPF observations supplement them.
Packet capture, TLS interception, HTTP headers, cookies, authorization values,
credentials, and application plaintext are not accepted.

Every event inherits the authenticated tenant, host, instance, agent, collector,
runtime, and source layer from `CollectorScope`. Reliable process identity is
propagated; cgroup identity is digested. DNS questions are always digested because
labels can contain secrets. Flow address fields accept only literal IPv4/IPv6
values, preventing arbitrary content from entering the five-tuple.

| Runtime path | Primary evidence | Attribution | Limitation |
|---|---|---|---|
| QEMU/libvirt tap | enforced gateway + tap flow | VM/tap; process only with guest correlation | host NAT may hide original destination |
| Cloud Hypervisor tap | enforced gateway + tap flow | VM/tap; process only with guest correlation | tap observation cannot see encrypted content |
| Docker bridge/veth | gateway/nftables + cgroup/veth flow | container/cgroup and process where race-free | NAT and short-lived process races |
| Native Linux host | gateway/nftables/eBPF | supervisor cgroup/process | tunnels can bypass per-flow interpretation |
| macOS host/container | enforced gateway policy only in this milestone | instance/gateway | no Linux eBPF/cgroup attribution; #716 evaluates native sources |

Encrypted DNS (DoH/DoT), TLS, QUIC, VPN/tunnels, proxies, NAT, and multiplexed
connections prevent application-level interpretation. The collector reports an
unsupported observation when a caller requests an encrypted DNS transport or an
unrecognized protocol/result. It never claims a hostname-to-flow relationship
that the source did not enforce.

The shared bounded queue emits `telemetry.loss` after backpressure and the durable
spool records monotonic delivery. Gateway allow/deny records include gateway and
rule identity; denied decisions use security retention. Cross-tenant tests use
independent collector scopes and assert that tenant, instance, and process fields
cannot cross.

Run `cargo run --release --example network_activity_benchmark -- 100000` for
repeatable normalization throughput and storage-size evidence. As with #711,
system CPU/RSS, live kernel-source drops, ingest latency, outage recovery, and the
seven-day volume profile are #715 environment measurements rather than inferred
from a user-space microbenchmark.

The checked-in result is
`docs/observability/evidence/network-activity-collector-benchmark-712.json`.
