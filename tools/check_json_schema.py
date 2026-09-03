#!/usr/bin/env python3
"""Validate strict JSON documents with a published Draft 2020-12 Schema."""

from __future__ import annotations

import argparse
from pathlib import Path

from jsonschema import Draft202012Validator
from validate_asset import load_strict_json


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SCHEMA = ROOT / "asset-format/explicit-v1.schema.json"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("documents", nargs="*", type=Path)
    parser.add_argument("--schema", type=Path, default=DEFAULT_SCHEMA)
    parser.add_argument(
        "--schema-only",
        action="store_true",
        help="check the Schema itself without validating instance documents",
    )
    args = parser.parse_args()

    if args.schema_only and args.documents:
        parser.error("--schema-only does not accept instance documents")
    if not args.schema_only and not args.documents:
        parser.error("at least one instance document is required")

    schema = load_strict_json(args.schema)
    Draft202012Validator.check_schema(schema)
    if args.schema_only:
        print(f"{args.schema}: schema definition PASS")
        return
    validator = Draft202012Validator(schema)
    failed = False
    for document_path in args.documents:
        instance = load_strict_json(document_path)
        errors = sorted(validator.iter_errors(instance), key=lambda error: list(error.path))
        if errors:
            failed = True
            for error in errors:
                location = "/".join(str(part) for part in error.absolute_path) or "<root>"
                print(f"{document_path}:{location}: {error.message}")
        else:
            print(f"{document_path}: schema PASS")
    if failed:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
