#!/usr/bin/env python3
"""Fail when a release binary contains host-specific absolute paths."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


BUILTIN_FORBIDDEN = (b"/Users/", b"/home/")


def offsets(payload: bytes, needle: bytes) -> list[int]:
    found = []
    start = 0
    while True:
        offset = payload.find(needle, start)
        if offset < 0:
            return found
        found.append(offset)
        start = offset + 1


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    parser.add_argument("--forbid", action="append", default=[])
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()

    if not args.binary.is_file():
        raise SystemExit(f"binary path scan: missing regular file: {args.binary}")
    payload = args.binary.read_bytes()
    needles = list(BUILTIN_FORBIDDEN)
    needles.extend(value.encode("utf-8") for value in args.forbid if value)

    matches = []
    for index, needle in enumerate(dict.fromkeys(needles)):
        hits = offsets(payload, needle)
        if hits:
            matches.append(
                {
                    "rule": f"forbidden-path-{index + 1}",
                    "match_count": len(hits),
                    "first_offsets": hits[:10],
                }
            )

    result = {
        "schema": "release-binary-path-scan.v1",
        "binary_size": len(payload),
        "forbidden_rule_count": len(dict.fromkeys(needles)),
        "status": "FAIL" if matches else "PASS",
        "matches": matches,
    }
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(
            json.dumps(result, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )

    if matches:
        print("binary path scan: FAIL (host path bytes found; see JSON report)")
        raise SystemExit(1)
    print(
        "binary path scan: PASS "
        f"({len(payload)} bytes, {result['forbidden_rule_count']} rules)"
    )


if __name__ == "__main__":
    main()
