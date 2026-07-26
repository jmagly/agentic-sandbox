#!/usr/bin/env python3
"""Validate sanitized macOS release evidence without third-party dependencies."""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any


MAX_DOCUMENT_BYTES = 256 * 1024
SCHEMA_ID = (
    "https://agentic-sandbox.dev/contracts/"
    "macos-release-evidence/v1/release-evidence.schema.json"
)
ALLOWED_CREDENTIAL_KEY = "credential_contents_retained"
FORBIDDEN_KEY_PARTS = {
    "access_token",
    "api_key",
    "apikey",
    "app_store_connect_key",
    "authorization",
    "auth_token",
    "credential",
    "keychain_export",
    "notary_password",
    "p8",
    "password",
    "private_key",
    "privatekey",
    "refresh_token",
    "secret",
}


class ValidationError(Exception):
    """A validation failure whose message never includes document values."""


def fail(path: str, rule: str) -> None:
    raise ValidationError(f"{path}: {rule}")


def reject_secret_bearing_keys(value: Any, path: str = "$") -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            normalized = re.sub(r"[^a-z0-9]+", "_", key.lower()).strip("_")
            if key != ALLOWED_CREDENTIAL_KEY and any(
                part in normalized for part in FORBIDDEN_KEY_PARTS
            ):
                fail(path, "secret-bearing field name is forbidden")
            reject_secret_bearing_keys(child, f"{path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            reject_secret_bearing_keys(child, f"{path}[{index}]")


def resolve_ref(root: dict[str, Any], ref: str) -> dict[str, Any]:
    if not ref.startswith("#/"):
        raise ValidationError("$schema: only local references are supported")
    value: Any = root
    for token in ref[2:].split("/"):
        token = token.replace("~1", "/").replace("~0", "~")
        if not isinstance(value, dict) or token not in value:
            raise ValidationError("$schema: unresolved local reference")
        value = value[token]
    if not isinstance(value, dict):
        raise ValidationError("$schema: local reference must resolve to an object")
    return value


def is_type(value: Any, expected: str) -> bool:
    return {
        "object": isinstance(value, dict),
        "array": isinstance(value, list),
        "string": isinstance(value, str),
        "boolean": isinstance(value, bool),
        "integer": isinstance(value, int) and not isinstance(value, bool),
        "number": isinstance(value, (int, float)) and not isinstance(value, bool),
        "null": value is None,
    }.get(expected, False)


def validate(
    value: Any,
    schema: dict[str, Any],
    root: dict[str, Any],
    path: str = "$",
) -> None:
    if "$ref" in schema:
        validate(value, resolve_ref(root, schema["$ref"]), root, path)

    for branch in schema.get("allOf", []):
        validate(value, branch, root, path)

    if "const" in schema and value != schema["const"]:
        fail(path, "value does not match the required constant")
    if "enum" in schema and value not in schema["enum"]:
        fail(path, "value is not in the permitted set")

    expected_type = schema.get("type")
    if expected_type is not None and not is_type(value, expected_type):
        fail(path, f"expected {expected_type}")

    if isinstance(value, str):
        if len(value) < schema.get("minLength", 0):
            fail(path, "string is shorter than permitted")
        if "maxLength" in schema and len(value) > schema["maxLength"]:
            fail(path, "string is longer than permitted")
        if "pattern" in schema and re.search(schema["pattern"], value) is None:
            fail(path, "string does not match the public metadata format")

    if isinstance(value, dict):
        properties = schema.get("properties", {})
        for name in schema.get("required", []):
            if name not in value:
                fail(path, f"required field is missing: {name}")
        if schema.get("additionalProperties") is False:
            unexpected = set(value) - set(properties)
            if unexpected:
                fail(path, "document contains a field outside the closed schema")
        for name, child_schema in properties.items():
            if name in value:
                validate(value[name], child_schema, root, f"{path}.{name}")

    if isinstance(value, list):
        if len(value) < schema.get("minItems", 0):
            fail(path, "array has too few entries")
        if "maxItems" in schema and len(value) > schema["maxItems"]:
            fail(path, "array has too many entries")
        prefix = schema.get("prefixItems", [])
        for index, child_schema in enumerate(prefix):
            if index < len(value):
                validate(value[index], child_schema, root, f"{path}[{index}]")
        items = schema.get("items")
        if items is False and len(value) > len(prefix):
            fail(path, "array contains entries outside the closed schema")
        if isinstance(items, dict):
            for index, child in enumerate(value[len(prefix) :], start=len(prefix)):
                validate(child, items, root, f"{path}[{index}]")


def load_json(path: Path, label: str) -> Any:
    try:
        size = path.stat().st_size
    except OSError as exc:
        raise ValidationError(f"{label}: cannot read file") from exc
    if size > MAX_DOCUMENT_BYTES:
        raise ValidationError(f"{label}: document exceeds {MAX_DOCUMENT_BYTES} bytes")
    try:
        def closed_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
            result: dict[str, Any] = {}
            for key, value in pairs:
                if key in result:
                    raise ValidationError(f"{label}: duplicate object field")
                result[key] = value
            return result

        return json.loads(
            path.read_text(encoding="utf-8"),
            object_pairs_hook=closed_object,
        )
    except ValidationError:
        raise
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ValidationError(f"{label}: invalid JSON") from exc


def require_binding(document: dict[str, Any], args: argparse.Namespace) -> None:
    release = document.get("release", {})
    package = document.get("package", {})
    bindings = (
        (args.expect_source_commit, release.get("source_commit"), "source commit"),
        (args.expect_tag, release.get("tag"), "release tag"),
        (
            args.expect_operator_approval_ref,
            release.get("operator_approval_ref"),
            "operator approval reference",
        ),
        (
            args.expect_preview_manifest_sha256,
            package.get("approved_preview_manifest", {}).get("sha256"),
            "preview manifest digest",
        ),
        (
            args.expect_preview_package_sha256,
            package.get("approved_preview_package", {}).get("sha256"),
            "preview package digest",
        ),
    )
    for expected, actual, label in bindings:
        if expected is not None and actual != expected:
            raise ValidationError(f"$: evidence does not bind the expected {label}")


def require_relations(document: dict[str, Any]) -> None:
    release = document["release"]
    package = document["package"]
    version = release["version"]
    expected_tag = f"v{version}"
    if release["tag"] != expected_tag:
        raise ValidationError("$: release tag and version are inconsistent")

    base = f"agentic-sandbox-v{version}-aarch64-darwin"
    expected_names = (
        (
            package["approved_preview_package"]["name"],
            f"{base}-preview.pkg",
            "approved preview package filename",
        ),
        (
            package["approved_preview_manifest"]["name"],
            f"{base}.payload-manifest.tsv",
            "approved preview manifest filename",
        ),
        (
            package["signed_payload_manifest"]["name"],
            f"{base}.payload-manifest.tsv",
            "signed payload manifest filename",
        ),
        (document["artifacts"][0]["name"], f"{base}.pkg", "package artifact filename"),
        (document["artifacts"][1]["name"], f"{base}.dmg", "DMG artifact filename"),
    )
    for actual, expected, label in expected_names:
        if actual != expected:
            raise ValidationError(f"$: {label} is inconsistent with release version")

    team_id = package["team_id"]
    preflight = document["preflight"]
    for identity_name in ("application_identity", "installer_identity"):
        identity = preflight[identity_name]
        if identity["team_id"] != team_id or not identity["selector"].endswith(
            f" ({team_id})"
        ):
            raise ValidationError("$: identity selector and package Team ID differ")

    expected_entitlements = package["expected_entitlements_sha256"]
    if any(
        payload["expected_entitlements_sha256"] != expected_entitlements
        for payload in document["payloads"]
    ):
        raise ValidationError("$: payload entitlement policy digests differ")


def parse_args() -> argparse.Namespace:
    root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(
        description="Validate closed, credential-free macOS release evidence"
    )
    parser.add_argument("evidence", type=Path)
    parser.add_argument(
        "--schema",
        type=Path,
        default=root
        / "docs/contracts/macos-release-evidence/v1/release-evidence.schema.json",
    )
    parser.add_argument("--expect-source-commit")
    parser.add_argument("--expect-tag")
    parser.add_argument("--expect-operator-approval-ref")
    parser.add_argument("--expect-preview-manifest-sha256")
    parser.add_argument("--expect-preview-package-sha256")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        schema = load_json(args.schema, "$schema")
        if not isinstance(schema, dict) or schema.get("$id") != SCHEMA_ID:
            raise ValidationError("$schema: unexpected schema identity")
        document = load_json(args.evidence, "$")
        if not isinstance(document, dict):
            raise ValidationError("$: expected object")
        reject_secret_bearing_keys(document)
        validate(document, schema, schema)
        require_relations(document)
        require_binding(document, args)
    except ValidationError as exc:
        print(f"macOS release evidence rejected: {exc}", file=sys.stderr)
        return 1
    print("macOS release evidence: valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
