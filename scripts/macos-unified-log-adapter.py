#!/usr/bin/env python3
"""Normalize allowlisted `log stream --style json` records without log content."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys

SAFE_LABEL = re.compile(r"^[A-Za-z0-9._-]{1,128}$")


def digest(value: str) -> str:
    return "sha256:" + hashlib.sha256(value.encode("utf-8", errors="replace")).hexdigest()


def normalize(record: dict[str, object], allowed: set[str]) -> dict[str, object] | None:
    subsystem = record.get("subsystem")
    category = record.get("category")
    message = record.get("eventMessage")
    level = record.get("messageType", "info")
    process_id = record.get("processID")
    if not isinstance(subsystem, str) or subsystem not in allowed:
        return None
    if not isinstance(category, str) or SAFE_LABEL.fullmatch(category) is None:
        return None
    if not isinstance(message, str):
        message = ""
    return {
        "event": "system.unified_log",
        "subsystem": subsystem,
        "category": category,
        "level": str(level).lower(),
        "process_id": process_id if isinstance(process_id, int) else None,
        "message_digest": digest(message),
        "content_captured": False,
        "source_adapter": "unified-log",
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--subsystem",
        action="append",
        default=["io.aiwg.agentic-sandbox", "com.apple.endpointsecurity"],
        help="allowlisted subsystem (repeatable)",
    )
    args = parser.parse_args()
    allowed = set(args.subsystem)
    for line in sys.stdin:
        try:
            record = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(record, dict) and (output := normalize(record, allowed)) is not None:
            print(json.dumps(output, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
