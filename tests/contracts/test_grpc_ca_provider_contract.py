#!/usr/bin/env python3
"""Regression tests for the dependency-free CA-provider contract checker."""

from __future__ import annotations

import importlib.util
import json
import shutil
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT = REPO_ROOT / "scripts" / "check-grpc-ca-provider-contract.py"
FIXTURES = REPO_ROOT / "docs" / "contracts" / "ca-provider" / "v1" / "fixtures"
SCHEMA = FIXTURES.parent / "protocol.schema.json"
SPEC = importlib.util.spec_from_file_location("ca_provider_contract", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
CONTRACT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CONTRACT)


class CaProviderContractTests(unittest.TestCase):
    def fixture_copy(self) -> tuple[tempfile.TemporaryDirectory[str], Path]:
        temporary = tempfile.TemporaryDirectory()
        destination = Path(temporary.name) / "fixtures"
        shutil.copytree(FIXTURES, destination)
        return temporary, destination

    def mutate(self, fixtures: Path, name: str, mutation) -> None:
        path = fixtures / name
        value = json.loads(path.read_text(encoding="utf-8"))
        mutation(value)
        path.write_text(json.dumps(value), encoding="utf-8")

    def assert_rejected(self, fixtures: Path) -> None:
        with self.assertRaises(CONTRACT.ValidationError):
            CONTRACT.validate_fixture_directory(fixtures)

    def test_committed_fixtures_pass(self) -> None:
        CONTRACT.validate_fixture_directory(FIXTURES)

    def test_schema_definitions_are_complete_and_closed(self) -> None:
        schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
        expected = {
            "describeResponse",
            "healthResponse",
            "trustBundleRequest",
            "trustBundleResponse",
            "signRequest",
            "signResponse",
        }
        definitions = schema["$defs"]
        self.assertTrue(expected.issubset(definitions))
        for name in expected:
            self.assertFalse(definitions[name]["additionalProperties"])

    def test_unknown_field_fails_closed(self) -> None:
        temporary, fixtures = self.fixture_copy()
        self.addCleanup(temporary.cleanup)
        self.mutate(
            fixtures,
            "describe.response.json",
            lambda value: value.update({"unexpected": True}),
        )
        self.assert_rejected(fixtures)

        temporary_two, fixtures_two = self.fixture_copy()
        self.addCleanup(temporary_two.cleanup)
        self.mutate(
            fixtures_two,
            "describe.response.json",
            lambda value: value.update({"capabilities": [{}]}),
        )
        self.assert_rejected(fixtures_two)

    def test_malformed_major_and_cross_message_mismatch_fail_closed(self) -> None:
        temporary, fixtures = self.fixture_copy()
        self.addCleanup(temporary.cleanup)
        self.mutate(
            fixtures,
            "health.response.json",
            lambda value: value["protocol"].update({"major": 2}),
        )
        self.assert_rejected(fixtures)

        temporary_two, fixtures_two = self.fixture_copy()
        self.addCleanup(temporary_two.cleanup)
        self.mutate(
            fixtures_two,
            "sign.response.json",
            lambda value: value.update({"request_id": "different-request"}),
        )
        self.assert_rejected(fixtures_two)

    def test_duplicate_key_and_oversized_fixture_fail_closed(self) -> None:
        temporary, fixtures = self.fixture_copy()
        self.addCleanup(temporary.cleanup)
        (fixtures / "health.response.json").write_text(
            '{"protocol":{"major":1,"minor":0},"state":"ready","state":"degraded"}',
            encoding="utf-8",
        )
        self.assert_rejected(fixtures)

        temporary_two, fixtures_two = self.fixture_copy()
        self.addCleanup(temporary_two.cleanup)
        (fixtures_two / "sign.request.json").write_bytes(
            b" " * (CONTRACT.MAX_REQUEST_BYTES + 1)
        )
        self.assert_rejected(fixtures_two)


if __name__ == "__main__":
    unittest.main()
