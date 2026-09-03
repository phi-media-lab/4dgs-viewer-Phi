#!/usr/bin/env python3
"""Convert a verified Pixel4DGS AssetBundle v1 into Phi explicit-v1.

The bridge intentionally accepts the inference-only AssetBundle contract, not a
training checkpoint. It verifies every source byte, maps seconds to normalized
time, preserves the physical temporal Gaussian, reorders gsplat quaternions,
and records the one lossy operation: SH3 f32 to binary16.
"""

from __future__ import annotations

import argparse
import ctypes
import errno
import hashlib
import json
import math
import mmap
import os
import shutil
import stat
import struct
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any


P2G_BUNDLE_SCHEMA = "p2g.asset_bundle.v1"
P2G_MANIFEST_SCHEMA = "p2g.asset_bundle_manifest.v1"
P2G_MODEL_SCHEMA = "p2g.asset_model.v1"
P2G_EQUATION_VERSION = "p2g.linear_motion_gaussian_gate.v1"
P2G_CAMERA_PATH_SCHEMA = "p2g.camera_path.v1"
P2G_RENDERER_ABI = "p2g.gsplat_rocm.v1"

PHI_SCHEMA = "phi.4dgs.explicit.v1"
MAGIC = b"4DGSWG01"
HEADER = struct.Struct("<8sIIIIQQQQQ")
RECORD = struct.Struct("<20f")
SH3_RECORD = struct.Struct("<45e2x")
HEADER_BYTES = 64
RECORD_BYTES = 80
SH3_RECORD_BYTES = 92
SHADER_SAFE_ABS = 1.0e30
PLAYER_GATE_SCALE = 20.0
PLAYER_DURATION_FLOOR = 1.0e-6
P2G_PIXEL_ALPHA_MIN = 1.0 / 255.0
SHA256_LENGTH = 64
MAX_SAFETENSORS_HEADER_BYTES = 16 * 1024 * 1024
CAMERA_MATRIX_TOLERANCE = 1.0e-8
CAMERA_ROTATION_ABS_TOLERANCE = 1.0e-4
CAMERA_ROTATION_REL_TOLERANCE = 1.0e-4
CAMERA_MAX_DIMENSION = 16_384
CAMERA_MAX_FPS = 240

EXPECTED_PLANES = {
    "center_times": ("F32", (None, 1)),
    "duration_logits": ("F32", (None, 1)),
    "duration_max_seconds": ("F32", (None, 1)),
    "duration_min_seconds": ("F32", (None, 1)),
    "gate_logit_scale": ("F32", ()),
    "log_scales": ("F32", (None, 3)),
    "means": ("F32", (None, 3)),
    "opacity_logits": ("F32", (None, 1)),
    "persistence_logits": ("F32", (None, 1)),
    "quaternions": ("F32", (None, 4)),
    "runtime_ids": ("I64", (None,)),
    "sh0": ("F32", (None, 1, 3)),
    "sh_rest": ("F32", (None, 15, 3)),
    "velocities": ("F32", (None, 3)),
}


class ContractError(ValueError):
    """Input or output violates a versioned asset contract."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ContractError(message)


def reject_duplicate_keys(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        require(key not in result, f"duplicate JSON key: {key}")
        result[key] = value
    return result


def reject_non_standard_constant(token: str) -> None:
    raise ContractError(f"non-standard JSON constant: {token}")


def load_json_bytes(payload: bytes, *, label: str) -> dict[str, Any]:
    try:
        value = json.loads(
            payload,
            object_pairs_hook=reject_duplicate_keys,
            parse_constant=reject_non_standard_constant,
        )
    except (UnicodeDecodeError, json.JSONDecodeError, ContractError) as error:
        raise ContractError(f"{label} is not strict JSON: {error}") from error
    require(isinstance(value, dict), f"{label} must be a JSON object")
    return value


def load_json(path: Path, *, label: str) -> tuple[dict[str, Any], bytes]:
    try:
        before = path.lstat()
    except OSError as error:
        raise ContractError(f"cannot inspect {label} {path}: {error}") from error
    require(
        stat.S_ISREG(before.st_mode),
        f"{label} must be a regular file, not a symlink: {path}",
    )
    flags = os.O_RDONLY
    if hasattr(os, "O_BINARY"):
        flags |= os.O_BINARY
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        raise ContractError(f"cannot open {label} safely {path}: {error}") from error
    try:
        opened = os.fstat(descriptor)
        require(
            stat.S_ISREG(opened.st_mode)
            and (opened.st_dev, opened.st_ino) == (before.st_dev, before.st_ino),
            f"{label} changed while it was being opened: {path}",
        )
        chunks: list[bytes] = []
        while block := os.read(descriptor, 1024 * 1024):
            chunks.append(block)
        after = os.fstat(descriptor)
        require(
            (
                after.st_dev,
                after.st_ino,
                after.st_size,
                after.st_mtime_ns,
                after.st_ctime_ns,
            )
            == (
                opened.st_dev,
                opened.st_ino,
                opened.st_size,
                opened.st_mtime_ns,
                opened.st_ctime_ns,
            ),
            f"{label} changed while it was being read: {path}",
        )
        payload = b"".join(chunks)
    finally:
        os.close(descriptor)
    return load_json_bytes(payload, label=label), payload


def canonical_json_bytes(value: object) -> bytes:
    return (
        json.dumps(
            value,
            allow_nan=False,
            ensure_ascii=False,
            separators=(",", ":"),
            sort_keys=True,
        )
        + "\n"
    ).encode("utf-8")


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(4 * 1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def finite_number(value: object) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(value)
    )


def positive_integer(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def f32(value: float) -> float:
    try:
        result = struct.unpack("<f", struct.pack("<f", value))[0]
    except (OverflowError, struct.error) as error:
        raise ContractError(f"value is not representable as f32: {value!r}") from error
    require(math.isfinite(result) and abs(result) < SHADER_SAFE_ABS, "shader-unsafe f32 value")
    return result


def sigmoid(value: float) -> float:
    if value >= 0.0:
        inverse = math.exp(-value)
        return 1.0 / (1.0 + inverse)
    exponential = math.exp(value)
    return exponential / (1.0 + exponential)


def logit(value: float) -> float:
    require(0.0 < value < 1.0, "duration cannot be represented by a finite logit")
    return math.log(value / (1.0 - value))


def next_positive_f32(value: float) -> float:
    """Return the next representable binary32 value above a positive input."""

    rounded = f32(value)
    require(rounded > 0.0, "next_positive_f32 requires a positive value")
    bits = struct.unpack("<I", struct.pack("<f", rounded))[0]
    require(bits < 0x7F7FFFFF, "binary32 value has no shader-safe successor")
    return f32(struct.unpack("<f", struct.pack("<I", bits + 1))[0])


def product(shape: tuple[int, ...]) -> int:
    result = 1
    for extent in shape:
        result *= extent
    return result


def publish_directory_noreplace(staging: Path, output: Path) -> None:
    """Atomically rename a directory while refusing an existing destination."""

    libc = ctypes.CDLL(None, use_errno=True)
    source_bytes = os.fsencode(staging)
    output_bytes = os.fsencode(output)
    if sys.platform.startswith("linux"):
        try:
            rename = libc.renameat2
        except AttributeError as error:
            raise ContractError(
                "safe publication requires Linux renameat2(RENAME_NOREPLACE)"
            ) from error
        rename.argtypes = [
            ctypes.c_int,
            ctypes.c_char_p,
            ctypes.c_int,
            ctypes.c_char_p,
            ctypes.c_uint,
        ]
        rename.restype = ctypes.c_int
        result = rename(-100, source_bytes, -100, output_bytes, 1)
    elif sys.platform == "darwin":
        try:
            rename = libc.renamex_np
        except AttributeError as error:
            raise ContractError(
                "safe publication requires macOS renamex_np(RENAME_EXCL)"
            ) from error
        rename.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
        rename.restype = ctypes.c_int
        result = rename(source_bytes, output_bytes, 0x00000004)
    else:
        raise ContractError(
            "safe atomic publication is supported only on Linux and macOS"
        )
    if result == 0:
        return
    error_number = ctypes.get_errno()
    if error_number in {errno.EEXIST, errno.ENOTEMPTY}:
        raise ContractError(f"refusing to overwrite output: {output}")
    raise OSError(
        error_number,
        f"cannot atomically publish converted asset: {os.strerror(error_number)}",
        output,
    )


@dataclass(frozen=True)
class TensorInfo:
    name: str
    dtype: str
    shape: tuple[int, ...]
    start: int
    stop: int

    @property
    def byte_count(self) -> int:
        return self.stop - self.start


class SafeTensorArchive:
    """Small read-only Safetensors reader for the two p2g v1 dtypes."""

    def __init__(self, path: Path, *, expected_bytes: int, expected_sha256: str) -> None:
        require(sys.byteorder == "little", "conversion currently requires a little-endian host")
        self.path = path
        flags = os.O_RDONLY
        if hasattr(os, "O_BINARY"):
            flags |= os.O_BINARY
        if hasattr(os, "O_NOFOLLOW"):
            flags |= os.O_NOFOLLOW
        descriptor = os.open(path, flags)
        try:
            self._stream = os.fdopen(descriptor, "rb")
        except BaseException:
            os.close(descriptor)
            raise
        self._mapping: mmap.mmap | None = None
        try:
            opened = os.fstat(self._stream.fileno())
            require(stat.S_ISREG(opened.st_mode), "model.safetensors must be a regular file")
            require(opened.st_size == expected_bytes, "model.safetensors byte count changed before conversion")
            self._identity = (
                opened.st_dev,
                opened.st_ino,
                opened.st_size,
                opened.st_mtime_ns,
                opened.st_ctime_ns,
            )
            self._expected_sha256 = expected_sha256
            self._mapping = mmap.mmap(self._stream.fileno(), 0, access=mmap.ACCESS_READ)
            require(
                hashlib.sha256(self._mapping).hexdigest() == expected_sha256,
                "model.safetensors changed between verification and conversion",
            )
            require(len(self._mapping) >= 8, "Safetensors file is truncated")
            header_bytes = struct.unpack_from("<Q", self._mapping, 0)[0]
            require(
                0 < header_bytes <= MAX_SAFETENSORS_HEADER_BYTES,
                "Safetensors header length is invalid",
            )
            self.data_start = 8 + header_bytes
            require(self.data_start <= len(self._mapping), "Safetensors header is truncated")
            header = load_json_bytes(
                self._mapping[8 : self.data_start].rstrip(b" "),
                label="Safetensors header",
            )
            raw_metadata = header.pop("__metadata__", {})
            require(
                isinstance(raw_metadata, dict)
                and all(isinstance(key, str) and isinstance(value, str) for key, value in raw_metadata.items()),
                "Safetensors __metadata__ must be a string map",
            )
            self.metadata: dict[str, str] = raw_metadata
            infos: dict[str, TensorInfo] = {}
            dtype_bytes = {"F32": 4, "I64": 8}
            for name, raw in header.items():
                require(isinstance(name, str) and name, "Safetensors tensor name is invalid")
                require(isinstance(raw, dict), f"Safetensors entry {name} must be an object")
                require(set(raw) == {"dtype", "shape", "data_offsets"}, f"Safetensors entry {name} has invalid fields")
                dtype = raw["dtype"]
                shape_raw = raw["shape"]
                offsets = raw["data_offsets"]
                require(dtype in dtype_bytes, f"unsupported Safetensors dtype: {name}={dtype}")
                require(
                    isinstance(shape_raw, list)
                    and all(isinstance(item, int) and not isinstance(item, bool) and item >= 0 for item in shape_raw),
                    f"invalid Safetensors shape: {name}",
                )
                require(
                    isinstance(offsets, list)
                    and len(offsets) == 2
                    and all(isinstance(item, int) and not isinstance(item, bool) and item >= 0 for item in offsets),
                    f"invalid Safetensors offsets: {name}",
                )
                start, stop = offsets
                shape = tuple(shape_raw)
                require(start <= stop, f"reversed Safetensors offsets: {name}")
                require(
                    stop - start == product(shape) * dtype_bytes[dtype],
                    f"Safetensors byte count disagrees with shape: {name}",
                )
                infos[name] = TensorInfo(name, dtype, shape, start, stop)
            ordered = sorted(infos.values(), key=lambda item: (item.start, item.stop, item.name))
            cursor = 0
            for info in ordered:
                require(info.start == cursor, f"Safetensors data has a gap or overlap before {info.name}")
                cursor = info.stop
            require(
                self.data_start + cursor == len(self._mapping),
                "Safetensors data length does not match its tensor catalog",
            )
            self.infos = infos
            self._views: list[memoryview] = []
        except BaseException as error:
            if self._mapping is not None:
                self._mapping.close()
            self._stream.close()
            if isinstance(error, (ContractError, KeyboardInterrupt, SystemExit)):
                raise
            if isinstance(error, Exception):
                raise ContractError(f"cannot decode model.safetensors: {error}") from error
            raise

    def verify_unchanged(self) -> None:
        require(self._mapping is not None, "Safetensors archive is closed")
        current = os.fstat(self._stream.fileno())
        identity = (
            current.st_dev,
            current.st_ino,
            current.st_size,
            current.st_mtime_ns,
            current.st_ctime_ns,
        )
        require(identity == self._identity, "model.safetensors changed during conversion")
        require(
            hashlib.sha256(self._mapping).hexdigest() == self._expected_sha256,
            "model.safetensors contents changed during conversion",
        )

    def view(self, name: str, dtype: str, shape: tuple[int | None, ...]) -> memoryview:
        require(self._mapping is not None, "Safetensors archive is closed")
        require(name in self.infos, f"required tensor is missing: {name}")
        info = self.infos[name]
        require(info.dtype == dtype, f"tensor {name} must use {dtype}")
        require(len(info.shape) == len(shape), f"tensor {name} has the wrong rank")
        require(
            all(expected is None or actual == expected for actual, expected in zip(info.shape, shape, strict=True)),
            f"tensor {name} has the wrong shape: {info.shape}",
        )
        raw = memoryview(self._mapping)[
            self.data_start + info.start : self.data_start + info.stop
        ]
        view = raw.cast("f" if dtype == "F32" else "q")
        raw.release()
        self._views.append(view)
        return view

    def close(self) -> None:
        for view in reversed(self._views):
            view.release()
        self._views.clear()
        if self._mapping is not None:
            self._mapping.close()
            self._mapping = None
        self._stream.close()

    def __enter__(self) -> SafeTensorArchive:
        return self

    def __exit__(self, _type: object, _value: object, _traceback: object) -> None:
        self.close()


@dataclass(frozen=True)
class SourceBundle:
    root: Path
    bundle_id: str
    metadata: dict[str, Any]
    model_path: Path
    model_bytes: int
    model_sha256: str
    count: int


def verify_source_bundle(root: Path) -> SourceBundle:
    root = root.expanduser()
    require(not root.is_symlink(), f"source bundle must not be a symlink: {root}")
    root = root.resolve()
    require(root.is_dir(), f"source bundle is not a regular directory: {root}")
    actual_names = {entry.name for entry in root.iterdir()}
    require(
        actual_names == {"asset.json", "manifest.json", "model.safetensors"},
        "source bundle must contain exactly asset.json, manifest.json, and model.safetensors",
    )
    manifest_path = root / "manifest.json"
    require(
        manifest_path.is_file() and not manifest_path.is_symlink(),
        "p2g bundle manifest must be a regular file, not a symlink",
    )
    manifest, _ = load_json(manifest_path, label="p2g bundle manifest")
    require(manifest.get("schema_version") == P2G_MANIFEST_SCHEMA, "unsupported p2g bundle manifest schema")
    files = manifest.get("files")
    require(isinstance(files, list) and len(files) == 2, "p2g manifest must describe exactly two files")
    require(all(isinstance(item, dict) for item in files), "p2g manifest file records must be objects")
    require({item.get("path") for item in files} == {"asset.json", "model.safetensors"}, "p2g manifest file catalog is invalid")
    require(
        manifest.get("bundle_id") == sha256_bytes(canonical_json_bytes(files)),
        "p2g bundle ID is invalid",
    )
    verified_files: dict[str, tuple[int, str]] = {}
    for item in files:
        require(set(item) == {"path", "bytes", "sha256"}, "p2g manifest file record has invalid fields")
        name = item["path"]
        path = root / name
        require(path.is_file() and not path.is_symlink(), f"p2g bundle member is unsafe: {name}")
        require(positive_integer(item["bytes"]) and item["bytes"] == path.stat().st_size, f"p2g byte count mismatch: {name}")
        declared_sha256 = item["sha256"]
        require(
            isinstance(declared_sha256, str)
            and len(declared_sha256) == SHA256_LENGTH,
            f"p2g SHA-256 is invalid: {name}",
        )
        actual_sha256 = sha256_file(path)
        require(declared_sha256 == actual_sha256, f"p2g SHA-256 mismatch: {name}")
        verified_files[name] = (item["bytes"], actual_sha256)
    metadata, _ = load_json(root / "asset.json", label="p2g asset metadata")
    require(metadata.get("schema_version") == P2G_BUNDLE_SCHEMA, "unsupported p2g asset schema")
    version = metadata.get("format_version")
    require(isinstance(version, dict) and version.get("major") == 1, "unsupported p2g format major version")
    model = metadata.get("model")
    require(isinstance(model, dict), "p2g model metadata is missing")
    model_path = root / "model.safetensors"
    model_bytes, model_sha256 = verified_files["model.safetensors"]
    require(
        model.get("file") == "model.safetensors"
        and model.get("schema_version") == P2G_MODEL_SCHEMA
        and model.get("bytes") == model_bytes
        and model.get("sha256") == model_sha256,
        "p2g asset metadata does not bind model.safetensors",
    )
    count = model.get("gaussian_count")
    require(positive_integer(count) and count <= 0xFFFFFFFF, "p2g Gaussian count is invalid")
    return SourceBundle(
        root,
        manifest["bundle_id"],
        metadata,
        model_path,
        model_bytes,
        model_sha256,
        count,
    )


def validate_source_semantics(source: SourceBundle, archive: SafeTensorArchive) -> dict[str, Any]:
    metadata = source.metadata
    model = metadata["model"]
    equations = metadata.get("equations")
    appearance = metadata.get("appearance")
    time = metadata.get("time")
    coordinates = metadata.get("coordinates")
    camera = metadata.get("camera")
    renderer = metadata.get("renderer")
    require(isinstance(equations, dict), "p2g equations metadata is missing")
    require(equations.get("version") == P2G_EQUATION_VERSION, "unsupported p2g equation version")
    require(equations.get("persistence") == "learned", "player bridge requires learned persistence")
    gate_scale = equations.get("gate_logit_scale")
    require(finite_number(gate_scale) and gate_scale > 0.0, "p2g gate scale is invalid")
    require(isinstance(appearance, dict), "p2g appearance metadata is missing")
    require(
        appearance.get("representation") == "real_spherical_harmonics"
        and appearance.get("convention") == "gsplat_real_sh_v1"
        and appearance.get("coefficient_color_space") == "linear_rgb",
        "unsupported p2g appearance convention",
    )
    require(
        appearance.get("max_sh_degree") == 3 and appearance.get("default_sh_degree") == 3,
        "player bridge currently requires a default SH degree of 3",
    )
    photometric_space = appearance.get("output_photometric_space")
    require(
        photometric_space in {"linear_rgb", "srgb_reference_profile"},
        "unsupported p2g output photometric space",
    )
    require(isinstance(time, dict) and time.get("unit") == "seconds", "p2g time must use seconds")
    interval = time.get("valid_interval")
    require(
        isinstance(interval, list)
        and len(interval) == 2
        and all(finite_number(value) for value in interval)
        and interval[0] < interval[1],
        "p2g valid time interval is invalid",
    )
    require(
        isinstance(coordinates, dict)
        and coordinates.get("extrinsic") == "world_to_camera"
        and coordinates.get("camera_axes") == "opencv_x_right_y_down_z_forward",
        "unsupported p2g coordinate convention",
    )
    require(
        isinstance(camera, dict)
        and camera.get("model") == "pinhole"
        and camera.get("distortion") == "pre-undistorted"
        and camera.get("intrinsic_matrix") == "3x3_pixel_center"
        and camera.get("extrinsic_matrix") == "4x4_world_to_camera",
        "unsupported p2g camera convention",
    )
    require(isinstance(renderer, dict) and renderer.get("abi") == P2G_RENDERER_ABI, "unsupported p2g renderer ABI")
    require(renderer.get("clamp_rgb") is True, "player bridge requires clamp_rgb=true")
    require(renderer.get("radius_clip") == 0, "player bridge cannot preserve non-zero radius_clip")
    for name in ("near_plane", "far_plane", "eps2d"):
        require(finite_number(renderer.get(name)), f"p2g renderer {name} is invalid")
    require(0.0 < renderer["near_plane"] < renderer["far_plane"], "p2g near/far interval is invalid")
    require(renderer["eps2d"] >= 0.0, "p2g eps2d is invalid")
    background = renderer.get("background_linear_rgb")
    require(
        isinstance(background, list)
        and len(background) == 3
        and all(finite_number(value) and 0.0 <= value <= 1.0 for value in background),
        "p2g background is invalid",
    )

    require(set(archive.infos) == set(EXPECTED_PLANES), "p2g model tensor set is unsupported")
    for name, (dtype, shape) in EXPECTED_PLANES.items():
        expected = tuple(source.count if extent is None else extent for extent in shape)
        info = archive.infos[name]
        require(info.dtype == dtype and info.shape == expected, f"p2g tensor contract mismatch: {name}")
    dtype_names = {"F32": "torch.float32", "I64": "torch.int64"}
    catalog = [
        {
            "name": name,
            "dtype": dtype_names[archive.infos[name].dtype],
            "shape": list(archive.infos[name].shape),
            "bytes": archive.infos[name].byte_count,
        }
        for name in sorted(archive.infos)
    ]
    require(model.get("tensor_count") == len(catalog) and model.get("tensors") == catalog, "p2g tensor catalog disagrees with Safetensors")
    tensor_metadata = archive.metadata
    require(tensor_metadata.get("schema_version") == P2G_MODEL_SCHEMA, "unsupported p2g Safetensors schema")
    require(tensor_metadata.get("equation_version") == P2G_EQUATION_VERSION, "p2g equation metadata mismatch")
    require(tensor_metadata.get("persistence") == "learned", "p2g persistence metadata mismatch")
    try:
        header_gate_scale = float(tensor_metadata["gate_logit_scale"])
    except (KeyError, ValueError) as error:
        raise ContractError("p2g Safetensors gate scale is invalid") from error
    require(
        math.isfinite(header_gate_scale)
        and header_gate_scale > 0.0
        and header_gate_scale == float(gate_scale),
        "p2g gate scale metadata mismatch",
    )
    tensor_gate_scale = float(archive.view("gate_logit_scale", "F32", ())[0])
    require(
        math.isfinite(tensor_gate_scale)
        and tensor_gate_scale > 0.0
        and tensor_gate_scale == header_gate_scale,
        "p2g gate_logit_scale tensor disagrees with asset/header metadata",
    )
    return {
        "gate_scale": float(gate_scale),
        "photometric_space": photometric_space,
        "time_start": float(interval[0]),
        "time_stop": float(interval[1]),
        "renderer": renderer,
    }


def selected_camera(
    path: Path,
    *,
    bundle_id: str,
    frame_index: int,
    valid_interval: tuple[float, float],
) -> tuple[dict[str, Any], str]:
    path = path.expanduser()
    require(not path.is_symlink(), f"camera path must not be a symlink: {path}")
    path = path.resolve()
    require(path.is_file(), f"camera path is not a regular file: {path}")
    camera_path, payload = load_json(path, label="p2g camera path")
    expected_fields = {
        "schema_version",
        "asset_bundle_id",
        "time_unit",
        "camera_model",
        "pixel_domain",
        "intrinsic_matrix",
        "extrinsic_matrix",
        "camera_axes",
        "width",
        "height",
        "fps",
        "frames",
    }
    require(set(camera_path) == expected_fields, "camera path has missing or unknown fields")
    require(camera_path.get("schema_version") == P2G_CAMERA_PATH_SCHEMA, "unsupported camera-path schema")
    require(camera_path.get("asset_bundle_id") == bundle_id, "camera path is bound to a different AssetBundle")
    require(
        camera_path.get("camera_axes") == "opencv_x_right_y_down_z_forward"
        and camera_path.get("camera_model") == "pinhole"
        and camera_path.get("extrinsic_matrix") == "4x4_world_to_camera"
        and camera_path.get("intrinsic_matrix") == "3x3_pixel_center"
        and camera_path.get("pixel_domain") == "pre-undistorted"
        and camera_path.get("time_unit") == "seconds",
        "unsupported camera-path convention",
    )
    width, height = camera_path.get("width"), camera_path.get("height")
    require(
        positive_integer(width)
        and positive_integer(height)
        and width <= CAMERA_MAX_DIMENSION
        and height <= CAMERA_MAX_DIMENSION,
        "camera-path dimensions are invalid",
    )
    fps = camera_path.get("fps")
    require(positive_integer(fps) and fps <= CAMERA_MAX_FPS, "camera-path fps is invalid")
    frames = camera_path.get("frames")
    require(isinstance(frames, list) and len(frames) >= 2, "camera path must contain at least two frames")
    require(0 <= frame_index < len(frames), "camera frame index is out of range")
    validated_frames: list[dict[str, Any]] = []
    previous_timestamp: float | None = None
    time_start, time_stop = valid_interval
    for index, raw_frame in enumerate(frames):
        require(
            isinstance(raw_frame, dict)
            and set(raw_frame) == {"timestamp_seconds", "intrinsic", "world_to_camera"},
            f"camera-path frame {index} has missing or unknown fields",
        )
        matrix = raw_frame["world_to_camera"]
        intrinsic = raw_frame["intrinsic"]
        require(
            isinstance(matrix, list)
            and len(matrix) == 4
            and all(isinstance(row, list) and len(row) == 4 for row in matrix)
            and all(finite_number(value) for row in matrix for value in row),
            f"camera-path frame {index} world-to-camera matrix is invalid",
        )
        require(
            isinstance(intrinsic, list)
            and len(intrinsic) == 3
            and all(isinstance(row, list) and len(row) == 3 for row in intrinsic)
            and all(finite_number(value) for row in intrinsic for value in row),
            f"camera-path frame {index} intrinsic matrix is invalid",
        )
        require(
            abs(float(intrinsic[0][1])) <= CAMERA_MATRIX_TOLERANCE
            and abs(float(intrinsic[1][0])) <= CAMERA_MATRIX_TOLERANCE
            and abs(float(intrinsic[2][0])) <= CAMERA_MATRIX_TOLERANCE
            and abs(float(intrinsic[2][1])) <= CAMERA_MATRIX_TOLERANCE
            and abs(float(intrinsic[2][2]) - 1.0) <= CAMERA_MATRIX_TOLERANCE,
            f"camera-path frame {index} uses skewed or projective intrinsics",
        )
        require(
            intrinsic[0][0] > 0.0 and intrinsic[1][1] > 0.0,
            f"camera-path frame {index} focal lengths must be positive",
        )
        require(
            all(
                abs(float(matrix[3][column]) - expected) <= CAMERA_MATRIX_TOLERANCE
                for column, expected in enumerate((0.0, 0.0, 0.0, 1.0))
            ),
            f"camera-path frame {index} has an invalid affine last row",
        )
        for row in range(3):
            for other in range(3):
                dot = sum(float(matrix[row][column]) * float(matrix[other][column]) for column in range(3))
                expected = 1.0 if row == other else 0.0
                tolerance = CAMERA_ROTATION_ABS_TOLERANCE + CAMERA_ROTATION_REL_TOLERANCE * abs(expected)
                require(
                    abs(dot - expected) <= tolerance,
                    f"camera-path frame {index} rotation is not orthonormal",
                )
        determinant = (
            matrix[0][0] * (matrix[1][1] * matrix[2][2] - matrix[1][2] * matrix[2][1])
            - matrix[0][1] * (matrix[1][0] * matrix[2][2] - matrix[1][2] * matrix[2][0])
            + matrix[0][2] * (matrix[1][0] * matrix[2][1] - matrix[1][1] * matrix[2][0])
        )
        require(
            abs(float(determinant) - 1.0) <= CAMERA_ROTATION_ABS_TOLERANCE,
            f"camera-path frame {index} rotation is not right-handed",
        )
        timestamp = raw_frame["timestamp_seconds"]
        require(finite_number(timestamp), f"camera-path frame {index} timestamp is invalid")
        timestamp = float(timestamp)
        require(
            previous_timestamp is None or timestamp >= previous_timestamp,
            "camera-path timestamps must be nondecreasing",
        )
        require(
            time_start <= timestamp <= time_stop,
            f"camera-path frame {index} timestamp is outside the asset time interval",
        )
        previous_timestamp = timestamp
        validated_frames.append(raw_frame)

    frame = validated_frames[frame_index]
    matrix = frame["world_to_camera"]
    intrinsic = frame["intrinsic"]
    timestamp = float(frame["timestamp_seconds"])
    initial_normalized_time = f32((timestamp - time_start) / (time_stop - time_start))
    return {
        "world_to_camera_row_major": [
            [f32(float(value)) for value in row]
            for row in matrix
        ],
        "intrinsics": [
            f32(float(intrinsic[0][0])),
            f32(float(intrinsic[1][1])),
            f32(float(intrinsic[0][2])),
            f32(float(intrinsic[1][2])),
        ],
        "source_size": [width, height],
        "timestamp_seconds": timestamp,
        "initial_normalized_time": initial_normalized_time,
        "frame_count": len(frames),
    }, sha256_bytes(payload)


def write_output(
    source: SourceBundle,
    archive: SafeTensorArchive,
    semantics: dict[str, Any],
    camera: dict[str, Any],
    camera_sha256: str,
    output: Path,
    *,
    name: str,
    camera_frame: int,
    temporal_threshold: float,
    alpha_min: float,
) -> dict[str, Any]:
    count = source.count
    views = {
        name: archive.view(name, dtype, tuple(count if extent is None else extent for extent in shape))
        for name, (dtype, shape) in EXPECTED_PLANES.items()
    }
    runtime_ids = views["runtime_ids"]
    unique_runtime_ids: set[int] = set()
    for index in range(count):
        runtime_id = int(runtime_ids[index])
        require(runtime_id not in unique_runtime_ids, "runtime_ids must be unique")
        unique_runtime_ids.add(runtime_id)
    del unique_runtime_ids

    duration_min = views["duration_min_seconds"]
    duration_max = views["duration_max_seconds"]
    first_max = float(duration_max[0])
    common_ftgspp_duration = True
    source_duration_upper = 0.0
    for index in range(count):
        minimum = float(duration_min[index])
        maximum = float(duration_max[index])
        require(
            math.isfinite(minimum)
            and math.isfinite(maximum)
            and 0.0 <= minimum < maximum,
            f"invalid duration bounds at Gaussian {index}",
        )
        source_duration_upper = max(source_duration_upper, maximum)
        common_ftgspp_duration &= minimum == 0.0 and maximum == first_max

    time_start = semantics["time_start"]
    time_stop = semantics["time_stop"]
    time_span = time_stop - time_start
    exact_duration_upper = source_duration_upper / time_span
    player_max_duration = f32(6.0 * exact_duration_upper)
    player_duration_upper = f32(player_max_duration / 6.0)
    if not common_ftgspp_duration:
        # Reparameterized durations require a finite logit. Keep the target
        # upper bound strictly above every source bound even when two f32
        # roundings (max_duration and the shader's /6) move downward.
        while player_duration_upper <= exact_duration_upper:
            player_max_duration = next_positive_f32(player_max_duration)
            player_duration_upper = f32(player_max_duration / 6.0)
    require(player_duration_upper > 0.0, "normalized duration upper bound underflows")

    requested_output = output.expanduser()
    require(
        not requested_output.exists() and not requested_output.is_symlink(),
        f"refusing to overwrite output: {requested_output}",
    )
    requested_output.parent.mkdir(parents=True, exist_ok=True)
    output = requested_output.parent.resolve() / requested_output.name
    require(not output.exists() and not output.is_symlink(), f"refusing to overwrite output: {output}")
    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.tmp-", dir=output.parent))
    geometry_path = staging / "gaussians.bin"
    appearance_path = staging / "sh3.f16"
    geometry_digest = hashlib.sha256()
    appearance_digest = hashlib.sha256()
    sh_error_max = 0.0
    sh_error_sum = 0.0
    sh_value_count = count * 45
    duration_error_max = 0.0
    gate_scale = semantics["gate_scale"]

    means = views["means"]
    centers = views["center_times"]
    log_scales = views["log_scales"]
    opacities = views["opacity_logits"]
    quaternions = views["quaternions"]
    velocities = views["velocities"]
    persistence = views["persistence_logits"]
    sh0 = views["sh0"]
    duration_logits = views["duration_logits"]
    sh_rest = views["sh_rest"]

    header = HEADER.pack(
        MAGIC,
        1,
        HEADER_BYTES,
        count,
        RECORD_BYTES,
        HEADER_BYTES,
        count * RECORD_BYTES,
        0,
        0,
        0,
    )
    geometry_digest.update(header)
    try:
        with geometry_path.open("xb") as geometry, appearance_path.open("xb") as appearance:
            geometry.write(header)
            for index in range(count):
                center = f32((float(centers[index]) - time_start) / time_span)
                velocity = [f32(float(velocities[index * 3 + axis]) * time_span) for axis in range(3)]
                raw_gate = f32(float(persistence[index]) * gate_scale / PLAYER_GATE_SCALE)
                source_raw_duration = float(duration_logits[index])
                source_sigma = float(duration_min[index]) + (
                    float(duration_max[index]) - float(duration_min[index])
                ) * sigmoid(source_raw_duration)
                normalized_sigma = source_sigma / time_span
                require(
                    normalized_sigma >= PLAYER_DURATION_FLOOR,
                    f"duration at Gaussian {index} is below the player floor",
                )
                if common_ftgspp_duration:
                    raw_duration = f32(source_raw_duration)
                else:
                    raw_duration = f32(logit(normalized_sigma / player_duration_upper))
                reconstructed_sigma = player_duration_upper * sigmoid(raw_duration)
                duration_error_max = max(duration_error_max, abs(reconstructed_sigma - normalized_sigma))
                wxyz = [float(quaternions[index * 4 + axis]) for axis in range(4)]
                values = (
                    *(f32(float(means[index * 3 + axis])) for axis in range(3)),
                    center,
                    *(f32(float(log_scales[index * 3 + axis])) for axis in range(3)),
                    f32(float(opacities[index])),
                    f32(wxyz[1]),
                    f32(wxyz[2]),
                    f32(wxyz[3]),
                    f32(wxyz[0]),
                    *velocity,
                    raw_gate,
                    *(f32(float(sh0[index * 3 + axis])) for axis in range(3)),
                    raw_duration,
                )
                require(
                    sum(value * value for value in values[8:12]) > 1.0e-12,
                    f"zero quaternion at Gaussian {index}",
                )
                record = RECORD.pack(*values)
                geometry.write(record)
                geometry_digest.update(record)

                source_coefficients = [float(sh_rest[index * 45 + offset]) for offset in range(45)]
                require(
                    all(math.isfinite(value) and abs(value) < SHADER_SAFE_ABS for value in source_coefficients),
                    f"invalid SH coefficient at Gaussian {index}",
                )
                try:
                    encoded_sh = SH3_RECORD.pack(*source_coefficients)
                except (OverflowError, struct.error) as error:
                    raise ContractError(f"SH coefficient does not fit binary16 at Gaussian {index}") from error
                quantized = struct.unpack_from("<45e", encoded_sh)
                for original, encoded in zip(source_coefficients, quantized, strict=True):
                    error = abs(original - encoded)
                    sh_error_max = max(sh_error_max, error)
                    sh_error_sum += error
                appearance.write(encoded_sh)
                appearance_digest.update(encoded_sh)
            geometry.flush()
            appearance.flush()
            os.fsync(geometry.fileno())
            os.fsync(appearance.fileno())

        archive.verify_unchanged()

        renderer = semantics["renderer"]
        if semantics["photometric_space"] == "linear_rgb":
            working_space = "linear-rgb"
            output_transfer = "srgb"
        else:
            working_space = "display-srgb"
            output_transfer = "identity"
        converter_path = Path(__file__).resolve()
        manifest: dict[str, Any] = {
            "schema": PHI_SCHEMA,
            "version": 1,
            "name": name,
            "gaussian_count": count,
            "record_stride": RECORD_BYTES,
            "binary": {
                "uri": geometry_path.name,
                "bytes": geometry_path.stat().st_size,
                "sha256": geometry_digest.hexdigest(),
            },
            "appearance": {
                "uri": appearance_path.name,
                "bytes": appearance_path.stat().st_size,
                "sha256": appearance_digest.hexdigest(),
                "degree": 3,
                "coefficients": 15,
                "channels": 3,
                "encoding": "float16-le-padded46",
                "record_stride": SH3_RECORD_BYTES,
            },
            "time": {
                "domain": [0, 1],
                "initial": camera["initial_normalized_time"],
                "max_duration": player_max_duration,
                "units": "normalized",
            },
            "representation": {
                "velocity": "explicit-linear",
                "rotation": "raw-xyzw",
                "scale": "raw-log",
                "opacity": "raw-logit",
                "gate": "raw-logit-times-20",
                "duration": "raw-logit-max-duration-over-6",
                "color": "raw-sh3",
            },
            "policy": {
                "temporal_threshold": temporal_threshold,
                "alpha_min": P2G_PIXEL_ALPHA_MIN,
                "low_pass": f32(float(renderer["eps2d"])),
                "opacity_compensation": "none",
                "alpha_cap": 0.999,
                "pixel_alpha_min": P2G_PIXEL_ALPHA_MIN,
                "transmittance_epsilon": 1.0e-4,
            },
            "render": {
                "working_space": working_space,
                "output_transfer": output_transfer,
                "background": [
                    *(f32(float(value)) for value in renderer["background_linear_rgb"]),
                    1.0,
                ],
            },
            "camera": {
                "fixed": {
                    "world_to_camera_row_major": camera["world_to_camera_row_major"],
                    "intrinsics": camera["intrinsics"],
                    "source_size": camera["source_size"],
                    "near": f32(float(renderer["near_plane"])),
                    "far": f32(float(renderer["far_plane"])),
                }
            },
            "provenance": {
                "kind": "p2g-asset-bundle-conversion",
                "converter": "tools/convert_p2g_asset.py",
                "converter_sha256": sha256_file(converter_path),
                "source_schema": P2G_BUNDLE_SCHEMA,
                "source_bundle_id": source.bundle_id,
                "source_model_sha256": source.model_sha256,
                "source_time_interval_seconds": [time_start, time_stop],
                "camera_path_sha256": camera_sha256,
                "camera_frame": camera_frame,
                "camera_timestamp_seconds": camera["timestamp_seconds"],
                "initial_normalized_time": camera["initial_normalized_time"],
                "duration_mapping": (
                    "normalized-ftgspp-raw-logit-preserved"
                    if common_ftgspp_duration
                    else "physical-sigma-reparameterized"
                ),
                "gate_mapping": "raw_out=source_gate_scale/20*persistence_logit",
                "quaternion_mapping": "wxyz-to-xyzw",
                "appearance_mapping": "sh0-f32-preserved;sh-rest-f32-to-f16",
                "rasterize_mode_mapping": "p2g-classic-to-opacity-compensation-none",
                "source_rights": source.metadata.get("rights", {}),
            },
        }
        manifest_path = staging / "manifest.json"
        manifest_path.write_bytes(
            (json.dumps(manifest, indent=2, sort_keys=True, allow_nan=False) + "\n").encode("utf-8")
        )
        with manifest_path.open("rb") as stream:
            os.fsync(stream.fileno())

        try:
            from tools import validate_asset
        except ImportError:
            import validate_asset  # type: ignore[no-redef]

        validation = validate_asset.validate(manifest_path)
        directory_fd = os.open(staging, os.O_RDONLY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
        publish_directory_noreplace(staging, output)
        validation["manifest"] = str(output / "manifest.json")
        parent_fd = os.open(output.parent, os.O_RDONLY)
        try:
            os.fsync(parent_fd)
        finally:
            os.close(parent_fd)
        return {
            "status": "PASS",
            "output": str(output),
            "source_bundle_id": source.bundle_id,
            "source_model_sha256": source.model_sha256,
            "gaussian_count": count,
            "geometry_sha256": manifest["binary"]["sha256"],
            "appearance_sha256": manifest["appearance"]["sha256"],
            "duration_mapping": manifest["provenance"]["duration_mapping"],
            "initial_normalized_time": camera["initial_normalized_time"],
            "duration_normalized_max_abs_error": duration_error_max,
            "sh3_f16_max_abs_error": sh_error_max,
            "sh3_f16_mean_abs_error": sh_error_sum / sh_value_count,
            "validation": validation,
        }
    except BaseException:
        shutil.rmtree(staging, ignore_errors=True)
        raise


def convert(
    bundle: Path,
    camera_path: Path,
    output: Path,
    *,
    name: str | None = None,
    camera_frame: int = 0,
    temporal_threshold: float = 0.0,
) -> dict[str, Any]:
    require(finite_number(temporal_threshold) and 0.0 <= temporal_threshold <= 1.0, "temporal threshold must be in [0,1]")
    require(
        isinstance(camera_frame, int) and not isinstance(camera_frame, bool) and camera_frame >= 0,
        "camera frame must be a non-negative integer",
    )
    source = verify_source_bundle(bundle)
    with SafeTensorArchive(
        source.model_path,
        expected_bytes=source.model_bytes,
        expected_sha256=source.model_sha256,
    ) as archive:
        semantics = validate_source_semantics(source, archive)
        camera, camera_sha256 = selected_camera(
            camera_path,
            bundle_id=source.bundle_id,
            frame_index=camera_frame,
            valid_interval=(semantics["time_start"], semantics["time_stop"]),
        )
        return write_output(
            source,
            archive,
            semantics,
            camera,
            camera_sha256,
            output,
            name=name or f"p2g-{source.bundle_id[:12]}",
            camera_frame=camera_frame,
            temporal_threshold=float(temporal_threshold),
            alpha_min=P2G_PIXEL_ALPHA_MIN,
        )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Convert a verified p2g AssetBundle v1 and camera path to Phi explicit-v1"
    )
    parser.add_argument("bundle", type=Path, help="directory containing asset.json, manifest.json, and model.safetensors")
    parser.add_argument("camera_path", type=Path, help="p2g.camera_path.v1 JSON bound to the AssetBundle")
    parser.add_argument("output", type=Path, help="new output directory; existing paths are refused")
    parser.add_argument("--name", help="asset name stored in the Phi manifest")
    parser.add_argument(
        "--camera-frame",
        type=int,
        default=0,
        help=(
            "camera-path frame used as the interactive initial camera; its normalized "
            "timestamp is stored as manifest time.initial"
        ),
    )
    parser.add_argument("--temporal-threshold", type=float, default=0.0, help="temporal culling threshold; zero preserves the p2g active set")
    args = parser.parse_args()
    try:
        receipt = convert(
            args.bundle,
            args.camera_path,
            args.output,
            name=args.name,
            camera_frame=args.camera_frame,
            temporal_threshold=args.temporal_threshold,
        )
    except (ContractError, OSError) as error:
        parser.exit(2, f"conversion failed: {error}\n")
    print(json.dumps(receipt, indent=2, sort_keys=True, allow_nan=False))


if __name__ == "__main__":
    main()
