#!/usr/bin/env python3
"""Audit the exact non-ignored files that could enter the next Git commit."""

from __future__ import annotations

import re
import subprocess
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parents[1]
MAX_SOURCE_BYTES = 5 * 1024 * 1024
TEXT_SUFFIXES = {
    ".cff",
    ".code-workspace",
    ".css",
    ".html",
    ".js",
    ".json",
    ".lock",
    ".md",
    ".py",
    ".rs",
    ".sh",
    ".toml",
    ".txt",
    ".wgsl",
    ".yaml",
    ".yml",
}
TEXT_NAMES = {".gitignore", ".node-version", "LICENSE", "NOTICE"}
BINARY_ALLOWLIST = {
    PurePosixPath("docs/assets/remote-frame-synthetic-motion.gif"),
    PurePosixPath("examples/minimal-sh0/gaussians.bin"),
    PurePosixPath("examples/synthetic-motion-sh3/gaussians.bin"),
    PurePosixPath("examples/synthetic-motion-sh3/sh3.f16"),
}
PRIVATE_ARTIFACT_SUFFIXES = {
    ".ckpt",
    ".env",
    ".jpeg",
    ".jpg",
    ".key",
    ".mov",
    ".mp4",
    ".npy",
    ".npz",
    ".p12",
    ".pem",
    ".pfx",
    ".ply",
    ".png",
    ".pt",
    ".pth",
    ".rgba8",
    ".safetensors",
    ".splat",
    ".webm",
}
ENVIRONMENT_PATTERNS = {
    "private macOS home path": re.compile(r"/Users/[A-Za-z0-9._-]+/"),
    "private Linux home path": re.compile(r"/home/(?!user(?:/|\b))[A-Za-z0-9._-]+/"),
    "scratch/storage absolute path": re.compile(r"/(?:mnt|scratch|data)/(?:[A-Za-z0-9._-]+/)+"),
    "RFC1918 IPv4 address": re.compile(
        r"(?<![0-9])(?:10\.(?:[0-9]{1,3}\.){2}[0-9]{1,3}|192\.168\.(?:[0-9]{1,3}\.)[0-9]{1,3}|172\.(?:1[6-9]|2[0-9]|3[01])\.(?:[0-9]{1,3}\.)[0-9]{1,3})(?![0-9])"
    ),
}
SECRET_PATTERNS = {
    "Hugging Face access token": re.compile(r"hf_[A-Za-z0-9]{20,}"),
    "GitHub access token": re.compile(r"gh[pousr]_[A-Za-z0-9]{20,}"),
    "private key": re.compile(r"BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY"),
}
TRUSTED_GITHUB_ACTIONS = {
    "actions/checkout",
    "actions/setup-node",
    "actions/setup-python",
    "actions/upload-artifact",
}
ACTION_USE_LINE = re.compile(
    r"(?m)^\s*(?:-\s*)?uses:\s*(.+?)(?:\s+#.*)?$"
)
FULL_GIT_COMMIT = re.compile(r"[0-9a-f]{40}")


def candidate_paths() -> list[PurePosixPath]:
    command = ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"]
    result = subprocess.run(command, cwd=ROOT, check=True, capture_output=True)
    try:
        names = result.stdout.decode("utf-8").split("\0")
    except UnicodeDecodeError as error:
        raise SystemExit(f"public-tree audit: Git path is not UTF-8: {error}") from error
    return sorted(PurePosixPath(name) for name in names if name)


def add_matches(
    findings: list[str],
    relative: PurePosixPath,
    text: str,
    patterns: dict[str, re.Pattern],
) -> None:
    for label, pattern in patterns.items():
        for match in pattern.finditer(text):
            line = text.count("\n", 0, match.start()) + 1
            findings.append(f"{relative}:{line}: {label}")


def check_workflow_actions(
    findings: list[str], relative: PurePosixPath, text: str
) -> None:
    if len(relative.parts) < 3 or relative.parts[:2] != (".github", "workflows"):
        return
    for match in ACTION_USE_LINE.finditer(text):
        reference = match.group(1).strip()
        line = text.count("\n", 0, match.start()) + 1
        if (
            len(reference) >= 2
            and reference[0] == reference[-1]
            and reference[0] in "\"'"
        ):
            reference = reference[1:-1]
        if "@" not in reference:
            findings.append(f"{relative}:{line}: GitHub Action has no commit revision")
            continue
        action, revision = reference.rsplit("@", 1)
        if action not in TRUSTED_GITHUB_ACTIONS:
            findings.append(f"{relative}:{line}: GitHub Action is not allowlisted: {action}")
        if not FULL_GIT_COMMIT.fullmatch(revision):
            findings.append(
                f"{relative}:{line}: GitHub Action must use a full 40-character commit"
            )


def main() -> None:
    findings: list[str] = []
    paths = candidate_paths()
    text_count = 0
    binary_count = 0

    for relative in paths:
        path = ROOT.joinpath(*relative.parts)
        if path.is_symlink():
            findings.append(f"{relative}: symbolic links are not allowed in the release tree")
            continue
        if not path.is_file():
            findings.append(f"{relative}: candidate file is missing or not regular")
            continue
        size = path.stat().st_size
        if size > MAX_SOURCE_BYTES:
            findings.append(f"{relative}: {size} bytes exceeds the {MAX_SOURCE_BYTES}-byte source limit")

        payload = path.read_bytes()
        secret_text = payload.decode("utf-8", errors="ignore")
        add_matches(findings, relative, secret_text, SECRET_PATTERNS)

        if relative in BINARY_ALLOWLIST:
            binary_count += 1
            continue
        if path.suffix.lower() in PRIVATE_ARTIFACT_SUFFIXES:
            findings.append(f"{relative}: private/media/checkpoint artifact type is not allowlisted")
            continue
        if path.suffix.lower() not in TEXT_SUFFIXES and path.name not in TEXT_NAMES:
            findings.append(f"{relative}: unexpected file type; classify it before release")
            continue

        text_count += 1
        try:
            text = payload.decode("utf-8")
        except UnicodeDecodeError:
            findings.append(f"{relative}: declared text file is not UTF-8")
            continue
        add_matches(findings, relative, text, ENVIRONMENT_PATTERNS)
        check_workflow_actions(findings, relative, text)

    if findings:
        print("public-tree audit: FAIL")
        print("\n".join(f"- {finding}" for finding in findings))
        raise SystemExit(1)
    print(
        "public-tree audit: PASS "
        f"({len(paths)} commit candidates: {text_count} UTF-8 text, "
        f"{binary_count} allowlisted binary)"
    )


if __name__ == "__main__":
    main()
