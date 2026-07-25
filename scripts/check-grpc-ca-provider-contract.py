#!/usr/bin/env python3
"""Validate Agentic Sandbox CA-provider v1 wire fixtures."""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any


MAX_REQUEST_BYTES = 1024 * 1024
MAX_RESPONSE_BYTES = 4 * 1024 * 1024
FIXTURE_NAMES = (
    "describe.response.json",
    "health.response.json",
    "trust-bundle.request.json",
    "trust-bundle.response.json",
    "sign.request.json",
    "sign.response.json",
)
REQUEST_ID_RE = re.compile(r"^[A-Za-z0-9._:-]+$")
TRUST_DOMAIN_RE = re.compile(r"^[A-Za-z0-9.-]+$")
CAPABILITY_RE = re.compile(r"^[A-Z][A-Z0-9_]*$")
DIAGNOSTIC_RE = re.compile(r"^[a-z][a-z0-9_]*$")


class ValidationError(ValueError):
    """A non-sensitive contract validation failure."""


def _reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValidationError("duplicate object key")
        result[key] = value
    return result


def _load_json(path: Path, limit: int) -> dict[str, Any]:
    try:
        size = path.stat().st_size
    except OSError as error:
        raise ValidationError(f"{path.name}: fixture unavailable") from error
    if size > limit:
        raise ValidationError(f"{path.name}: serialized size exceeds protocol limit")
    try:
        value = json.loads(
            path.read_text(encoding="utf-8"),
            object_pairs_hook=_reject_duplicate_keys,
        )
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ValidationError(f"{path.name}: invalid JSON") from error
    if type(value) is not dict:
        raise ValidationError(f"{path.name}: expected object")
    return value


def _object(
    value: Any,
    *,
    required: set[str],
    optional: set[str] = frozenset(),
    label: str,
) -> dict[str, Any]:
    if type(value) is not dict:
        raise ValidationError(f"{label}: expected object")
    keys = set(value)
    missing = required - keys
    unknown = keys - required - optional
    if missing:
        raise ValidationError(f"{label}: missing required field")
    if unknown:
        raise ValidationError(f"{label}: unknown field")
    return value


def _string(
    value: Any,
    *,
    label: str,
    maximum: int,
    pattern: re.Pattern[str] | None = None,
) -> str:
    if type(value) is not str or not value or len(value) > maximum:
        raise ValidationError(f"{label}: invalid string")
    if pattern is not None and pattern.fullmatch(value) is None:
        raise ValidationError(f"{label}: invalid format")
    return value


def _nullable_string(
    value: Any,
    *,
    label: str,
    maximum: int,
    pattern: re.Pattern[str] | None = None,
) -> None:
    if value is not None:
        _string(value, label=label, maximum=maximum, pattern=pattern)


def _protocol(value: Any, label: str) -> None:
    value = _object(
        value,
        required={"major", "minor"},
        label=f"{label}.protocol",
    )
    if type(value["major"]) is not int or value["major"] != 1:
        raise ValidationError(f"{label}.protocol: unsupported major")
    if (
        type(value["minor"]) is not int
        or value["minor"] < 0
        or value["minor"] > 65535
    ):
        raise ValidationError(f"{label}.protocol: invalid minor")


def _common(value: dict[str, Any], label: str) -> None:
    _protocol(value["protocol"], label)


def _validate_describe(value: dict[str, Any]) -> None:
    label = "describe.response.json"
    value = _object(
        value,
        required={
            "protocol",
            "implementation",
            "implementation_version",
            "capabilities",
        },
        optional={"build_provenance"},
        label=label,
    )
    _common(value, label)
    _string(value["implementation"], label=label, maximum=256)
    _string(value["implementation_version"], label=label, maximum=256)
    _nullable_string(value.get("build_provenance"), label=label, maximum=256)
    capabilities = value["capabilities"]
    if type(capabilities) is not list or len(capabilities) > 64:
        raise ValidationError(f"{label}: invalid capabilities")
    seen: set[str] = set()
    for capability in capabilities:
        _string(capability, label=label, maximum=64, pattern=CAPABILITY_RE)
        if capability in seen:
            raise ValidationError(f"{label}: duplicate capability")
        seen.add(capability)


def _validate_health(value: dict[str, Any]) -> None:
    label = "health.response.json"
    value = _object(
        value,
        required={"protocol", "state"},
        optional={"diagnostics_code"},
        label=label,
    )
    _common(value, label)
    state = _string(value["state"], label=label, maximum=11)
    if state not in {"ready", "degraded", "unavailable"}:
        raise ValidationError(f"{label}: invalid state")
    _nullable_string(
        value.get("diagnostics_code"),
        label=label,
        maximum=128,
        pattern=DIAGNOSTIC_RE,
    )


def _validate_trust_bundle_request(value: dict[str, Any]) -> None:
    label = "trust-bundle.request.json"
    value = _object(
        value,
        required={"protocol", "request_id", "expected_trust_domain"},
        label=label,
    )
    _common(value, label)
    _string(value["request_id"], label=label, maximum=128, pattern=REQUEST_ID_RE)
    _string(
        value["expected_trust_domain"],
        label=label,
        maximum=253,
        pattern=TRUST_DOMAIN_RE,
    )


def _validate_trust_bundle_response(value: dict[str, Any]) -> None:
    label = "trust-bundle.response.json"
    value = _object(
        value,
        required={
            "protocol",
            "request_id",
            "trust_domain",
            "bundle_pem",
            "revision",
        },
        label=label,
    )
    _common(value, label)
    _string(value["request_id"], label=label, maximum=128, pattern=REQUEST_ID_RE)
    _string(
        value["trust_domain"],
        label=label,
        maximum=253,
        pattern=TRUST_DOMAIN_RE,
    )
    _string(value["bundle_pem"], label=label, maximum=MAX_RESPONSE_BYTES)
    _string(value["revision"], label=label, maximum=256)


def _validate_sign_request(value: dict[str, Any]) -> None:
    label = "sign.request.json"
    value = _object(
        value,
        required={
            "protocol",
            "request_id",
            "spiffe_id",
            "csr_pem",
            "requested_ttl_seconds",
            "expected_trust_domain",
        },
        label=label,
    )
    _common(value, label)
    _string(value["request_id"], label=label, maximum=128, pattern=REQUEST_ID_RE)
    spiffe_id = _string(value["spiffe_id"], label=label, maximum=2048)
    if not spiffe_id.startswith("spiffe://"):
        raise ValidationError(f"{label}: invalid SPIFFE ID")
    _string(value["csr_pem"], label=label, maximum=MAX_REQUEST_BYTES)
    if (
        type(value["requested_ttl_seconds"]) is not int
        or value["requested_ttl_seconds"] <= 0
    ):
        raise ValidationError(f"{label}: invalid requested TTL")
    _string(
        value["expected_trust_domain"],
        label=label,
        maximum=253,
        pattern=TRUST_DOMAIN_RE,
    )


def _validate_sign_response(value: dict[str, Any]) -> None:
    label = "sign.response.json"
    value = _object(
        value,
        required={
            "protocol",
            "request_id",
            "spiffe_id",
            "certificate_chain_pem",
            "bundle_revision",
        },
        optional={"provider_audit_id"},
        label=label,
    )
    _common(value, label)
    _string(value["request_id"], label=label, maximum=128, pattern=REQUEST_ID_RE)
    spiffe_id = _string(value["spiffe_id"], label=label, maximum=2048)
    if not spiffe_id.startswith("spiffe://"):
        raise ValidationError(f"{label}: invalid SPIFFE ID")
    _string(
        value["certificate_chain_pem"],
        label=label,
        maximum=MAX_RESPONSE_BYTES,
    )
    _string(value["bundle_revision"], label=label, maximum=256)
    _nullable_string(value.get("provider_audit_id"), label=label, maximum=256)


VALIDATORS = {
    "describe.response.json": _validate_describe,
    "health.response.json": _validate_health,
    "trust-bundle.request.json": _validate_trust_bundle_request,
    "trust-bundle.response.json": _validate_trust_bundle_response,
    "sign.request.json": _validate_sign_request,
    "sign.response.json": _validate_sign_response,
}


def validate_fixture_directory(fixtures: Path) -> None:
    values: dict[str, dict[str, Any]] = {}
    for name in FIXTURE_NAMES:
        limit = MAX_REQUEST_BYTES if name.endswith("request.json") else MAX_RESPONSE_BYTES
        value = _load_json(fixtures / name, limit)
        VALIDATORS[name](value)
        values[name] = value

    bundle_request = values["trust-bundle.request.json"]
    bundle_response = values["trust-bundle.response.json"]
    if bundle_request["request_id"] != bundle_response["request_id"]:
        raise ValidationError("trust-bundle fixtures: request correlation mismatch")
    if bundle_request["expected_trust_domain"] != bundle_response["trust_domain"]:
        raise ValidationError("trust-bundle fixtures: trust-domain mismatch")

    sign_request = values["sign.request.json"]
    sign_response = values["sign.response.json"]
    if sign_request["request_id"] != sign_response["request_id"]:
        raise ValidationError("sign fixtures: request correlation mismatch")
    if sign_request["spiffe_id"] != sign_response["spiffe_id"]:
        raise ValidationError("sign fixtures: SPIFFE ID mismatch")
    if not sign_request["spiffe_id"].startswith(
        f"spiffe://{sign_request['expected_trust_domain']}/"
    ):
        raise ValidationError("sign fixtures: trust-domain mismatch")
    if bundle_response["revision"] != sign_response["bundle_revision"]:
        raise ValidationError("fixtures: bundle revision mismatch")


def main() -> int:
    repo_root = Path(__file__).resolve().parent.parent
    default_fixtures = (
        repo_root / "docs" / "contracts" / "ca-provider" / "v1" / "fixtures"
    )
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixtures", type=Path, default=default_fixtures)
    args = parser.parse_args()

    try:
        validate_fixture_directory(args.fixtures.resolve())
    except ValidationError as error:
        print(f"CA provider v1 contract check failed: {error}", file=sys.stderr)
        return 1
    print(f"CA provider v1 contract check passed: {args.fixtures.resolve()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
