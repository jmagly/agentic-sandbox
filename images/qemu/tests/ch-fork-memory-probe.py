#!/usr/bin/env python3
"""Live inherited-memory isolation probe for Cloud Hypervisor fork children.

The guest-side server is started before a base snapshot. Restored children
therefore inherit the same process, lineage identifier, PID, and buffer bytes.
The host-side verifier mutates each inherited buffer independently and combines
that result with guest-RAM-only smaps metrics emitted by the CH backend.
"""

import argparse
import hashlib
import json
import os
import re
import shlex
import signal
import select
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence


DEFAULT_REMOTE_PATH = "/tmp/ch-fork-memory-probe.py"
DEFAULT_SOCKET_PATH = "/tmp/ch-fork-memory-probe.sock"
DEFAULT_VM_STORAGE = "/var/lib/agentic-sandbox/vms"
DEFAULT_VSOCK_PORT = 16530
PROTOCOL_VERSION = 1
SAFE_VM_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")


def emit(payload: Dict[str, Any]) -> None:
    print(json.dumps(payload, sort_keys=True, separators=(",", ":")))


def patterned_buffer(lineage_id: str, size_bytes: int) -> bytearray:
    seed = hashlib.sha256(("agentic-sandbox:" + lineage_id).encode("utf-8")).digest()
    block = seed * (1024 * 1024 // len(seed))
    data = bytearray()
    remaining = size_bytes
    while remaining:
        chunk = block[: min(len(block), remaining)]
        data.extend(chunk)
        remaining -= len(chunk)
    return data


class MemoryProbeServer:
    def __init__(
        self,
        socket_path: str,
        lineage_id: str,
        size_bytes: int,
        vsock_port: Optional[int] = None,
    ) -> None:
        self.socket_path = socket_path
        self.lineage_id = lineage_id
        self.data = patterned_buffer(lineage_id, size_bytes)
        self.baseline_sha256 = hashlib.sha256(self.data).hexdigest()
        self.mutation_count = 0
        self.running = True
        self.vsock_port = vsock_port
        self.listeners: List[socket.socket] = []

    def status(self) -> Dict[str, Any]:
        return {
            "schema_version": PROTOCOL_VERSION,
            "lineage_id": self.lineage_id,
            "pid": os.getpid(),
            "size_bytes": len(self.data),
            "baseline_sha256": self.baseline_sha256,
            "current_sha256": hashlib.sha256(self.data).hexdigest(),
            "mutation_count": self.mutation_count,
        }

    def handle(self, request: Dict[str, Any]) -> Dict[str, Any]:
        action = request.get("action")
        if action == "status":
            return self.status()
        if action == "mutate":
            offset = int(request.get("offset", -1))
            length = int(request.get("length", 0))
            xor_byte = int(request.get("xor", 0))
            if offset < 0 or length <= 0 or offset + length > len(self.data):
                raise ValueError("mutation range is outside the probe buffer")
            if not 1 <= xor_byte <= 255:
                raise ValueError("xor must be in the range 1..255")
            for index in range(offset, offset + length):
                self.data[index] ^= xor_byte
            self.mutation_count += 1
            return self.status()
        if action == "stop":
            self.running = False
            return self.status()
        raise ValueError("unknown probe action")

    def serve(self) -> None:
        socket_path = Path(self.socket_path)
        socket_path.parent.mkdir(parents=True, exist_ok=True)
        try:
            socket_path.unlink()
        except FileNotFoundError:
            pass
        old_umask = os.umask(0o077)
        unix_listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            unix_listener.bind(self.socket_path)
            os.chmod(self.socket_path, 0o600)
            unix_listener.listen(8)
            self.listeners.append(unix_listener)
            if self.vsock_port is not None:
                if not hasattr(socket, "AF_VSOCK"):
                    raise RuntimeError("Python does not expose AF_VSOCK")
                vsock_listener = socket.socket(socket.AF_VSOCK, socket.SOCK_STREAM)
                vsock_listener.bind((socket.VMADDR_CID_ANY, self.vsock_port))
                vsock_listener.listen(8)
                self.listeners.append(vsock_listener)
            os.umask(old_umask)
            while self.running:
                readable, _, _ = select.select(self.listeners, [], [], 0.5)
                if not readable:
                    continue
                for listener in readable:
                    connection, _ = listener.accept()
                    with connection:
                        raw = connection.makefile("rb").readline(1024 * 1024)
                        if not raw:
                            # A host can complete the hybrid-vsock CONNECT
                            # handshake and disconnect before sending a probe
                            # request.  Keep serving subsequent connections.
                            continue
                        try:
                            request = json.loads(raw.decode("utf-8"))
                            response = self.handle(request)
                            response["ok"] = True
                        except (ValueError, TypeError, json.JSONDecodeError) as exc:
                            response = {"ok": False, "error": str(exc)}
                        try:
                            connection.sendall(
                                (json.dumps(response, sort_keys=True) + "\n").encode("utf-8")
                            )
                        except BrokenPipeError:
                            continue
        finally:
            os.umask(old_umask)
            for listener in self.listeners:
                listener.close()
            try:
                socket_path.unlink()
            except FileNotFoundError:
                pass


def unix_request(socket_path: str, request: Dict[str, Any]) -> Dict[str, Any]:
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(5.0)
    try:
        client.connect(socket_path)
        client.sendall((json.dumps(request, sort_keys=True) + "\n").encode("utf-8"))
        response = client.makefile("rb").readline(1024 * 1024)
    finally:
        client.close()
    payload = json.loads(response.decode("utf-8"))
    if not payload.get("ok"):
        raise RuntimeError(payload.get("error", "probe request failed"))
    return payload


def hybrid_vsock_request(
    socket_path: str, port: int, request: Dict[str, Any]
) -> Dict[str, Any]:
    """Connect through Cloud Hypervisor's host-side hybrid-vsock socket."""
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(5.0)
    try:
        client.connect(socket_path)
        stream = client.makefile("rwb", buffering=0)
        stream.write("CONNECT {}\n".format(port).encode("ascii"))
        acknowledgement = stream.readline(128)
        if not acknowledgement.startswith(b"OK "):
            raise RuntimeError(
                "hybrid-vsock CONNECT failed: {}".format(
                    acknowledgement.decode("utf-8", errors="replace").strip()
                )
            )
        stream.write((json.dumps(request, sort_keys=True) + "\n").encode("utf-8"))
        response = stream.readline(1024 * 1024)
    finally:
        client.close()
    if not response:
        raise RuntimeError("hybrid-vsock probe returned no response")
    payload = json.loads(response.decode("utf-8"))
    if not payload.get("ok"):
        raise RuntimeError(payload.get("error", "probe request failed"))
    return payload


def ssh_base(user: str, key: str, timeout: int) -> List[str]:
    return [
        "ssh",
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "ConnectTimeout={}".format(timeout),
        "-i",
        key,
        "{}@{{host}}".format(user),
    ]


def ssh_json(
    host: str,
    user: str,
    key: str,
    timeout: int,
    remote_argv: Sequence[str],
) -> Dict[str, Any]:
    command = [item.format(host=host) for item in ssh_base(user, key, timeout)]
    command.append(shlex.join(list(remote_argv)))
    completed = subprocess.run(
        command,
        check=True,
        capture_output=True,
        text=True,
        timeout=max(timeout + 5, 10),
    )
    lines = [line for line in completed.stdout.splitlines() if line.strip()]
    if not lines:
        raise RuntimeError("remote probe returned no JSON")
    return json.loads(lines[-1])


def child_vsock_socket(vm_storage: str, child: str) -> str:
    state = Path(vm_storage) / child / "cloud-hypervisor" / "vm.env"
    with state.open("r", encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("VSOCK_SOCKET="):
                socket_path = line.split("=", 1)[1].strip()
                if socket_path:
                    return socket_path
    raise ValueError("VSOCK_SOCKET missing for {}".format(child))


def child_request(
    args: argparse.Namespace,
    child: str,
    host: Optional[str],
    request: Dict[str, Any],
) -> Dict[str, Any]:
    if args.transport == "vsock":
        if args.pulse_paused:
            pulse_child(args.vm_storage, child, "resume")
        try:
            return hybrid_vsock_request(
                child_vsock_socket(args.vm_storage, child), args.vsock_port, request
            )
        finally:
            if args.pulse_paused:
                pulse_child(args.vm_storage, child, "pause")
    if not host:
        raise ValueError("SSH transport requires a guest address")
    remote = ["python3", args.remote_path]
    if request["action"] == "status":
        remote.extend(["status", "--socket", args.socket])
    elif request["action"] == "mutate":
        remote.extend(
            [
                "mutate",
                "--socket",
                args.socket,
                "--offset",
                str(request["offset"]),
                "--length",
                str(request["length"]),
                "--xor",
                str(request["xor"]),
            ]
        )
    else:
        raise ValueError("unsupported host-side action")
    return ssh_json(host, args.user, args.key, args.timeout, remote)


def detect_ch_remote() -> str:
    candidates = [
        os.environ.get("AGENTIC_CH_REMOTE_BIN", ""),
        "/opt/agentic-sandbox/cloud-hypervisor/current/bin/ch-remote",
        "ch-remote",
    ]
    for candidate in candidates:
        if not candidate:
            continue
        if candidate == "ch-remote" or Path(candidate).is_file():
            return candidate
    raise RuntimeError("ch-remote is unavailable")


def pulse_child(vm_storage: str, child: str, action: str) -> None:
    api_socket = Path(vm_storage) / child / "cloud-hypervisor" / "api.sock"
    subprocess.run(
        [detect_ch_remote(), "--api-socket", str(api_socket), action],
        check=True,
        capture_output=True,
        text=True,
        timeout=5,
    )


def load_metrics(vm_storage: str, child: str) -> Dict[str, Any]:
    path = Path(vm_storage) / child / "cloud-hypervisor" / "restore-metrics.json"
    with path.open("r", encoding="utf-8") as handle:
        metrics = json.load(handle)
    guest_ram = metrics.get("guest_ram", {})
    backing = metrics.get("snapshot_backing", {})
    if metrics.get("memory_restore_mode") != "ondemand":
        raise ValueError("{} was not restored with ondemand memory".format(child))
    if not guest_ram.get("available"):
        raise ValueError("{} has no guest-RAM-scoped smaps evidence".format(child))
    if not backing.get("available"):
        raise ValueError("{} has no snapshot-backing identity evidence".format(child))
    return metrics


def build_evidence(
    child_a: str,
    child_b: str,
    statuses: Dict[str, Dict[str, Any]],
    metrics_a: Dict[str, Any],
    metrics_b: Dict[str, Any],
    cloud_hypervisor_version: str,
) -> Dict[str, Any]:
    a0 = statuses["a_before"]
    b0 = statuses["b_before"]
    a1 = statuses["a_after_a_mutation"]
    b1 = statuses["b_after_a_mutation"]
    a2 = statuses["a_after_b_mutation"]
    b2 = statuses["b_after_b_mutation"]

    same_inherited_lineage = (
        a0["lineage_id"] == b0["lineage_id"]
        and a0["pid"] == b0["pid"]
        and a0["size_bytes"] == b0["size_bytes"]
        and a0["baseline_sha256"] == b0["baseline_sha256"]
        and a0["current_sha256"] == b0["current_sha256"]
        and a0["mutation_count"] == 0
        and b0["mutation_count"] == 0
    )
    child_a_mutation_isolated = (
        a1["current_sha256"] != a0["current_sha256"]
        and a1["mutation_count"] == 1
        and b1["current_sha256"] == b0["current_sha256"]
        and b1["mutation_count"] == 0
    )
    child_b_mutation_isolated = (
        b2["current_sha256"] != b1["current_sha256"]
        and b2["mutation_count"] == 1
        and a2["current_sha256"] == a1["current_sha256"]
        and a2["mutation_count"] == 1
    )

    guest_a = metrics_a["guest_ram"]
    guest_b = metrics_b["guest_ram"]
    backing_a = metrics_a["snapshot_backing"]
    backing_b = metrics_b["snapshot_backing"]
    backing_shared = (
        backing_a["device_id"] == backing_b["device_id"]
        and backing_a["inode"] == backing_b["inode"]
    )
    ram_inodes_a = set(guest_a.get("inodes", []))
    ram_inodes_b = set(guest_b.get("inodes", []))
    ram_mapping_inodes_distinct = bool(ram_inodes_a and ram_inodes_b) and not (
        ram_inodes_a & ram_inodes_b
    )
    total_rss = int(guest_a["rss_kb"]) + int(guest_b["rss_kb"])
    total_pss = int(guest_a["pss_kb"]) + int(guest_b["pss_kb"])
    pss_mapping_savings_kb = max(total_rss - total_pss, 0)
    ksm_shared_kb = int(guest_a.get("ksm_kb", 0)) + int(guest_b.get("ksm_kb", 0))
    ram_inode_overlap = bool(ram_inodes_a & ram_inodes_b)
    if ksm_shared_kb > 0:
        shared_guest_ram_kb = ksm_shared_kb
        cross_child_sharing_basis = "kernel-same-page-merging"
    elif ram_inode_overlap:
        shared_guest_ram_kb = pss_mapping_savings_kb
        cross_child_sharing_basis = "shared-guest-ram-backing-inode"
    else:
        shared_guest_ram_kb = 0
        cross_child_sharing_basis = "none"
    positive_resident_sharing = shared_guest_ram_kb > 0

    result = "pass" if all(
        [
            same_inherited_lineage,
            child_a_mutation_isolated,
            child_b_mutation_isolated,
            backing_shared,
            bool(ram_inodes_a),
            bool(ram_inodes_b),
        ]
    ) else "fail"

    return {
        "schema_version": 1,
        "evidence_type": "cloud-hypervisor-fork-inherited-memory-isolation",
        "checked_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "result": result,
        "cloud_hypervisor_version": cloud_hypervisor_version,
        "children": [child_a, child_b],
        "inherited_probe": {
            "lineage_id": a0["lineage_id"],
            "pid_in_both_children": a0["pid"],
            "size_bytes": a0["size_bytes"],
            "baseline_sha256": a0["baseline_sha256"],
            "same_inherited_lineage": same_inherited_lineage,
            "child_a_mutation_isolated": child_a_mutation_isolated,
            "child_b_mutation_isolated": child_b_mutation_isolated,
            "cross_child_ram_corruption_observed": not (
                child_a_mutation_isolated and child_b_mutation_isolated
            ),
            "states": statuses,
        },
        "memory_model": {
            "restore_mode": "ondemand",
            "upstream_mechanism": "userfaultfd UFFDIO_COPY into each child guest-memory mapping",
            "upstream_reference": "https://github.com/cloud-hypervisor/cloud-hypervisor/pull/7800",
            "snapshot_backing_shared": backing_shared,
            "snapshot_backing_identity": "{}:{}".format(
                backing_a["device_id"], backing_a["inode"]
            ),
            "guest_ram_mapping_inodes_distinct": ram_mapping_inodes_distinct,
            "measurement": "/proc/<vmm-pid>/smaps entries named /memfd:ch_ram",
            "total_guest_ram_rss_kb": total_rss,
            "total_guest_ram_pss_kb": total_pss,
            "pss_mapping_savings_kb": pss_mapping_savings_kb,
            "ksm_shared_kb": ksm_shared_kb,
            "defensible_shared_guest_ram_kb": shared_guest_ram_kb,
            "positive_resident_guest_ram_sharing_proven": positive_resident_sharing,
            "cross_child_sharing_basis": cross_child_sharing_basis,
            "ram_sharing_verdict": "observed" if positive_resident_sharing else "not-observed",
            "claim": (
                "resident-guest-ram-sharing-observed"
                if positive_resident_sharing
                else "snapshot-backing-shared-only"
            ),
            "children": {
                child_a: guest_a,
                child_b: guest_b,
            },
        },
    }


def detect_ch_version() -> str:
    candidates = [
        os.environ.get("AGENTIC_CH_BIN", ""),
        "/opt/agentic-sandbox/cloud-hypervisor/current/bin/cloud-hypervisor",
        "cloud-hypervisor",
    ]
    for candidate in candidates:
        if not candidate:
            continue
        try:
            completed = subprocess.run(
                [candidate, "--version"],
                check=True,
                capture_output=True,
                text=True,
                timeout=5,
            )
            return completed.stdout.strip().splitlines()[0]
        except (FileNotFoundError, subprocess.SubprocessError, IndexError):
            continue
    return "unavailable"


def command_prepare(args: argparse.Namespace) -> None:
    source = Path(__file__).resolve()
    subprocess.run(
        [
            "scp",
            "-q",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "ConnectTimeout={}".format(args.timeout),
            "-i",
            args.key,
            str(source),
            "{}@{}:{}".format(args.user, args.host, args.remote_path),
        ],
        check=True,
        timeout=max(args.timeout + 10, 15),
    )
    lineage_id = args.lineage_id or os.urandom(16).hex()
    remote_command = (
        "chmod 700 {probe}; "
        "if python3 {probe} stop --socket {socket} >/dev/null 2>&1; then "
        "for i in $(seq 1 50); do test ! -S {socket} && break; sleep 0.02; done; fi; "
        "nohup python3 {probe} serve --socket {socket} --vsock-port {vsock_port} --lineage-id {lineage} --size-mib {size} "
        ">{log} 2>&1 </dev/null & "
        "for i in $(seq 1 100); do "
        "python3 {probe} status --socket {socket} 2>/dev/null && exit 0; "
        "sleep 0.05; done; exit 1"
    ).format(
        probe=shlex.quote(args.remote_path),
        socket=shlex.quote(args.socket),
        vsock_port=args.vsock_port,
        lineage=shlex.quote(lineage_id),
        size=args.size_mib,
        log=shlex.quote(args.log_path),
    )
    command = [item.format(host=args.host) for item in ssh_base(args.user, args.key, args.timeout)]
    command.append(remote_command)
    completed = subprocess.run(
        command,
        check=True,
        capture_output=True,
        text=True,
        timeout=max(args.timeout + 15, 20),
    )
    lines = [line for line in completed.stdout.splitlines() if line.strip()]
    if not lines:
        raise RuntimeError("guest probe did not become ready")
    status = json.loads(lines[-1])
    status["prepared_for_snapshot"] = True
    emit(status)


def command_verify(args: argparse.Namespace) -> None:
    if args.child_a == args.child_b:
        raise ValueError("verification requires two distinct children")
    if not SAFE_VM_NAME.fullmatch(args.child_a) or not SAFE_VM_NAME.fullmatch(args.child_b):
        raise ValueError("child names must be safe VM identifiers")
    if args.transport == "ssh":
        if not args.host_a or not args.host_b or args.host_a == args.host_b:
            raise ValueError("SSH verification requires two distinct guest addresses")
        if not args.key or not Path(args.key).is_file():
            raise ValueError("SSH key is unreadable: {}".format(args.key))

    statuses: Dict[str, Dict[str, Any]] = {}
    statuses["a_before"] = child_request(args, args.child_a, args.host_a, {"action": "status"})
    statuses["b_before"] = child_request(args, args.child_b, args.host_b, {"action": "status"})
    size_bytes = int(statuses["a_before"]["size_bytes"])
    mutation_length = min(args.mutation_bytes, size_bytes // 4)
    if mutation_length <= 0:
        raise ValueError("probe buffer is too small for mutation")
    statuses["a_after_a_mutation"] = child_request(
        args,
        args.child_a,
        args.host_a,
        {"action": "mutate", "offset": 0, "length": mutation_length, "xor": 0xA5},
    )
    statuses["b_after_a_mutation"] = child_request(
        args, args.child_b, args.host_b, {"action": "status"}
    )
    statuses["b_after_b_mutation"] = child_request(
        args,
        args.child_b,
        args.host_b,
        {
            "action": "mutate",
            "offset": size_bytes - mutation_length,
            "length": mutation_length,
            "xor": 0x5A,
        },
    )
    statuses["a_after_b_mutation"] = child_request(
        args, args.child_a, args.host_a, {"action": "status"}
    )

    if args.fork_prefix:
        faststart = Path(__file__).resolve().parents[1] / "ch-faststart.sh"
        subprocess.run(
            [
                str(faststart),
                "fork-evidence",
                "--prefix",
                args.fork_prefix,
                "--count",
                "2",
            ],
            check=True,
            capture_output=True,
            text=True,
            timeout=30,
        )

    metrics_a = load_metrics(args.vm_storage, args.child_a)
    metrics_b = load_metrics(args.vm_storage, args.child_b)
    evidence = build_evidence(
        args.child_a,
        args.child_b,
        statuses,
        metrics_a,
        metrics_b,
        detect_ch_version(),
    )
    evidence_path = Path(args.evidence)
    evidence_path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        mode="w", encoding="utf-8", dir=str(evidence_path.parent), delete=False
    ) as handle:
        json.dump(evidence, handle, indent=2, sort_keys=True)
        handle.write("\n")
        temporary_path = Path(handle.name)
    os.chmod(temporary_path, 0o644)
    temporary_path.replace(evidence_path)
    emit(
        {
            "result": evidence["result"],
            "evidence": str(evidence_path),
            "ram_sharing_verdict": evidence["memory_model"]["ram_sharing_verdict"],
            "defensible_shared_guest_ram_kb": evidence["memory_model"][
                "defensible_shared_guest_ram_kb"
            ],
        }
    )
    if evidence["result"] != "pass":
        raise RuntimeError("cross-child inherited-memory isolation verification failed")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    serve = subparsers.add_parser("serve", help="run the pre-snapshot guest memory probe")
    serve.add_argument("--socket", default=DEFAULT_SOCKET_PATH)
    serve.add_argument("--lineage-id", required=True)
    serve.add_argument("--size-mib", type=int, default=64)
    serve.add_argument("--vsock-port", type=int, default=DEFAULT_VSOCK_PORT)
    serve.add_argument("--no-vsock", action="store_true")

    status = subparsers.add_parser("status", help="read guest probe status")
    status.add_argument("--socket", default=DEFAULT_SOCKET_PATH)

    mutate = subparsers.add_parser("mutate", help="mutate guest probe memory")
    mutate.add_argument("--socket", default=DEFAULT_SOCKET_PATH)
    mutate.add_argument("--offset", type=int, required=True)
    mutate.add_argument("--length", type=int, required=True)
    mutate.add_argument("--xor", type=int, required=True)

    stop = subparsers.add_parser("stop", help="stop a running guest probe")
    stop.add_argument("--socket", default=DEFAULT_SOCKET_PATH)

    prepare = subparsers.add_parser("prepare", help="install/start the probe in a base VM")
    prepare.add_argument("--host", required=True)
    prepare.add_argument("--user", default="agent")
    prepare.add_argument("--key", required=True)
    prepare.add_argument("--timeout", type=int, default=10)
    prepare.add_argument("--remote-path", default=DEFAULT_REMOTE_PATH)
    prepare.add_argument("--socket", default=DEFAULT_SOCKET_PATH)
    prepare.add_argument("--log-path", default="/tmp/ch-fork-memory-probe.log")
    prepare.add_argument("--lineage-id")
    prepare.add_argument("--size-mib", type=int, default=64)
    prepare.add_argument("--vsock-port", type=int, default=DEFAULT_VSOCK_PORT)

    verify = subparsers.add_parser("verify", help="prove two restored children are isolated")
    verify.add_argument("--child-a", required=True)
    verify.add_argument("--host-a")
    verify.add_argument("--child-b", required=True)
    verify.add_argument("--host-b")
    verify.add_argument("--transport", choices=("vsock", "ssh"), default="vsock")
    verify.add_argument("--user", default="agent")
    verify.add_argument("--key")
    verify.add_argument("--timeout", type=int, default=10)
    verify.add_argument("--remote-path", default=DEFAULT_REMOTE_PATH)
    verify.add_argument("--socket", default=DEFAULT_SOCKET_PATH)
    verify.add_argument("--vm-storage", default=DEFAULT_VM_STORAGE)
    verify.add_argument("--vsock-port", type=int, default=DEFAULT_VSOCK_PORT)
    verify.add_argument(
        "--pulse-paused",
        action="store_true",
        help="resume each paused child only for one vsock request, then pause it",
    )
    verify.add_argument("--mutation-bytes", type=int, default=4096)
    verify.add_argument("--fork-prefix", help="resample this two-child fork after mutation")
    verify.add_argument("--evidence", required=True)
    return parser


def main(argv: Optional[Sequence[str]] = None) -> int:
    args = build_parser().parse_args(argv)
    if args.command == "serve":
        if args.size_mib <= 0:
            raise ValueError("--size-mib must be positive")
        server = MemoryProbeServer(
            args.socket,
            args.lineage_id,
            args.size_mib * 1024 * 1024,
            None if args.no_vsock else args.vsock_port,
        )

        def stop(_signum: int, _frame: Any) -> None:
            server.running = False

        signal.signal(signal.SIGTERM, stop)
        signal.signal(signal.SIGINT, stop)
        server.serve()
        return 0
    if args.command == "status":
        emit(unix_request(args.socket, {"action": "status"}))
        return 0
    if args.command == "mutate":
        emit(
            unix_request(
                args.socket,
                {
                    "action": "mutate",
                    "offset": args.offset,
                    "length": args.length,
                    "xor": args.xor,
                },
            )
        )
        return 0
    if args.command == "stop":
        emit(unix_request(args.socket, {"action": "stop"}))
        return 0
    if args.command == "prepare":
        command_prepare(args)
        return 0
    if args.command == "verify":
        command_verify(args)
        return 0
    raise AssertionError("unhandled command")


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as exc:
        print("ERROR: {}".format(exc), file=sys.stderr)
        raise SystemExit(1)
