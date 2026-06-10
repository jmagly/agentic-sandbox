#!/usr/bin/env python3
"""Validate local documentation links without requiring the docsite builder."""

from __future__ import annotations

import argparse
import json
import re
import sys
from fnmatch import fnmatch
from pathlib import Path
from urllib.parse import unquote, urldefrag, urlparse


MARKDOWN_LINK_RE = re.compile(r"!?\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
HTML_HREF_RE = re.compile(r"\bhref=[\"']([^\"']+)[\"']", re.IGNORECASE)
FENCE_RE = re.compile(r"```.*?```", re.DOTALL)
INLINE_CODE_RE = re.compile(r"`[^`\n]+`")
HEADING_RE = re.compile(r"^(#{1,6})\s+(.+?)\s*#*\s*$", re.MULTILINE)


def load_manifest(docs_root: Path) -> dict:
    manifest = docs_root / "_manifest.json"
    if not manifest.exists():
        return {"exclude": []}
    return json.loads(manifest.read_text(encoding="utf-8"))


def is_excluded(path: Path, docs_root: Path, patterns: list[str]) -> bool:
    rel = path.relative_to(docs_root).as_posix()
    stem = rel.rsplit(".", 1)[0]
    return any(fnmatch(rel, pattern) or fnmatch(stem, pattern) for pattern in patterns)


def strip_code(text: str) -> str:
    return INLINE_CODE_RE.sub("", FENCE_RE.sub("", text))


def slugify(text: str) -> str:
    text = re.sub(r"<[^>]+>", "", text)
    text = re.sub(r"!\[[^\]]*\]\([^)]+\)", "", text)
    text = re.sub(r"\[[^\]]*\]\([^)]+\)", "", text)
    text = text.strip().lower()
    text = re.sub(r"[^\w\s-]", "", text)
    text = re.sub(r"\s+", "-", text)
    return text.strip("-")


def anchors_for(path: Path) -> set[str]:
    text = strip_code(path.read_text(encoding="utf-8"))
    anchors = set()
    seen: dict[str, int] = {}
    for match in HEADING_RE.finditer(text):
        slug = slugify(match.group(2))
        if not slug:
            continue
        count = seen.get(slug, 0)
        seen[slug] = count + 1
        anchors.add(slug if count == 0 else f"{slug}-{count}")
    return anchors


def candidate_paths(raw_path: str, source: Path, docs_root: Path) -> list[Path]:
    raw_path = unquote(raw_path)
    base = docs_root if raw_path.startswith("/") else source.parent
    normalized = (base / raw_path.lstrip("/")).resolve()
    candidates = [normalized]
    if normalized.suffix == "":
        candidates.append(normalized.with_suffix(".md"))
    if normalized.is_dir():
        candidates.append(normalized / "README.md")
        candidates.append(normalized / "index.md")
    return candidates


def resolve_target(raw_path: str, source: Path, docs_root: Path) -> Path | None:
    root = docs_root.resolve().parent
    for candidate in candidate_paths(raw_path, source, docs_root):
        try:
            candidate.relative_to(root)
        except ValueError:
            return None
        if candidate.exists() and (candidate.is_file() or candidate.is_dir()):
            return candidate
    return None


def iter_links(path: Path) -> list[str]:
    text = strip_code(path.read_text(encoding="utf-8"))
    return [m.group(1) for m in MARKDOWN_LINK_RE.finditer(text)] + [
        m.group(1) for m in HTML_HREF_RE.finditer(text)
    ]


def is_external(link: str) -> bool:
    parsed = urlparse(link)
    return bool(parsed.scheme or parsed.netloc) or link.startswith(("mailto:", "tel:"))


def validate(docs_root: Path) -> list[str]:
    manifest = load_manifest(docs_root)
    excludes = list(manifest.get("exclude", []))
    docs_root = docs_root.resolve()
    repo_root = docs_root.parent
    markdown_files = [
        path
        for path in docs_root.rglob("*.md")
        if not is_excluded(path, docs_root, excludes)
    ]
    repo_markdown = list(repo_root.rglob("*.md"))
    anchor_cache = {path.resolve(): anchors_for(path) for path in repo_markdown}
    errors: list[str] = []

    for source in markdown_files:
        for link in iter_links(source):
            if is_external(link) or link.startswith("#/"):
                continue
            target_part, fragment = urldefrag(link)
            if not target_part and fragment:
                target = source.resolve()
            elif target_part:
                target = resolve_target(target_part, source, docs_root)
                if target is None:
                    errors.append(f"{source.relative_to(docs_root)}: broken link: {link}")
                    continue
                target = target.resolve()
            else:
                continue

            if fragment and target.suffix.lower() == ".md":
                wanted = slugify(fragment)
                if wanted and wanted not in anchor_cache.get(target, set()):
                    errors.append(
                        f"{source.relative_to(docs_root)}: missing anchor: {link}"
                    )
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--docs-root", default="docs", type=Path)
    args = parser.parse_args()

    errors = validate(args.docs_root)
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        print(f"doc link check failed: {len(errors)} error(s)", file=sys.stderr)
        return 1

    print(f"doc link check passed: {args.docs_root}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
