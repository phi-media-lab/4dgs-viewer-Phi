#!/usr/bin/env python3
"""Generate deterministic 4DGS assets without external models or datasets."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import random
import struct
import tempfile
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MAGIC = b"4DGSWG01"
VERSION = 1
HEADER_BYTES = 64
RECORD_BYTES = 80
SH3_RECORD_BYTES = 92
SH_C0 = 0.28209479177387814
REFERENCE_TIME = 0.5084746

# The weights intentionally sum to the public fixture's default count. Keeping
# the layout as named regions makes smaller test fixtures preserve every
# diagnostic instead of degrading into the first N points of a large scene.
CALIBRATION_GROUP_WEIGHTS = (
    ("asymmetric-frame", 500),
    ("title", 350),
    ("camera-axes", 650),
    ("rgb-swatches", 400),
    ("depth-order", 900),
    ("timeline", 800),
    ("sh-view", 496),
)


@dataclass(frozen=True)
class Anchor:
    x: float
    y: float
    z: float
    color: tuple[float, float, float]
    scale: tuple[float, float, float] = (0.007, 0.007, 0.0035)
    angle: float = 0.0
    time_center: float = 0.5
    opacity: float = 0.9
    velocity: tuple[float, float, float] = (0.0, 0.0, 0.0)
    raw_gate: float = 1.0
    raw_duration: float = 0.0


FONT_5X7 = {
    " ": ("00000",) * 7,
    "4": ("10010", "10010", "10010", "11111", "00010", "00010", "00010"),
    "D": ("11110", "10001", "10001", "10001", "10001", "10001", "11110"),
    "E": ("11111", "10000", "10000", "11110", "10000", "10000", "11111"),
    "F": ("11111", "10000", "10000", "11110", "10000", "10000", "10000"),
    "G": ("01111", "10000", "10000", "10111", "10001", "10001", "01111"),
    "H": ("10001", "10001", "10001", "11111", "10001", "10001", "10001"),
    "M": ("10001", "11011", "10101", "10101", "10001", "10001", "10001"),
    "N": ("10001", "11001", "10101", "10011", "10001", "10001", "10001"),
    "S": ("01111", "10000", "10000", "01110", "00001", "00001", "11110"),
    "T": ("11111", "00100", "00100", "00100", "00100", "00100", "00100"),
    "X": ("10001", "01010", "00100", "00100", "00100", "01010", "10001"),
    "Y": ("10001", "01010", "00100", "00100", "00100", "00100", "00100"),
    "Z": ("11111", "00010", "00100", "00100", "01000", "10000", "11111"),
}


def logit(value: float) -> float:
    value = min(max(value, 1e-6), 1.0 - 1e-6)
    return math.log(value / (1.0 - value))


def sh0(rgb: tuple[float, float, float]) -> tuple[float, float, float]:
    return tuple((channel - 0.5) / SH_C0 for channel in rgb)


def record(
    mean: tuple[float, float, float],
    scale: tuple[float, float, float],
    color: tuple[float, float, float],
    *,
    time_center: float = 0.5,
    opacity: float = 0.85,
    rotation_xyzw: tuple[float, float, float, float] = (0.0, 0.0, 0.0, 1.0),
    velocity: tuple[float, float, float] = (0.0, 0.0, 0.0),
    raw_gate: float = 1.0,
    raw_duration: float = 0.0,
) -> tuple[float, ...]:
    return (
        *mean,
        time_center,
        *(math.log(max(axis, 1e-6)) for axis in scale),
        logit(opacity),
        *rotation_xyzw,
        *velocity,
        raw_gate,
        *sh0(color),
        raw_duration,
    )


def encode_geometry(records: list[tuple[float, ...]]) -> bytes:
    payload = b"".join(struct.pack("<20f", *item) for item in records)
    header = struct.pack(
        "<8sIIIIQQQQQ",
        MAGIC,
        VERSION,
        HEADER_BYTES,
        len(records),
        RECORD_BYTES,
        HEADER_BYTES,
        len(payload),
        0,
        0,
        0,
    )
    assert len(header) == HEADER_BYTES
    return header + payload


def calibration_group_counts(count: int) -> dict[str, int]:
    """Allocate an arbitrary count without dropping a calibration region."""

    total_weight = sum(weight for _, weight in CALIBRATION_GROUP_WEIGHTS)
    result = {
        name: count * weight // total_weight
        for name, weight in CALIBRATION_GROUP_WEIGHTS
    }
    remainder = count - sum(result.values())
    fractions = sorted(
        (
            (count * weight % total_weight, -index, name)
            for index, (name, weight) in enumerate(CALIBRATION_GROUP_WEIGHTS)
        ),
        reverse=True,
    )
    for _, _, name in fractions[:remainder]:
        result[name] += 1
    return result


def calibration_group_range(count: int, target: str) -> range:
    groups = calibration_group_counts(count)
    start = 0
    for name, _ in CALIBRATION_GROUP_WEIGHTS:
        end = start + groups[name]
        if name == target:
            return range(start, end)
        start = end
    raise KeyError(target)


def encode_sh3(records: list[tuple[float, ...]], seed: int) -> bytes:
    """Encode deliberate view-dependent probes, leaving other regions exact SH0.

    Only the three neutral targets in the SH region receive non-zero terms.
    Their l=1, l=2 and l=3 coefficients exercise basis ordering and signs while
    making an orbit visibly change color. ``seed`` participates in a tiny,
    deterministic amplitude adjustment so changing the fixture seed changes
    both payloads without introducing visual noise.
    """

    count = len(records)
    sh_region = calibration_group_range(count, "sh-view")
    seed_scale = 0.98 + 0.04 * random.Random(seed ^ 0x534833).random()
    output = bytearray()
    for index, item in enumerate(records):
        coefficients = [[0.0, 0.0, 0.0] for _ in range(15)]
        # A neutral SH0 value identifies the three response targets; the "SH"
        # glyph shares the region but remains view-invariant for comparison.
        neutral_sh0 = all(abs(item[16 + channel]) < 1e-12 for channel in range(3))
        if index in sh_region and neutral_sh0:
            y = item[1]
            if y < -0.18:
                # Top target: primarily horizontal view response.
                coefficients[2] = [-0.62, 0.16, 0.62]
                coefficients[7] = [0.16, -0.10, -0.06]
                coefficients[14] = [0.10, 0.02, -0.10]
            elif y < 0.13:
                # Middle target: primarily vertical view response.
                coefficients[0] = [0.18, -0.62, 0.44]
                coefficients[3] = [-0.10, 0.15, -0.05]
                coefficients[8] = [0.08, -0.04, -0.08]
            else:
                # Bottom target: forward/depth response at the fixed camera.
                coefficients[1] = [0.38, 0.08, -0.42]
                coefficients[5] = [0.10, -0.08, 0.12]
                coefficients[11] = [-0.06, 0.08, -0.04]
        for coefficient in coefficients:
            for value in coefficient:
                output.extend(struct.pack("<e", value * seed_scale))
        output.extend(b"\0\0")
    assert len(output) == count * SH3_RECORD_BYTES
    return bytes(output)


def fixed_camera() -> dict[str, object]:
    return {
        "world_to_camera_row_major": [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
        "intrinsics": [500.0, 500.0, 320.0, 180.0],
        "source_size": [640, 360],
        "near": 0.05,
        "far": 100.0,
    }


def manifest(
    name: str,
    geometry: bytes,
    count: int,
    *,
    appearance: bytes | None,
    seed: int,
) -> dict:
    color = "raw-sh3" if appearance is not None else "raw-sh0"
    if appearance is None:
        purpose: dict[str, object] = {
            "id": "loader-smoke-test",
            "description": "three static SH0 Gaussians for format and loader checks",
        }
    else:
        groups = calibration_group_counts(count)
        purpose = {
            "id": "4d-calibration-target",
            "description": "analytic non-artistic target for projection, depth, time and SH checks",
            "reference_time": REFERENCE_TIME,
            "regions": [
                {
                    "id": "asymmetric-frame",
                    "gaussian_count": groups["asymmetric-frame"],
                    "invariant": "image orientation, principal point and aspect-preserving camera fit",
                },
                {
                    "id": "title",
                    "gaussian_count": groups["title"],
                    "invariant": "small-footprint covariance, alpha coverage and text legibility",
                },
                {
                    "id": "camera-axes",
                    "gaussian_count": groups["camera-axes"],
                    "invariant": "RGB axis orientation and anisotropic covariance rotation",
                },
                {
                    "id": "rgb-swatches",
                    "gaussian_count": groups["rgb-swatches"],
                    "invariant": "SH0 RGB channel order and display-space color handling",
                },
                {
                    "id": "depth-order",
                    "gaussian_count": groups["depth-order"],
                    "invariant": "front-to-back sorting and alpha compositing of overlapping layers",
                },
                {
                    "id": "timeline",
                    "gaussian_count": groups["timeline"],
                    "invariant": "explicit linear velocity, temporal gate and normalized time playback",
                },
                {
                    "id": "sh-view",
                    "gaussian_count": groups["sh-view"],
                    "invariant": "SH coefficient order, basis signs and camera-dependent color",
                },
            ],
        }
    result: dict[str, object] = {
        "schema": "phi.4dgs.explicit.v1",
        "version": VERSION,
        "name": name,
        "gaussian_count": count,
        "record_stride": RECORD_BYTES,
        "binary": {
            "uri": "gaussians.bin",
            "bytes": len(geometry),
            "sha256": hashlib.sha256(geometry).hexdigest(),
        },
        "time": {
            "domain": [0, 1],
            "initial": REFERENCE_TIME,
            "max_duration": 1.2,
            "units": "normalized",
        },
        "representation": {
            "velocity": "explicit-linear",
            "rotation": "raw-xyzw",
            "scale": "raw-log",
            "opacity": "raw-logit",
            "gate": "raw-logit-times-20",
            "duration": "raw-logit-max-duration-over-6",
            "color": color,
        },
        "policy": {
            "temporal_threshold": 0.002,
            "alpha_min": 1.0 / 255.0,
            "low_pass": 0.3,
        },
        "render": {
            "working_space": "display-srgb",
            "background": [0.018, 0.018, 0.018, 1.0],
        },
        "camera": {"fixed": fixed_camera()},
        "provenance": {
            "kind": "procedural-synthetic",
            "generator": "tools/generate_synthetic_asset.py",
            "generator_version": 3,
            "seed": seed,
            "gaussian_count": count,
            "numeric_recipe": "python-f64-packed-f32-calibration-v2",
            "purpose": purpose,
            "external_models": [],
            "external_datasets": [],
        },
    }
    if appearance is not None:
        result["appearance"] = {
            "uri": "sh3.f16",
            "bytes": len(appearance),
            "sha256": hashlib.sha256(appearance).hexdigest(),
            "degree": 3,
            "coefficients": 15,
            "channels": 3,
            "encoding": "float16-le-padded46",
            "record_stride": SH3_RECORD_BYTES,
        }
    return result


def minimal_records() -> list[tuple[float, ...]]:
    return [
        record((-0.42, -0.18, 3.0), (0.22, 0.10, 0.06), (0.95, 0.32, 0.16)),
        record((0.35, -0.12, 3.4), (0.16, 0.24, 0.05), (0.12, 0.68, 0.95)),
        record((0.02, 0.34, 3.2), (0.21, 0.09, 0.05), (0.92, 0.78, 0.18)),
    ]


def quaternion_z(angle: float) -> tuple[float, float, float, float]:
    return (0.0, 0.0, math.sin(0.5 * angle), math.cos(0.5 * angle))


def stroke_anchors(
    start: tuple[float, float],
    end: tuple[float, float],
    steps: int,
    *,
    z: float,
    color: tuple[float, float, float],
    width: float = 0.0045,
    opacity: float = 0.9,
    dashed: bool = False,
) -> list[Anchor]:
    dx = end[0] - start[0]
    dy = end[1] - start[1]
    angle = math.atan2(dy, dx)
    length_scale = max(0.009, math.hypot(dx, dy) / max(steps, 1) * 1.4)
    output = []
    for index in range(steps):
        phase = (index + 0.5) / steps
        if dashed and int(phase * 14.0) % 2:
            continue
        output.append(
            Anchor(
                start[0] + dx * phase,
                start[1] + dy * phase,
                z,
                color,
                (length_scale, width, 0.0035),
                angle,
                opacity=opacity,
            )
        )
    return output


def glyph_anchors(
    text: str,
    origin: tuple[float, float],
    cell: float,
    *,
    z: float,
    color: tuple[float, float, float],
    opacity: float = 0.94,
) -> list[Anchor]:
    output = []
    cursor = origin[0]
    for character in text:
        bitmap = FONT_5X7[character]
        for row, pixels in enumerate(bitmap):
            for column, pixel in enumerate(pixels):
                if pixel == "1":
                    output.append(
                        Anchor(
                            cursor + column * cell,
                            origin[1] + row * cell,
                            z,
                            color,
                            (cell * 0.27, cell * 0.27, 0.0035),
                            opacity=opacity,
                        )
                    )
        cursor += 6.0 * cell
    return output


def anchor_records(
    anchors: list[Anchor],
    count: int,
    rng: random.Random,
    *,
    jitter: float = 0.0018,
) -> list[tuple[float, ...]]:
    if count == 0:
        return []
    if not anchors:
        raise ValueError("cannot sample an empty anchor list")
    output = []
    for index in range(count):
        if count < len(anchors):
            anchor_index = min(len(anchors) - 1, index * len(anchors) // count)
        else:
            anchor_index = index % len(anchors)
        anchor = anchors[anchor_index]
        repeat = index // len(anchors)
        spread = jitter if repeat else jitter * 0.35
        scale_factor = rng.uniform(0.94, 1.06)
        output.append(
            record(
                (
                    anchor.x + rng.uniform(-spread, spread),
                    anchor.y + rng.uniform(-spread, spread),
                    anchor.z + rng.uniform(-0.0008, 0.0008),
                ),
                tuple(axis * scale_factor for axis in anchor.scale),
                anchor.color,
                time_center=anchor.time_center,
                opacity=anchor.opacity,
                rotation_xyzw=quaternion_z(anchor.angle),
                velocity=anchor.velocity,
                raw_gate=anchor.raw_gate,
                raw_duration=anchor.raw_duration,
            )
        )
    return output


def asymmetric_frame_records(count: int, rng: random.Random) -> list[tuple[float, ...]]:
    gray = (0.34, 0.37, 0.40)
    anchors = []
    anchors += stroke_anchors((-1.50, -0.88), (1.48, -0.88), 160, z=3.72, color=gray)
    anchors += stroke_anchors((-1.50, -0.88), (-1.50, 0.86), 100, z=3.72, color=gray)
    anchors += stroke_anchors(
        (1.50, -0.18), (1.50, 0.86), 80, z=3.72, color=gray, dashed=True
    )
    anchors += stroke_anchors(
        (-0.30, 0.88), (1.50, 0.88), 100, z=3.72, color=gray, dashed=True
    )
    # A solid cyan corner and a magenta cut at the opposite corner make flips
    # immediately visible without a UI overlay.
    for row in range(8):
        for column in range(8):
            anchors.append(
                Anchor(
                    -1.48 + column * 0.012,
                    -0.86 + row * 0.012,
                    3.62,
                    (0.05, 0.90, 0.94),
                )
            )
    anchors += stroke_anchors(
        (1.34, 0.88), (1.50, 0.72), 30, z=3.60, color=(0.96, 0.12, 0.62), width=0.006
    )
    return anchor_records(anchors, count, rng)


def title_records(count: int, rng: random.Random) -> list[tuple[float, ...]]:
    anchors = glyph_anchors(
        "4DGS TEST",
        (-1.18, -0.79),
        0.026,
        z=3.08,
        color=(0.88, 0.92, 0.96),
    )
    anchors += stroke_anchors(
        (-1.28, -0.58),
        (0.18, -0.58),
        100,
        z=3.14,
        color=(0.20, 0.66, 0.92),
        width=0.0035,
    )
    return anchor_records(anchors, count, rng, jitter=0.0014)


def axis_records(count: int, rng: random.Random) -> list[tuple[float, ...]]:
    origin = (-1.06, 0.06)
    red = (0.98, 0.08, 0.07)
    green = (0.08, 0.92, 0.18)
    blue = (0.08, 0.32, 1.00)
    anchors = []
    anchors += stroke_anchors(origin, (-0.42, 0.06), 90, z=3.02, color=red, width=0.006)
    anchors += stroke_anchors(
        (-0.50, -0.01), (-0.40, 0.06), 24, z=2.98, color=red, width=0.006
    )
    anchors += stroke_anchors(
        (-0.50, 0.13), (-0.40, 0.06), 24, z=2.98, color=red, width=0.006
    )
    anchors += glyph_anchors("X", (-0.34, -0.02), 0.022, z=2.94, color=red)

    anchors += stroke_anchors(
        origin, (-1.06, 0.60), 82, z=3.02, color=green, width=0.006
    )
    anchors += stroke_anchors(
        (-1.13, 0.51), (-1.06, 0.62), 24, z=2.98, color=green, width=0.006
    )
    anchors += stroke_anchors(
        (-0.99, 0.51), (-1.06, 0.62), 24, z=2.98, color=green, width=0.006
    )
    anchors += glyph_anchors("Y", (-1.12, 0.66), 0.022, z=2.94, color=green)

    # The Z stem also recedes in world Z; orbiting exposes its true depth.
    for index in range(72):
        phase = (index + 0.5) / 72.0
        anchors.append(
            Anchor(
                origin[0] + 0.30 * phase,
                origin[1] - 0.25 * phase,
                3.00 + 0.62 * phase,
                blue,
                (0.012, 0.0055, 0.0035),
                math.atan2(-0.25, 0.30),
                opacity=0.94,
            )
        )
    anchors += glyph_anchors("Z", (-0.72, -0.28), 0.022, z=3.54, color=blue)
    for index in range(48):
        angle = math.tau * index / 48.0
        anchors.append(
            Anchor(
                origin[0] + 0.055 * math.cos(angle),
                origin[1] + 0.055 * math.sin(angle),
                2.92,
                (0.95, 0.95, 0.95),
                (0.006, 0.006, 0.0035),
            )
        )
    return anchor_records(anchors, count, rng, jitter=0.0015)


def swatch_records(count: int, rng: random.Random) -> list[tuple[float, ...]]:
    colors = (
        (0.98, 0.06, 0.05),
        (0.05, 0.92, 0.15),
        (0.06, 0.28, 1.00),
        (0.92, 0.92, 0.92),
    )
    output = []
    per_swatch = [
        sum(1 for index in range(count) if index % len(colors) == slot)
        for slot in range(len(colors))
    ]
    for index in range(count):
        swatch = index % len(colors)
        local = index // len(colors)
        columns = max(1, math.ceil(math.sqrt(per_swatch[swatch] * 0.17 / 0.15)))
        rows = max(1, math.ceil(per_swatch[swatch] / columns))
        center_x = -1.25 + 0.245 * swatch
        x = center_x + ((local % columns + 0.5) / columns - 0.5) * 0.17
        y = -0.43 + ((local // columns + 0.5) / rows - 0.5) * 0.15
        output.append(
            record(
                (
                    x + rng.uniform(-0.0005, 0.0005),
                    y + rng.uniform(-0.0005, 0.0005),
                    3.28 + 0.015 * swatch,
                ),
                (0.010, 0.010, 0.004),
                colors[swatch],
                opacity=0.94,
            )
        )
    return output


def rotated_card_point(
    *,
    center: tuple[float, float],
    size: tuple[float, float],
    z: float,
    angle: float,
    z_slope: float,
    local_index: int,
    local_count: int,
) -> tuple[float, float, float, float, float]:
    columns = max(1, math.ceil(math.sqrt(local_count * size[0] / size[1])))
    rows = max(1, math.ceil(local_count / columns))
    u = ((local_index % columns + 0.5) / columns - 0.5) * size[0]
    v = ((local_index // columns + 0.5) / rows - 0.5) * size[1]
    cosine = math.cos(angle)
    sine = math.sin(angle)
    return (
        center[0] + u * cosine - v * sine,
        center[1] + u * sine + v * cosine,
        z + z_slope * u,
        u,
        v,
    )


def depth_order_records(count: int, rng: random.Random) -> list[tuple[float, ...]]:
    labels_count = min(count // 6, 150)
    fill_count = count - labels_count
    cards = (
        ((0.28, -0.10), (0.78, 0.48), 3.68, -0.06, 0.18, (0.08, 0.30, 0.96)),
        ((0.22, 0.05), (0.63, 0.38), 3.22, 0.05, -0.15, (0.06, 0.78, 0.34)),
        ((0.05, 0.18), (0.49, 0.30), 2.76, -0.08, 0.12, (0.98, 0.38, 0.06)),
    )
    output = []
    per_card = [
        sum(1 for index in range(fill_count) if index % len(cards) == slot)
        for slot in range(len(cards))
    ]
    for index in range(fill_count):
        # Deliberately interleave far/mid/near records. Correct appearance must
        # therefore come from the renderer's depth sort, never input order.
        card = index % len(cards)
        center, size, z, angle, z_slope, base = cards[card]
        x, y, point_z, u, v = rotated_card_point(
            center=center,
            size=size,
            z=z,
            angle=angle,
            z_slope=z_slope,
            local_index=index // len(cards),
            local_count=per_card[card],
        )
        checker = (int((u / size[0] + 0.5) * 8) + int((v / size[1] + 0.5) * 5)) & 1
        factor = 0.88 if checker else 1.0
        output.append(
            record(
                (x, y, point_z),
                (0.0125, 0.0125, 0.004),
                tuple(channel * factor for channel in base),
                opacity=0.86,
                rotation_xyzw=quaternion_z(angle),
            )
        )

    label_anchors = []
    label_anchors += glyph_anchors(
        "F", (0.43, -0.27), 0.025, z=3.57, color=(0.94, 0.96, 1.0)
    )
    label_anchors += glyph_anchors(
        "M", (0.32, -0.04), 0.025, z=3.12, color=(0.95, 1.0, 0.96)
    )
    label_anchors += glyph_anchors(
        "N", (-0.04, 0.10), 0.025, z=2.66, color=(1.0, 0.95, 0.90)
    )
    output += anchor_records(label_anchors, labels_count, rng, jitter=0.0012)
    assert len(output) == count
    return output


def timeline_records(count: int, rng: random.Random) -> list[tuple[float, ...]]:
    rail_count = count * 3 // 10
    mover_count = count * 4 // 10
    pulse_count = count - rail_count - mover_count

    rail = stroke_anchors(
        (-0.52, 0.69), (1.28, 0.69), 150, z=3.58, color=(0.34, 0.37, 0.40), width=0.0035
    )
    for x in (-0.45, 0.40, 1.25):
        rail += stroke_anchors(
            (x, 0.64), (x, 0.74), 16, z=3.54, color=(0.62, 0.65, 0.68), width=0.0035
        )
    rail += glyph_anchors("T", (-0.69, 0.60), 0.022, z=3.36, color=(0.82, 0.85, 0.88))
    output = anchor_records(rail, rail_count, rng, jitter=0.0014)

    # At t=0.5 the yellow diamond is centered over the middle tick. Across the
    # normalized domain it travels from the first to the last tick.
    for _ in range(mover_count):
        while True:
            u = rng.uniform(-0.085, 0.085)
            v = rng.uniform(-0.085, 0.085)
            if abs(u) + abs(v) <= 0.085:
                break
        output.append(
            record(
                (0.40 + u, 0.69 + v, 2.82),
                (0.008, 0.008, 0.0035),
                (1.0, 0.78, 0.04),
                time_center=0.5,
                opacity=0.94,
                velocity=(1.70, 0.0, 0.0),
                raw_gate=1.0,
            )
        )

    pulse_centers = ((-0.45, 0.20), (0.40, 0.50), (1.25, 0.80))
    pulse_colors = ((0.94, 0.13, 0.66), (0.02, 0.90, 0.94), (0.76, 0.34, 1.0))
    for index in range(pulse_count):
        pulse = index % len(pulse_centers)
        center_x, time_center = pulse_centers[pulse]
        angle = (
            math.tau
            * ((index // len(pulse_centers)) + 0.5)
            / max(1, math.ceil(pulse_count / 3))
        )
        radius = 0.075 + rng.uniform(-0.008, 0.008)
        output.append(
            record(
                (
                    center_x + radius * math.cos(angle),
                    0.52 + radius * math.sin(angle),
                    3.02,
                ),
                (0.007, 0.007, 0.0035),
                pulse_colors[pulse],
                time_center=time_center,
                opacity=0.95,
                raw_gate=-1.0,
                raw_duration=-0.8,
            )
        )
    assert len(output) == count
    return output


def sh_view_records(count: int, rng: random.Random) -> list[tuple[float, ...]]:
    label_count = count // 5
    label = glyph_anchors("SH", (0.83, -0.72), 0.025, z=3.12, color=(0.86, 0.90, 0.94))
    output = anchor_records(label, label_count, rng, jitter=0.0014)
    centers = ((1.08, -0.34), (1.08, -0.03), (1.08, 0.28))
    target_count = count - label_count
    per_target = [
        sum(1 for index in range(target_count) if index % len(centers) == slot)
        for slot in range(len(centers))
    ]
    golden_angle = math.pi * (3.0 - math.sqrt(5.0))
    for index in range(target_count):
        target = index % len(centers)
        local = index // len(centers)
        radius = 0.115 * math.sqrt((local + 0.5) / per_target[target])
        angle = golden_angle * local
        output.append(
            record(
                (
                    centers[target][0] + radius * math.cos(angle),
                    centers[target][1] + radius * math.sin(angle),
                    3.04 + 0.08 * target,
                ),
                (0.009, 0.009, 0.004),
                (0.5, 0.5, 0.5),
                opacity=0.90,
            )
        )
    assert len(output) == count
    return output


def motion_records(count: int, seed: int) -> list[tuple[float, ...]]:
    """Build the analytic 4D calibration target in stable semantic regions."""

    rng = random.Random(seed)
    groups = calibration_group_counts(count)
    output = []
    output += asymmetric_frame_records(groups["asymmetric-frame"], rng)
    output += title_records(groups["title"], rng)
    output += axis_records(groups["camera-axes"], rng)
    output += swatch_records(groups["rgb-swatches"], rng)
    output += depth_order_records(groups["depth-order"], rng)
    output += timeline_records(groups["timeline"], rng)
    output += sh_view_records(groups["sh-view"], rng)
    assert len(output) == count
    return output


def atomic_write(path: Path, payload: bytes) -> None:
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            dir=path.parent,
            prefix=f".{path.name}.",
            suffix=".tmp",
            delete=False,
        ) as handle:
            temporary = Path(handle.name)
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        if temporary is not None and temporary.exists():
            temporary.unlink()


def write_asset(
    root: Path, name: str, records: list[tuple[float, ...]], *, sh3: bool, seed: int
) -> None:
    destination = root / name
    destination.mkdir(parents=True, exist_ok=True)
    geometry = encode_geometry(records)
    appearance = encode_sh3(records, seed) if sh3 else None
    atomic_write(destination / "gaussians.bin", geometry)
    if appearance is not None:
        atomic_write(destination / "sh3.f16", appearance)
    rendered = json.dumps(
        manifest(name, geometry, len(records), appearance=appearance, seed=seed),
        indent=2,
    )
    atomic_write(destination / "manifest.json", (rendered + "\n").encode())
    if appearance is None and (destination / "sh3.f16").exists():
        (destination / "sh3.f16").unlink()
    print(f"generated {name}: {len(records)} Gaussians in {destination}")


def generate_all(root: Path, motion_count: int, seed: int) -> None:
    write_asset(root, "minimal-sh0", minimal_records(), sh3=False, seed=seed)
    write_asset(
        root,
        "synthetic-motion-sh3",
        motion_records(motion_count, seed),
        sh3=True,
        seed=seed,
    )


def check_generated(expected_root: Path, actual_root: Path) -> None:
    generated_files = {
        Path("minimal-sh0/gaussians.bin"),
        Path("minimal-sh0/manifest.json"),
        Path("synthetic-motion-sh3/gaussians.bin"),
        Path("synthetic-motion-sh3/manifest.json"),
        Path("synthetic-motion-sh3/sh3.f16"),
    }
    mismatches = []
    for relative in sorted(generated_files):
        expected = expected_root / relative
        actual = actual_root / relative
        if not expected.is_file():
            mismatches.append(f"missing checked-in file: {relative}")
        elif expected.read_bytes() != actual.read_bytes():
            mismatches.append(f"content differs: {relative}")
    if mismatches:
        raise SystemExit("synthetic asset check failed:\n- " + "\n- ".join(mismatches))
    print("synthetic asset check: PASS")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-root", type=Path, default=ROOT / "examples")
    parser.add_argument("--motion-count", type=int, default=4096)
    parser.add_argument("--seed", type=int, default=20260903)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--check",
        action="store_true",
        help="compare a fresh generation with --output-root",
    )
    mode.add_argument(
        "--force",
        action="store_true",
        help="replace generated files in existing asset directories",
    )
    args = parser.parse_args()
    if args.motion_count < 1:
        parser.error("--motion-count must be positive")
    if args.check:
        with tempfile.TemporaryDirectory(prefix="phi-4dgs-assets-") as directory:
            generated = Path(directory)
            generate_all(generated, args.motion_count, args.seed)
            check_generated(args.output_root, generated)
        return
    existing = [
        args.output_root / name
        for name in ("minimal-sh0", "synthetic-motion-sh3")
        if (args.output_root / name).exists()
    ]
    if existing and not args.force:
        parser.error(
            "asset directories already exist; pass --check or explicitly pass --force"
        )
    generate_all(args.output_root, args.motion_count, args.seed)


if __name__ == "__main__":
    main()
