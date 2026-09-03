#!/usr/bin/env python3
"""Validate the explicit-v1 manifest and every referenced binary byte."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import struct
from pathlib import Path


MAGIC = b"4DGSWG01"
HEADER = struct.Struct("<8sIIIIQQQQQ")
HEADER_BYTES = 64
RECORD = struct.Struct("<20f")
RECORD_BYTES = 80
SH3_RECORD_BYTES = 92
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
SHADER_SAFE_ABS = 1.0e30
F32_MAX = 3.4028234663852886e38
CAMERA_ORTHONORMAL_TOLERANCE = 1.0e-3
LEGACY_INITIAL_TIME = 0.5084746


def is_number(value: object) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def is_shader_number(value: object) -> bool:
    return is_number(value) and abs(value) < SHADER_SAFE_ABS


def is_rigid_world_to_camera(matrix: list[list[object]]) -> bool:
    target = [0.0, 0.0, 0.0, 1.0]
    if any(abs(matrix[3][index] - target[index]) > CAMERA_ORTHONORMAL_TOLERANCE for index in range(4)):
        return False
    for row in range(3):
        for other in range(3):
            dot = sum(matrix[row][column] * matrix[other][column] for column in range(3))
            expected = 1.0 if row == other else 0.0
            if abs(dot - expected) > CAMERA_ORTHONORMAL_TOLERANCE:
                return False
    determinant = (
        matrix[0][0] * (matrix[1][1] * matrix[2][2] - matrix[1][2] * matrix[2][1])
        - matrix[0][1] * (matrix[1][0] * matrix[2][2] - matrix[1][2] * matrix[2][0])
        + matrix[0][2] * (matrix[1][0] * matrix[2][1] - matrix[1][1] * matrix[2][0])
    )
    return abs(determinant - 1.0) <= CAMERA_ORTHONORMAL_TOLERANCE


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def exact_object(
    value: object,
    *,
    required: set[str],
    optional: set[str] | None = None,
    label: str,
) -> dict:
    require(isinstance(value, dict), f"{label} must be an object")
    optional = optional or set()
    missing = required - value.keys()
    unknown = value.keys() - required - optional
    require(not missing, f"{label} missing fields: {sorted(missing)}")
    require(not unknown, f"{label} has unknown fields: {sorted(unknown)}")
    return value


def reject_duplicate_keys(pairs: list[tuple[str, object]]) -> dict:
    result: dict[str, object] = {}
    for key, value in pairs:
        require(key not in result, f"duplicate JSON key: {key}")
        result[key] = value
    return result


def reject_non_standard_constant(token: str) -> None:
    raise ValueError(f"non-standard JSON constant {token}")


def load_strict_json(path: Path) -> object:
    try:
        return json.loads(
            path.read_text(encoding="utf-8"),
            object_pairs_hook=reject_duplicate_keys,
            parse_constant=reject_non_standard_constant,
        )
    except (json.JSONDecodeError, ValueError) as error:
        raise ValueError(f"{path} is not strict JSON: {error}") from error


def relative_payload(asset_dir: Path, raw_uri: object) -> Path:
    require(isinstance(raw_uri, str) and raw_uri, "payload uri must be a non-empty string")
    uri = Path(raw_uri)
    require(not uri.is_absolute(), f"payload uri must be relative: {raw_uri}")
    path = (asset_dir / uri).resolve()
    root = asset_dir.resolve()
    require(path == root or root in path.parents, f"payload uri escapes asset directory: {raw_uri}")
    return path


def read_pinned(
    asset_dir: Path,
    description: object,
    label: str,
    *,
    metadata_fields: set[str] | None = None,
) -> bytes:
    description = exact_object(
        description,
        required={"uri", "bytes", "sha256"},
        optional=metadata_fields,
        label=f"{label} description",
    )
    require(
        isinstance(description["bytes"], int)
        and not isinstance(description["bytes"], bool)
        and description["bytes"] > 0,
        f"{label} bytes must be a positive integer",
    )
    require(
        isinstance(description["sha256"], str) and SHA256_RE.fullmatch(description["sha256"]),
        f"{label} sha256 is malformed",
    )
    path = relative_payload(asset_dir, description.get("uri"))
    payload = path.read_bytes()
    require(description.get("bytes") == len(payload), f"{label} byte count mismatch")
    digest = hashlib.sha256(payload).hexdigest()
    require(description.get("sha256") == digest, f"{label} SHA-256 mismatch")
    return payload


def validate_manifest_shape(manifest: object) -> dict:
    required = {
        "schema",
        "version",
        "name",
        "gaussian_count",
        "record_stride",
        "binary",
        "time",
        "representation",
        "policy",
        "render",
        "camera",
        "provenance",
    }
    manifest = exact_object(
        manifest,
        required=required,
        optional={"appearance"},
        label="manifest",
    )
    require(manifest["schema"] == "phi.4dgs.explicit.v1", "unsupported schema")
    require(manifest["version"] == 1, "version must be 1")
    require(isinstance(manifest["name"], str) and manifest["name"].strip(), "name must be a non-empty string")
    count = manifest["gaussian_count"]
    require(
        isinstance(count, int) and not isinstance(count, bool) and 0 < count <= 0xFFFFFFFF,
        "gaussian_count must be a positive u32",
    )
    require(manifest["record_stride"] == RECORD_BYTES, "record_stride must be 80")
    time = exact_object(
        manifest["time"],
        required={"domain", "max_duration", "units"},
        optional={"initial"},
        label="time",
    )
    require(
        isinstance(time["domain"], list)
        and len(time["domain"]) == 2
        and all(is_number(value) for value in time["domain"])
        and time["domain"] == [0, 1],
        "time domain must be [0, 1]",
    )
    require(time.get("units") == "normalized", "time units must be normalized")
    initial_time = time.get("initial", LEGACY_INITIAL_TIME)
    require(
        is_number(initial_time) and 0 <= initial_time <= 1,
        "time initial must be finite and in [0, 1]",
    )
    require(
        is_shader_number(time["max_duration"]) and time["max_duration"] > 0,
        "max_duration must be positive and shader-safe finite",
    )
    representation = exact_object(
        manifest["representation"],
        required={"velocity", "rotation", "scale", "opacity", "gate", "duration", "color"},
        label="representation",
    )
    expected_representation = {
        "velocity": "explicit-linear",
        "rotation": "raw-xyzw",
        "scale": "raw-log",
        "opacity": "raw-logit",
        "gate": "raw-logit-times-20",
        "duration": "raw-logit-max-duration-over-6",
    }
    for field, expected in expected_representation.items():
        require(representation[field] == expected, f"unsupported {field} representation")
    require(representation.get("color") in {"raw-sh0", "raw-sh3"}, "unsupported color representation")
    policy = exact_object(
        manifest["policy"],
        required={"temporal_threshold", "alpha_min", "low_pass"},
        optional={
            "opacity_compensation",
            "alpha_cap",
            "pixel_alpha_min",
            "transmittance_epsilon",
        },
        label="policy",
    )
    require(
        is_number(policy["temporal_threshold"]) and 0 <= policy["temporal_threshold"] <= 1,
        "temporal_threshold must be finite and in [0, 1]",
    )
    require(is_number(policy["alpha_min"]) and 0 < policy["alpha_min"] < 1, "alpha_min must be finite and in (0, 1)")
    require(
        is_shader_number(policy["low_pass"]) and policy["low_pass"] >= 0,
        "low_pass must be shader-safe finite and non-negative",
    )
    require(
        policy.get("opacity_compensation", "determinant-ratio")
        in {"none", "determinant-ratio"},
        "unsupported opacity_compensation",
    )
    raster_fields = {
        "alpha_cap",
        "pixel_alpha_min",
        "transmittance_epsilon",
    }
    present_raster_fields = raster_fields & policy.keys()
    require(
        not present_raster_fields or present_raster_fields == raster_fields,
        "alpha_cap, pixel_alpha_min, and transmittance_epsilon must be declared together",
    )
    if present_raster_fields:
        alpha_cap = policy["alpha_cap"]
        pixel_alpha_min = policy["pixel_alpha_min"]
        transmittance_epsilon = policy["transmittance_epsilon"]
        require(
            is_number(alpha_cap) and 0 < alpha_cap < 1,
            "alpha_cap must be finite and in (0, 1)",
        )
        require(
            is_number(pixel_alpha_min)
            and 0 < pixel_alpha_min < 1
            and pixel_alpha_min <= alpha_cap,
            "pixel_alpha_min must be finite, in (0, 1), and no greater than alpha_cap",
        )
        require(
            is_number(transmittance_epsilon)
            and 0 < transmittance_epsilon < 1,
            "transmittance_epsilon must be finite and in (0, 1)",
        )
    render = exact_object(
        manifest["render"],
        required={"working_space", "background"},
        optional={"output_transfer"},
        label="render",
    )
    render_pair = (render["working_space"], render.get("output_transfer", "identity"))
    require(
        render_pair in {("display-srgb", "identity"), ("linear-rgb", "srgb")},
        "unsupported render working_space/output_transfer",
    )
    background = render["background"]
    require(
        isinstance(background, list)
        and len(background) == 4
        and all(is_number(value) and 0 <= value <= 1 for value in background),
        "render background must contain four finite values in [0, 1]",
    )
    require(background[3] == 1, "render background alpha must be exactly 1")
    camera = exact_object(manifest["camera"], required={"fixed"}, label="camera")
    fixed = exact_object(
        camera["fixed"],
        required={"world_to_camera_row_major", "intrinsics", "source_size", "near", "far"},
        label="camera.fixed",
    )
    matrix = fixed.get("world_to_camera_row_major")
    require(isinstance(matrix, list) and len(matrix) == 4, "camera matrix must have four rows")
    require(all(isinstance(row, list) and len(row) == 4 for row in matrix), "camera matrix rows must have four values")
    flat_camera = [value for row in matrix for value in row]
    intrinsics = fixed.get("intrinsics")
    require(isinstance(intrinsics, list) and len(intrinsics) == 4, "camera intrinsics must have four values")
    require(all(is_shader_number(value) for value in flat_camera + intrinsics), "camera values must be shader-safe finite numbers")
    require(is_rigid_world_to_camera(matrix), "camera matrix must be a right-handed rigid affine transform")
    require(intrinsics[0] > 0 and intrinsics[1] > 0, "camera focal lengths must be positive")
    source_size = fixed.get("source_size")
    require(isinstance(source_size, list) and len(source_size) == 2, "camera source_size must have two values")
    require(
        all(isinstance(value, int) and not isinstance(value, bool) and 0 < value <= 0xFFFFFFFF for value in source_size),
        "camera source_size must contain positive u32 values",
    )
    near = fixed.get("near")
    far = fixed.get("far")
    require(
        is_shader_number(near) and is_shader_number(far) and 0 < near < far,
        "camera near/far are invalid",
    )
    require(isinstance(manifest["provenance"], dict), "provenance must be an object")
    return manifest


def validate_appearance_description(description: object, count: int) -> dict:
    description = exact_object(
        description,
        required={"uri", "bytes", "sha256", "degree", "coefficients", "channels", "encoding", "record_stride"},
        label="appearance description",
    )
    require(description["degree"] == 3, "appearance degree must be 3")
    require(description["coefficients"] == 15, "appearance coefficients must be 15")
    require(description["channels"] == 3, "appearance channels must be 3")
    require(description["encoding"] == "float16-le-padded46", "unsupported appearance encoding")
    require(description["record_stride"] == SH3_RECORD_BYTES, "appearance record_stride must be 92")
    require(description["bytes"] == count * SH3_RECORD_BYTES, "appearance declared byte count is invalid")
    return description


def validate_geometry(payload: bytes, count: int) -> None:
    require(len(payload) >= HEADER_BYTES, "geometry is shorter than its header")
    magic, version, header_bytes, header_count, stride, offset, body_bytes, r0, r1, r2 = HEADER.unpack_from(payload)
    require(magic == MAGIC, "geometry magic mismatch")
    require(version == 1 and header_bytes == HEADER_BYTES, "geometry header version/size mismatch")
    require(header_count == count and stride == RECORD_BYTES, "geometry count/stride mismatch")
    require(offset == HEADER_BYTES, "geometry payload offset mismatch")
    require(body_bytes == count * RECORD_BYTES, "geometry payload length in header is invalid")
    require(len(payload) == HEADER_BYTES + body_bytes, "geometry file length is invalid")
    require((r0, r1, r2) == (0, 0, 0), "geometry reserved header values must be zero")
    for index in range(count):
        values = RECORD.unpack_from(payload, HEADER_BYTES + index * RECORD_BYTES)
        require(
            all(math.isfinite(value) and abs(value) < SHADER_SAFE_ABS for value in values),
            f"Gaussian {index} contains a non-finite or shader-unsafe scalar",
        )
        quaternion_norm = sum(value * value for value in values[8:12])
        require(
            math.isfinite(quaternion_norm) and 1e-12 < quaternion_norm <= F32_MAX,
            f"Gaussian {index} quaternion is invalid",
        )


def validate_appearance(payload: bytes, count: int) -> None:
    require(len(payload) == count * SH3_RECORD_BYTES, "SH3 sidecar length mismatch")
    for index in range(count):
        base = index * SH3_RECORD_BYTES
        for scalar in range(45):
            value = struct.unpack_from("<e", payload, base + scalar * 2)[0]
            require(math.isfinite(value), f"SH3 record {index} contains a non-finite scalar")
        require(payload[base + 90 : base + 92] == b"\0\0", f"SH3 record {index} padding must be zero")


def validate(path: Path) -> dict[str, object]:
    manifest_path = path.resolve()
    manifest_json = load_strict_json(manifest_path)
    manifest = validate_manifest_shape(manifest_json)
    count = manifest["gaussian_count"]
    require(manifest["binary"]["bytes"] == HEADER_BYTES + count * RECORD_BYTES, "geometry declared byte count is invalid")
    geometry = read_pinned(manifest_path.parent, manifest["binary"], "geometry")
    validate_geometry(geometry, count)
    color = manifest["representation"]["color"]
    if color == "raw-sh3":
        require("appearance" in manifest, "raw-sh3 manifest is missing appearance")
        appearance_description = validate_appearance_description(manifest["appearance"], count)
        appearance = read_pinned(
            manifest_path.parent,
            appearance_description,
            "appearance",
            metadata_fields={"degree", "coefficients", "channels", "encoding", "record_stride"},
        )
        validate_appearance(appearance, count)
    else:
        require("appearance" not in manifest, "raw-sh0 manifest must not contain appearance")
    return {
        "manifest": str(path),
        "name": manifest["name"],
        "gaussian_count": count,
        "color": color,
        "geometry_sha256": manifest["binary"]["sha256"],
        "appearance_sha256": manifest.get("appearance", {}).get("sha256"),
        "status": "PASS",
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifests", nargs="+", type=Path)
    args = parser.parse_args()
    for manifest in args.manifests:
        print(json.dumps(validate(manifest), sort_keys=True))


if __name__ == "__main__":
    main()
