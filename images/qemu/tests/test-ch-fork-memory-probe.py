#!/usr/bin/env python3
import importlib.util
import json
import socket
import subprocess
import tempfile
import threading
import time
import unittest
from argparse import Namespace
from pathlib import Path


SCRIPT = Path(__file__).with_name("ch-fork-memory-probe.py")
SPEC = importlib.util.spec_from_file_location("ch_fork_memory_probe", SCRIPT)
assert SPEC and SPEC.loader
PROBE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROBE)


class MemoryProbeTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.processes = []

    def tearDown(self):
        for process in self.processes:
            process.terminate()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)
        self.temporary.cleanup()

    def start_server(self, name):
        socket_path = self.root / (name + ".sock")
        process = subprocess.Popen(
            [
                "python3",
                str(SCRIPT),
                "serve",
                "--socket",
                str(socket_path),
                "--lineage-id",
                "shared-lineage",
                "--size-mib",
                "1",
                "--no-vsock",
            ]
        )
        self.processes.append(process)
        for _ in range(100):
            if socket_path.exists():
                return socket_path
            time.sleep(0.02)
        self.fail("probe server did not create its socket")

    def test_guest_probe_mutations_remain_isolated(self):
        socket_a = self.start_server("a")
        socket_b = self.start_server("b")
        a0 = PROBE.unix_request(str(socket_a), {"action": "status"})
        b0 = PROBE.unix_request(str(socket_b), {"action": "status"})
        self.assertEqual(a0["baseline_sha256"], b0["baseline_sha256"])
        a1 = PROBE.unix_request(
            str(socket_a), {"action": "mutate", "offset": 0, "length": 4096, "xor": 0xA5}
        )
        b1 = PROBE.unix_request(str(socket_b), {"action": "status"})
        self.assertNotEqual(a1["current_sha256"], a0["current_sha256"])
        self.assertEqual(b1["current_sha256"], b0["current_sha256"])

    def test_hybrid_vsock_request_performs_connect_handshake(self):
        socket_path = self.root / "hybrid.sock"
        ready = threading.Event()

        def fake_hypervisor():
            listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                listener.bind(str(socket_path))
                listener.listen(1)
                ready.set()
                connection, _ = listener.accept()
                with connection:
                    stream = connection.makefile("rwb", buffering=0)
                    self.assertEqual(stream.readline(128), b"CONNECT 16530\n")
                    stream.write(b"OK 1073741824\n")
                    request = json.loads(stream.readline(1024).decode("utf-8"))
                    self.assertEqual(request, {"action": "status"})
                    stream.write(b'{"ok":true,"transport":"hybrid-vsock"}\n')
            finally:
                listener.close()

        thread = threading.Thread(target=fake_hypervisor, daemon=True)
        thread.start()
        self.assertTrue(ready.wait(timeout=2))
        response = PROBE.hybrid_vsock_request(
            str(socket_path), 16530, {"action": "status"}
        )
        thread.join(timeout=2)
        self.assertFalse(thread.is_alive())
        self.assertEqual(response["transport"], "hybrid-vsock")

    def test_verifier_rejects_child_path_traversal(self):
        args = Namespace(child_a="../outside", child_b="child-b", transport="vsock")
        with self.assertRaisesRegex(ValueError, "safe VM identifiers"):
            PROBE.command_verify(args)

    def test_evidence_separates_snapshot_backing_from_resident_ram(self):
        baseline = {
            "lineage_id": "lineage",
            "pid": 444,
            "size_bytes": 1024 * 1024,
            "baseline_sha256": "a" * 64,
            "current_sha256": "a" * 64,
            "mutation_count": 0,
        }
        a1 = dict(baseline, current_sha256="b" * 64, mutation_count=1)
        b2 = dict(baseline, current_sha256="c" * 64, mutation_count=1)
        statuses = {
            "a_before": dict(baseline),
            "b_before": dict(baseline),
            "a_after_a_mutation": a1,
            "b_after_a_mutation": dict(baseline),
            "a_after_b_mutation": dict(a1),
            "b_after_b_mutation": b2,
        }
        metrics_a = {
            "guest_ram": {"available": True, "inodes": [101], "rss_kb": 8192, "pss_kb": 8000, "ksm_kb": 0},
            "snapshot_backing": {"available": True, "device_id": "7", "inode": 99},
        }
        metrics_b = {
            "guest_ram": {"available": True, "inodes": [102], "rss_kb": 8192, "pss_kb": 8000, "ksm_kb": 0},
            "snapshot_backing": {"available": True, "device_id": "7", "inode": 99},
        }
        evidence = PROBE.build_evidence(
            "child-a", "child-b", statuses, metrics_a, metrics_b, "cloud-hypervisor v53.0"
        )
        self.assertEqual(evidence["result"], "pass")
        self.assertTrue(evidence["memory_model"]["snapshot_backing_shared"])
        self.assertTrue(evidence["memory_model"]["guest_ram_mapping_inodes_distinct"])
        self.assertEqual(evidence["memory_model"]["defensible_shared_guest_ram_kb"], 0)
        self.assertEqual(evidence["memory_model"]["pss_mapping_savings_kb"], 384)
        self.assertEqual(evidence["memory_model"]["cross_child_sharing_basis"], "none")
        self.assertEqual(evidence["memory_model"]["claim"], "snapshot-backing-shared-only")


if __name__ == "__main__":
    unittest.main(verbosity=2)
