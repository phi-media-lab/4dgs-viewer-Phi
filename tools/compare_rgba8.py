#!/usr/bin/env python3
"""Compare two tightly packed raw RGBA8 images and emit a pinned receipt."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import stat
import sys
from pathlib import Path


SCHEMA = "phi.rgba8-comparison.v1"
CHANNEL_NAMES = ("r", "g", "b")


class ComparisonError(ValueError):
    """An invalid input or unsafe filesystem operation."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ComparisonError(message)


def positive_dimension(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("must be an integer") from error
    if parsed <= 0:
        raise argparse.ArgumentTypeError("must be positive")
    return parsed


def finite_float(value: str) -> float:
    try:
        parsed = float(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("must be a number") from error
    if not math.isfinite(parsed):
        raise argparse.ArgumentTypeError("must be finite")
    return parsed


def unit_float(value: str) -> float:
    parsed = finite_float(value)
    if not 0.0 <= parsed <= 1.0:
        raise argparse.ArgumentTypeError("must be in [0, 1]")
    return parsed


def byte_error(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("must be an integer") from error
    if not 0 <= parsed <= 255:
        raise argparse.ArgumentTypeError("must be in [0, 255]")
    return parsed


def nonnegative_error(value: str) -> float:
    parsed = finite_float(value)
    if not 0.0 <= parsed <= 255.0:
        raise argparse.ArgumentTypeError("must be in [0, 255]")
    return parsed


def nonnegative_psnr(value: str) -> float:
    parsed = finite_float(value)
    if parsed < 0.0:
        raise argparse.ArgumentTypeError("must be non-negative")
    return parsed


def checked_rgba8_size(width: int, height: int) -> int:
    require(isinstance(width, int) and not isinstance(width, bool), "width must be an integer")
    require(isinstance(height, int) and not isinstance(height, bool), "height must be an integer")
    require(width > 0 and height > 0, "width and height must be positive")
    pixels = width * height
    # Keep the declared layout and the host allocation bound explicit.
    require(pixels <= sys.maxsize // 4, "RGBA8 image byte count exceeds host limits")
    return pixels * 4


def read_regular_file(path: Path, expected_bytes: int, label: str) -> bytes:
    """Read a leaf path without following a symlink and pin its file identity."""

    try:
        before = path.lstat()
    except OSError as error:
        raise ComparisonError(f"{label} cannot be inspected: {path}: {error}") from error
    require(not stat.S_ISLNK(before.st_mode), f"{label} must not be a symlink: {path}")
    require(stat.S_ISREG(before.st_mode), f"{label} must be a regular file: {path}")
    require(
        before.st_size == expected_bytes,
        f"{label} byte length mismatch: expected {expected_bytes}, got {before.st_size}",
    )

    flags = os.O_RDONLY
    if hasattr(os, "O_BINARY"):
        flags |= os.O_BINARY
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        raise ComparisonError(f"{label} cannot be opened safely: {path}: {error}") from error
    try:
        opened = os.fstat(descriptor)
        require(stat.S_ISREG(opened.st_mode), f"{label} must be a regular file: {path}")
        require(
            (opened.st_dev, opened.st_ino) == (before.st_dev, before.st_ino),
            f"{label} changed while it was being opened: {path}",
        )
        require(
            opened.st_size == expected_bytes,
            f"{label} byte length mismatch: expected {expected_bytes}, got {opened.st_size}",
        )
        chunks: list[bytes] = []
        remaining = expected_bytes
        while remaining:
            chunk = os.read(descriptor, min(remaining, 1024 * 1024))
            require(chunk != b"", f"{label} ended before its declared byte length")
            chunks.append(chunk)
            remaining -= len(chunk)
        require(os.read(descriptor, 1) == b"", f"{label} exceeds its declared byte length")
        after = os.fstat(descriptor)
        require(
            (after.st_size, after.st_mtime_ns) == (opened.st_size, opened.st_mtime_ns),
            f"{label} changed while it was being read: {path}",
        )
        return b"".join(chunks)
    finally:
        os.close(descriptor)


def metric(sum_abs: int, sum_squared: int, maximum: int, sample_count: int) -> dict:
    mean_abs = sum_abs / sample_count
    rmse = math.sqrt(sum_squared / sample_count)
    if sum_squared == 0:
        psnr_db = None
        psnr_infinite = True
    else:
        psnr_db = 20.0 * math.log10(255.0 / rmse)
        psnr_infinite = False
    return {
        "max_abs": maximum,
        "mean_abs": mean_abs,
        "psnr_db": psnr_db,
        "psnr_infinite": psnr_infinite,
        "rmse": rmse,
    }


def compare_payloads(
    golden: bytes,
    actual: bytes,
    *,
    width: int,
    height: int,
    require_rgb_exact: bool = False,
    min_psnr_db: float | None = None,
    max_abs: int | None = None,
    max_mean_abs: float | None = None,
    max_rmse: float | None = None,
    max_changed_pixel_ratio: float | None = None,
) -> dict:
    expected_bytes = checked_rgba8_size(width, height)
    require(len(golden) == expected_bytes, "golden payload byte length mismatch")
    require(len(actual) == expected_bytes, "actual payload byte length mismatch")
    require(isinstance(require_rgb_exact, bool), "require_rgb_exact must be boolean")
    require(
        require_rgb_exact
        or any(
            value is not None
            for value in (
                min_psnr_db,
                max_abs,
                max_mean_abs,
                max_rmse,
                max_changed_pixel_ratio,
            )
        ),
        "at least one RGB acceptance threshold is required",
    )
    if min_psnr_db is not None:
        require(math.isfinite(min_psnr_db) and min_psnr_db >= 0.0, "min_psnr_db must be finite and non-negative")
    if max_abs is not None:
        require(isinstance(max_abs, int) and not isinstance(max_abs, bool) and 0 <= max_abs <= 255, "max_abs must be an integer in [0, 255]")
    for name, value in (("max_mean_abs", max_mean_abs), ("max_rmse", max_rmse)):
        if value is not None:
            require(math.isfinite(value) and 0.0 <= value <= 255.0, f"{name} must be finite and in [0, 255]")
    if max_changed_pixel_ratio is not None:
        require(math.isfinite(max_changed_pixel_ratio) and 0.0 <= max_changed_pixel_ratio <= 1.0, "max_changed_pixel_ratio must be finite and in [0, 1]")

    channel_abs = [0, 0, 0]
    channel_squared = [0, 0, 0]
    channel_maximum = [0, 0, 0]
    changed_pixels = 0
    golden_non_opaque = 0
    actual_non_opaque = 0
    pixel_count = width * height

    for offset in range(0, expected_bytes, 4):
        pixel_changed = False
        for channel in range(3):
            difference = abs(golden[offset + channel] - actual[offset + channel])
            channel_abs[channel] += difference
            channel_squared[channel] += difference * difference
            channel_maximum[channel] = max(channel_maximum[channel], difference)
            pixel_changed |= difference != 0
        changed_pixels += pixel_changed
        golden_non_opaque += golden[offset + 3] != 255
        actual_non_opaque += actual[offset + 3] != 255

    total_abs = sum(channel_abs)
    total_squared = sum(channel_squared)
    total_maximum = max(channel_maximum)
    aggregate = metric(total_abs, total_squared, total_maximum, pixel_count * 3)
    channels = {
        name: metric(channel_abs[index], channel_squared[index], channel_maximum[index], pixel_count)
        for index, name in enumerate(CHANNEL_NAMES)
    }
    rgb_exact = total_squared == 0
    changed_pixel_ratio = changed_pixels / pixel_count

    checks = [
        {
            "check": "golden-alpha-opaque",
            "passed": golden_non_opaque == 0,
        },
        {
            "check": "actual-alpha-opaque",
            "passed": actual_non_opaque == 0,
        },
    ]
    if require_rgb_exact:
        checks.append({"check": "rgb-exact", "passed": rgb_exact})
    if min_psnr_db is not None:
        checks.append(
            {
                "check": "minimum-psnr-db",
                "passed": aggregate["psnr_infinite"] or aggregate["psnr_db"] >= min_psnr_db,
            }
        )
    if max_abs is not None:
        checks.append({"check": "maximum-absolute-error", "passed": aggregate["max_abs"] <= max_abs})
    if max_mean_abs is not None:
        checks.append({"check": "maximum-mean-absolute-error", "passed": aggregate["mean_abs"] <= max_mean_abs})
    if max_rmse is not None:
        checks.append({"check": "maximum-rmse", "passed": aggregate["rmse"] <= max_rmse})
    if max_changed_pixel_ratio is not None:
        checks.append(
            {
                "check": "maximum-changed-pixel-ratio",
                "passed": changed_pixel_ratio <= max_changed_pixel_ratio,
            }
        )

    return {
        "alpha": {
            "actual_non_opaque_pixels": actual_non_opaque,
            "actual_opaque": actual_non_opaque == 0,
            "golden_non_opaque_pixels": golden_non_opaque,
            "golden_opaque": golden_non_opaque == 0,
        },
        "checks": checks,
        "height": height,
        "inputs": {
            "actual": {
                "bytes": len(actual),
                "sha256": hashlib.sha256(actual).hexdigest(),
            },
            "golden": {
                "bytes": len(golden),
                "sha256": hashlib.sha256(golden).hexdigest(),
            },
        },
        "metrics": {
            "changed_pixel_ratio": changed_pixel_ratio,
            "changed_pixels": changed_pixels,
            "channels": channels,
            "rgb": aggregate,
            "rgb_exact": rgb_exact,
        },
        "pixel_count": pixel_count,
        "schema": SCHEMA,
        "status": "PASS" if all(check["passed"] for check in checks) else "FAIL",
        "thresholds": {
            "max_abs": max_abs,
            "max_changed_pixel_ratio": max_changed_pixel_ratio,
            "max_mean_abs": max_mean_abs,
            "max_rmse": max_rmse,
            "min_psnr_db": min_psnr_db,
            "require_rgb_exact": require_rgb_exact,
        },
        "width": width,
    }


def compare_files(
    golden_path: Path,
    actual_path: Path,
    *,
    width: int,
    height: int,
    **thresholds: object,
) -> dict:
    expected_bytes = checked_rgba8_size(width, height)
    golden = read_regular_file(golden_path, expected_bytes, "golden")
    actual = read_regular_file(actual_path, expected_bytes, "actual")
    return compare_payloads(golden, actual, width=width, height=height, **thresholds)


def canonical_json_bytes(receipt: dict) -> bytes:
    return (
        json.dumps(
            receipt,
            allow_nan=False,
            ensure_ascii=False,
            separators=(",", ":"),
            sort_keys=True,
        )
        + "\n"
    ).encode("utf-8")


def write_exclusive(path: Path, payload: bytes) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_BINARY"):
        flags |= os.O_BINARY
    try:
        descriptor = os.open(path, flags, 0o644)
    except OSError as error:
        raise ComparisonError(f"output must be a new file: {path}: {error}") from error
    try:
        view = memoryview(payload)
        while view:
            written = os.write(descriptor, view)
            require(written > 0, f"output write made no progress: {path}")
            view = view[written:]
    finally:
        os.close(descriptor)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("golden", type=Path, help="golden tightly packed raw RGBA8 file")
    parser.add_argument("actual", type=Path, help="actual tightly packed raw RGBA8 file")
    parser.add_argument("--width", type=positive_dimension, required=True)
    parser.add_argument("--height", type=positive_dimension, required=True)
    parser.add_argument("--output", type=Path, help="new receipt path; existing paths are never overwritten")
    parser.add_argument("--require-rgb-exact", action="store_true")
    parser.add_argument("--min-psnr-db", type=nonnegative_psnr)
    parser.add_argument("--max-abs", type=byte_error)
    parser.add_argument("--max-mean-abs", type=nonnegative_error)
    parser.add_argument("--max-rmse", type=nonnegative_error)
    parser.add_argument("--max-changed-pixel-ratio", type=unit_float)
    return parser


def run(arguments: list[str] | None = None) -> int:
    args = build_parser().parse_args(arguments)
    try:
        receipt = compare_files(
            args.golden,
            args.actual,
            width=args.width,
            height=args.height,
            require_rgb_exact=args.require_rgb_exact,
            min_psnr_db=args.min_psnr_db,
            max_abs=args.max_abs,
            max_mean_abs=args.max_mean_abs,
            max_rmse=args.max_rmse,
            max_changed_pixel_ratio=args.max_changed_pixel_ratio,
        )
        encoded = canonical_json_bytes(receipt)
        if args.output is not None:
            write_exclusive(args.output, encoded)
        sys.stdout.buffer.write(encoded)
        return 0 if receipt["status"] == "PASS" else 1
    except (ComparisonError, OSError) as error:
        print(f"RGBA8 comparison error: {error}", file=sys.stderr)
        return 2


def main() -> None:
    raise SystemExit(run())


if __name__ == "__main__":
    main()
