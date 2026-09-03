#!/usr/bin/env python3
"""Export a deterministic, path-free inventory from the checked-in lock files.

This intentionally uses only the Python standard library.  It is dependency
evidence, not a substitute for a license review or a full runtime SBOM.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CARGO_LOCK = ROOT / "player" / "Cargo.lock"
NPM_LOCK = ROOT / "lessons" / "package-lock.json"
PYTHON_LOCK = ROOT / "tools" / "requirements-schema.lock"
PYTHON_REQUIREMENT = re.compile(
    r"^([A-Za-z0-9_.-]+)==([^\s;\\]+)(?:\s*;\s*(.*?))?\s*\\?\s*$"
)
PYTHON_HASH = re.compile(r"--hash=sha256:([0-9a-f]{64})")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def cargo_packages() -> list[dict[str, object]]:
    lock = tomllib.loads(CARGO_LOCK.read_text(encoding="utf-8"))
    packages = []
    for package in lock["package"]:
        item: dict[str, object] = {
            "name": package["name"],
            "version": package["version"],
            "dependencies": sorted(package.get("dependencies", [])),
        }
        for field in ("source", "checksum"):
            if field in package:
                item[field] = package[field]
        packages.append(item)
    return sorted(
        packages,
        key=lambda package: (
            str(package["name"]),
            str(package["version"]),
            str(package.get("source", "")),
        ),
    )


def npm_name(path: str, package: dict[str, object]) -> str:
    if "name" in package:
        return str(package["name"])
    return path.rsplit("node_modules/", 1)[-1]


def npm_packages() -> list[dict[str, object]]:
    lock = json.loads(NPM_LOCK.read_text(encoding="utf-8"))
    packages = []
    for path, package in lock["packages"].items():
        if not path:
            continue
        item: dict[str, object] = {
            "path": path,
            "name": npm_name(path, package),
            "version": package["version"],
        }
        for field in (
            "resolved",
            "integrity",
            "license",
            "dev",
            "optional",
            "os",
            "cpu",
            "dependencies",
            "optionalDependencies",
        ):
            if field in package:
                item[field] = package[field]
        packages.append(item)
    return sorted(packages, key=lambda package: str(package["path"]))


def python_packages() -> list[dict[str, object]]:
    lines = PYTHON_LOCK.read_text(encoding="utf-8").splitlines()
    packages: list[dict[str, object]] = []
    current: dict[str, object] | None = None
    for line in lines:
        match = PYTHON_REQUIREMENT.match(line)
        if match:
            current = {
                "name": match.group(1),
                "version": match.group(2),
                "hashes": [],
            }
            marker = match.group(3)
            if marker:
                current["marker"] = marker.strip()
            packages.append(current)
            continue
        if current is not None:
            current["hashes"].extend(PYTHON_HASH.findall(line))
    for package in packages:
        package["hashes"] = sorted(set(package["hashes"]))
    return sorted(packages, key=lambda package: str(package["name"]).lower())


def inventory() -> dict[str, object]:
    return {
        "schema": "lockfile-dependency-inventory.v1",
        "sources": {
            "cargo": {
                "path": "player/Cargo.lock",
                "sha256": sha256(CARGO_LOCK),
            },
            "npm": {
                "path": "lessons/package-lock.json",
                "sha256": sha256(NPM_LOCK),
            },
            "python": {
                "path": "tools/requirements-schema.lock",
                "sha256": sha256(PYTHON_LOCK),
            },
        },
        "packages": {
            "cargo": cargo_packages(),
            "npm": npm_packages(),
            "python": python_packages(),
        },
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(inventory(), indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(f"dependency inventory: wrote {args.output}")


if __name__ == "__main__":
    main()
