from __future__ import annotations

import hashlib
import json
import math
import struct
import tempfile
import unittest
from pathlib import Path

from tools import convert_p2g_asset as bridge
from tools import validate_asset


class P2gBridgeTest(unittest.TestCase):
    count = 2

    @staticmethod
    def _canonical(value: object) -> bytes:
        return bridge.canonical_json_bytes(value)

    def _tensors(
        self,
        *,
        tiny_duration: bool = False,
        duration_logits: list[float] | None = None,
        duration_max_seconds: list[float] | None = None,
        runtime_ids: list[int] | None = None,
        tensor_gate_scale: float = 20.0,
    ) -> dict[str, tuple[str, list[int], list[float | int]]]:
        raw_duration = duration_logits or ([-100.0, 1.0] if tiny_duration else [0.0, 1.0])
        return {
            "center_times": ("F32", [2, 1], [2.0, 4.0]),
            "duration_logits": ("F32", [2, 1], raw_duration),
            "duration_max_seconds": (
                "F32",
                [2, 1],
                duration_max_seconds or [0.5, 0.5],
            ),
            "duration_min_seconds": ("F32", [2, 1], [0.0, 0.0]),
            "gate_logit_scale": ("F32", [], [tensor_gate_scale]),
            "log_scales": ("F32", [2, 3], [-2.0, -2.1, -2.2, -3.0, -3.1, -3.2]),
            "means": ("F32", [2, 3], [1.0, 2.0, 3.0, -1.0, -2.0, 4.0]),
            "opacity_logits": ("F32", [2, 1], [0.25, -0.5]),
            "persistence_logits": ("F32", [2, 1], [-0.1, 0.2]),
            "quaternions": (
                "F32",
                [2, 4],
                [1.0, 0.0, 0.0, 0.0, math.sqrt(0.5), math.sqrt(0.5), 0.0, 0.0],
            ),
            "runtime_ids": ("I64", [2], runtime_ids or [42, 7]),
            "sh0": ("F32", [2, 1, 3], [0.1, 0.2, 0.3, -0.1, -0.2, -0.3]),
            "sh_rest": (
                "F32",
                [2, 15, 3],
                [((index % 13) - 6) / 127.0 for index in range(90)],
            ),
            "velocities": ("F32", [2, 3], [0.1, 0.2, 0.3, -0.1, 0.0, 0.1]),
        }

    def _write_safetensors(
        self,
        path: Path,
        tensors: dict[str, tuple[str, list[int], list[float | int]]],
        *,
        header_gate_scale: float = 20.0,
    ) -> None:
        header: dict[str, object] = {
            "__metadata__": {
                "equation_version": bridge.P2G_EQUATION_VERSION,
                "gate_logit_scale": repr(header_gate_scale),
                "persistence": "learned",
                "schema_version": bridge.P2G_MODEL_SCHEMA,
            }
        }
        chunks: list[bytes] = []
        offset = 0
        for name in sorted(tensors):
            dtype, shape, values = tensors[name]
            code = "f" if dtype == "F32" else "q"
            payload = struct.pack(f"<{len(values)}{code}", *values)
            header[name] = {
                "data_offsets": [offset, offset + len(payload)],
                "dtype": dtype,
                "shape": shape,
            }
            chunks.append(payload)
            offset += len(payload)
        encoded = json.dumps(
            header,
            allow_nan=False,
            ensure_ascii=False,
            separators=(",", ":"),
            sort_keys=True,
        ).encode("utf-8")
        encoded += b" " * (-len(encoded) % 8)
        path.write_bytes(struct.pack("<Q", len(encoded)) + encoded + b"".join(chunks))

    def _write_bundle(
        self,
        root: Path,
        *,
        photometric_space: str = "srgb_reference_profile",
        tiny_duration: bool = False,
        duration_logits: list[float] | None = None,
        duration_max_seconds: list[float] | None = None,
        runtime_ids: list[int] | None = None,
        tensor_gate_scale: float = 20.0,
        header_gate_scale: float = 20.0,
        asset_gate_scale: float = 20.0,
    ) -> tuple[Path, Path, str]:
        bundle = root / "bundle"
        bundle.mkdir()
        tensors = self._tensors(
            tiny_duration=tiny_duration,
            duration_logits=duration_logits,
            duration_max_seconds=duration_max_seconds,
            runtime_ids=runtime_ids,
            tensor_gate_scale=tensor_gate_scale,
        )
        model_path = bundle / "model.safetensors"
        self._write_safetensors(
            model_path,
            tensors,
            header_gate_scale=header_gate_scale,
        )
        dtype_names = {"F32": "torch.float32", "I64": "torch.int64"}
        dtype_bytes = {"F32": 4, "I64": 8}
        catalog = [
            {
                "name": name,
                "dtype": dtype_names[tensors[name][0]],
                "shape": tensors[name][1],
                "bytes": len(tensors[name][2]) * dtype_bytes[tensors[name][0]],
            }
            for name in sorted(tensors)
        ]
        model_sha256 = hashlib.sha256(model_path.read_bytes()).hexdigest()
        metadata = {
            "schema_version": bridge.P2G_BUNDLE_SCHEMA,
            "format_version": {"major": 1, "minor": 0},
            "model": {
                "file": "model.safetensors",
                "schema_version": bridge.P2G_MODEL_SCHEMA,
                "bytes": model_path.stat().st_size,
                "sha256": model_sha256,
                "gaussian_count": self.count,
                "tensor_count": len(catalog),
                "tensors": catalog,
            },
            "equations": {
                "version": bridge.P2G_EQUATION_VERSION,
                "persistence": "learned",
                "gate_logit_scale": asset_gate_scale,
            },
            "appearance": {
                "representation": "real_spherical_harmonics",
                "convention": "gsplat_real_sh_v1",
                "max_sh_degree": 3,
                "default_sh_degree": 3,
                "coefficient_color_space": "linear_rgb",
                "output_photometric_space": photometric_space,
            },
            "time": {
                "unit": "seconds",
                "valid_interval": [2.0, 6.0],
                "reference_time": 2.0,
            },
            "coordinates": {
                "extrinsic": "world_to_camera",
                "camera_axes": "opencv_x_right_y_down_z_forward",
            },
            "camera": {
                "model": "pinhole",
                "distortion": "pre-undistorted",
                "intrinsic_matrix": "3x3_pixel_center",
                "extrinsic_matrix": "4x4_world_to_camera",
            },
            "renderer": {
                "abi": bridge.P2G_RENDERER_ABI,
                "near_plane": 0.01,
                "far_plane": 100.0,
                "eps2d": 0.3,
                "radius_clip": 0.0,
                "clamp_rgb": True,
                "background_linear_rgb": [0.01, 0.02, 0.03],
            },
            "rights": {
                "asset_license": "test-only",
                "source_data_license": "test-only",
                "redistribution": "restricted",
                "provenance_summary": "deterministic unit fixture",
            },
        }
        asset_path = bundle / "asset.json"
        asset_path.write_bytes(self._canonical(metadata))
        files = [
            {
                "path": "asset.json",
                "bytes": asset_path.stat().st_size,
                "sha256": hashlib.sha256(asset_path.read_bytes()).hexdigest(),
            },
            {
                "path": "model.safetensors",
                "bytes": model_path.stat().st_size,
                "sha256": model_sha256,
            },
        ]
        bundle_id = hashlib.sha256(self._canonical(files)).hexdigest()
        (bundle / "manifest.json").write_bytes(
            self._canonical(
                {
                    "schema_version": bridge.P2G_MANIFEST_SCHEMA,
                    "bundle_id": bundle_id,
                    "files": files,
                }
            )
        )
        camera_path = root / "camera_path.json"
        camera_path.write_bytes(
            self._canonical(
                {
                    "schema_version": bridge.P2G_CAMERA_PATH_SCHEMA,
                    "asset_bundle_id": bundle_id,
                    "camera_axes": "opencv_x_right_y_down_z_forward",
                    "camera_model": "pinhole",
                    "extrinsic_matrix": "4x4_world_to_camera",
                    "intrinsic_matrix": "3x3_pixel_center",
                    "pixel_domain": "pre-undistorted",
                    "time_unit": "seconds",
                    "width": 640,
                    "height": 360,
                    "fps": 30,
                    "frames": [
                        {
                            "timestamp_seconds": 2.0,
                            "world_to_camera": [
                                [1.0, 0.0, 0.0, 0.0],
                                [0.0, 1.0, 0.0, 0.0],
                                [0.0, 0.0, 1.0, 0.0],
                                [0.0, 0.0, 0.0, 1.0],
                            ],
                            "intrinsic": [
                                [500.0, 0.0, 320.0],
                                [0.0, 501.0, 180.0],
                                [0.0, 0.0, 1.0],
                            ],
                        },
                        {
                            "timestamp_seconds": 3.0,
                            "world_to_camera": [
                                [1.0, 0.0, 0.0, 0.25],
                                [0.0, 1.0, 0.0, 0.0],
                                [0.0, 0.0, 1.0, 0.0],
                                [0.0, 0.0, 0.0, 1.0],
                            ],
                            "intrinsic": [
                                [500.0, 0.0, 320.0],
                                [0.0, 501.0, 180.0],
                                [0.0, 0.0, 1.0],
                            ],
                        },
                    ],
                }
            )
        )
        return bundle, camera_path, bundle_id

    @staticmethod
    def _tree_digest(root: Path) -> dict[str, str]:
        return {
            path.name: hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(root.iterdir())
            if path.is_file()
        }

    def test_bridge_maps_the_p2g_contract_and_is_byte_deterministic(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle, camera_path, bundle_id = self._write_bundle(root)
            first = root / "first"
            second = root / "second"
            receipt = bridge.convert(bundle, camera_path, first, name="bridge-fixture")
            bridge.convert(bundle, camera_path, second, name="bridge-fixture")
            self.assertEqual(self._tree_digest(first), self._tree_digest(second))
            self.assertEqual(receipt["status"], "PASS")
            self.assertEqual(receipt["source_bundle_id"], bundle_id)
            self.assertEqual(receipt["duration_mapping"], "normalized-ftgspp-raw-logit-preserved")
            self.assertEqual(receipt["initial_normalized_time"], 0.0)
            self.assertEqual(
                receipt["validation"]["manifest"],
                str((first / "manifest.json").resolve()),
            )
            self.assertGreater(receipt["sh3_f16_max_abs_error"], 0.0)
            validation = validate_asset.validate(first / "manifest.json")
            self.assertEqual(validation["gaussian_count"], self.count)
            manifest = json.loads((first / "manifest.json").read_text())
            self.assertEqual(manifest["render"]["working_space"], "display-srgb")
            self.assertEqual(manifest["render"]["output_transfer"], "identity")
            self.assertEqual(manifest["policy"]["opacity_compensation"], "none")
            self.assertEqual(manifest["policy"]["alpha_cap"], 0.999)
            self.assertEqual(manifest["policy"]["pixel_alpha_min"], 1.0 / 255.0)
            self.assertEqual(manifest["policy"]["transmittance_epsilon"], 1.0e-4)
            self.assertEqual(manifest["time"]["initial"], 0.0)
            self.assertEqual(manifest["time"]["max_duration"], 0.75)
            self.assertEqual(manifest["provenance"]["initial_normalized_time"], 0.0)
            self.assertNotIn(str(root), json.dumps(manifest))

            payload = (first / "gaussians.bin").read_bytes()
            row0 = struct.unpack_from("<20f", payload, bridge.HEADER_BYTES)
            row1 = struct.unpack_from("<20f", payload, bridge.HEADER_BYTES + bridge.RECORD_BYTES)
            self.assertEqual(row0[0:4], (1.0, 2.0, 3.0, 0.0))
            self.assertEqual(row0[8:12], (0.0, 0.0, 0.0, 1.0))
            self.assertEqual(
                row1[8:12],
                (bridge.f32(math.sqrt(0.5)), 0.0, 0.0, bridge.f32(math.sqrt(0.5))),
            )
            self.assertAlmostEqual(row0[12], 0.4)
            self.assertAlmostEqual(row0[13], 0.8)
            self.assertAlmostEqual(row0[14], 1.2)
            self.assertAlmostEqual(row0[15], -0.1)
            self.assertEqual(row0[19], 0.0)
            self.assertEqual(row1[3], 0.5)
            self.assertEqual(row1[19], 1.0)
            appearance = (first / "sh3.f16").read_bytes()
            self.assertEqual(len(appearance), self.count * bridge.SH3_RECORD_BYTES)
            self.assertEqual(appearance[90:92], b"\0\0")

    def test_linear_source_declares_one_post_composite_srgb_transfer(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle, camera_path, _ = self._write_bundle(root, photometric_space="linear_rgb")
            output = root / "output"
            bridge.convert(bundle, camera_path, output)
            manifest = json.loads((output / "manifest.json").read_text())
            self.assertEqual(
                (manifest["render"]["working_space"], manifest["render"]["output_transfer"]),
                ("linear-rgb", "srgb"),
            )
            validate_asset.validate(output / "manifest.json")

    def test_nonzero_camera_frame_exposes_the_player_initial_time(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle, camera_path, _ = self._write_bundle(root)
            output = root / "output"
            receipt = bridge.convert(bundle, camera_path, output, camera_frame=1)
            manifest = json.loads((output / "manifest.json").read_text())
            self.assertEqual(receipt["initial_normalized_time"], 0.25)
            self.assertEqual(manifest["time"]["initial"], 0.25)
            self.assertEqual(manifest["provenance"]["camera_frame"], 1)
            self.assertEqual(manifest["provenance"]["initial_normalized_time"], 0.25)
            self.assertEqual(
                manifest["camera"]["fixed"]["world_to_camera_row_major"][0][3],
                0.25,
            )

    def test_runtime_ids_are_stable_unique_ids_not_dense_row_numbers(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle, camera_path, _ = self._write_bundle(
                root,
                runtime_ids=[9_000_000_007, 17],
            )
            bridge.convert(bundle, camera_path, root / "output")

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle, camera_path, _ = self._write_bundle(root, runtime_ids=[17, 17])
            output = root / "output"
            with self.assertRaisesRegex(bridge.ContractError, "runtime_ids must be unique"):
                bridge.convert(bundle, camera_path, output)
            self.assertFalse(output.exists())

    def test_gate_scale_tensor_header_and_asset_must_close(self) -> None:
        cases = [
            ({"tensor_gate_scale": 10.0}, "gate_logit_scale tensor"),
            ({"tensor_gate_scale": float("nan")}, "gate_logit_scale tensor"),
            ({"header_gate_scale": 10.0}, "gate scale metadata mismatch"),
            ({"asset_gate_scale": 10.0}, "gate scale metadata mismatch"),
        ]
        for overrides, message in cases:
            with self.subTest(overrides=overrides):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    bundle, camera_path, _ = self._write_bundle(root, **overrides)
                    with self.assertRaisesRegex(bridge.ContractError, message):
                        bridge.convert(bundle, camera_path, root / "output")

    def test_camera_path_validates_every_frame_before_selection(self) -> None:
        cases = [
            ("fps", "camera-path fps is invalid"),
            ("timestamps", "timestamps must be nondecreasing"),
            ("outside", "outside the asset time interval"),
            ("affine", "invalid affine last row"),
            ("orthonormal", "rotation is not orthonormal"),
            ("handedness", "rotation is not right-handed"),
            ("unknown", "missing or unknown fields"),
        ]
        for case, message in cases:
            with self.subTest(case=case):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    bundle, camera_path, _ = self._write_bundle(root)
                    camera = json.loads(camera_path.read_text())
                    if case == "fps":
                        camera["fps"] = 0
                    elif case == "timestamps":
                        camera["frames"][0]["timestamp_seconds"] = 3.0
                        camera["frames"][1]["timestamp_seconds"] = 2.0
                    elif case == "outside":
                        camera["frames"][1]["timestamp_seconds"] = 7.0
                    elif case == "affine":
                        camera["frames"][1]["world_to_camera"][3][3] = 2.0
                    elif case == "orthonormal":
                        camera["frames"][1]["world_to_camera"][0][0] = 2.0
                    elif case == "handedness":
                        camera["frames"][1]["world_to_camera"][0][0] = -1.0
                    else:
                        camera["unexpected"] = True
                    camera_path.write_bytes(self._canonical(camera))
                    output = root / "output"
                    with self.assertRaisesRegex(bridge.ContractError, message):
                        bridge.convert(bundle, camera_path, output, camera_frame=0)
                    self.assertFalse(output.exists())

    def test_bundle_and_camera_symlinks_are_rejected_before_resolution(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle, camera_path, _ = self._write_bundle(root)
            bundle_link = root / "bundle-link"
            bundle_link.symlink_to(bundle, target_is_directory=True)
            with self.assertRaisesRegex(bridge.ContractError, "bundle must not be a symlink"):
                bridge.convert(bundle_link, camera_path, root / "bundle-link-output")

            camera_link = root / "camera-link.json"
            camera_link.symlink_to(camera_path)
            with self.assertRaisesRegex(bridge.ContractError, "camera path must not be a symlink"):
                bridge.convert(bundle, camera_link, root / "camera-link-output")

            output_link = root / "output-link"
            output_link.symlink_to(root / "missing-output", target_is_directory=True)
            with self.assertRaisesRegex(bridge.ContractError, "overwrite"):
                bridge.convert(bundle, camera_path, output_link)

    def test_bundle_manifest_leaf_symlink_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle, camera_path, _ = self._write_bundle(root)
            manifest_path = bundle / "manifest.json"
            manifest_real = root / "manifest-real.json"
            manifest_real.write_bytes(manifest_path.read_bytes())
            manifest_path.unlink()
            manifest_path.symlink_to(manifest_real)
            with self.assertRaisesRegex(bridge.ContractError, "manifest must be a regular file"):
                bridge.convert(bundle, camera_path, root / "output")

    def test_reparameterized_duration_has_a_strict_finite_upper_bound(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle, camera_path, _ = self._write_bundle(
                root,
                duration_logits=[100.0, 100.0],
                duration_max_seconds=[0.5, 0.75],
            )
            receipt = bridge.convert(bundle, camera_path, root / "output")
            self.assertEqual(receipt["duration_mapping"], "physical-sigma-reparameterized")
            self.assertLess(receipt["duration_normalized_max_abs_error"], 1.0e-6)

    def test_atomic_publish_never_replaces_an_existing_empty_directory(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            staging = root / "staging"
            output = root / "output"
            staging.mkdir()
            output.mkdir()
            (staging / "payload").write_bytes(b"new")

            with self.assertRaisesRegex(bridge.ContractError, "overwrite"):
                bridge.publish_directory_noreplace(staging, output)

            self.assertTrue(staging.is_dir())
            self.assertTrue((staging / "payload").is_file())
            self.assertEqual(list(output.iterdir()), [])

    def test_bridge_rejects_tampering_mismatched_camera_and_overwrite(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle, camera_path, _ = self._write_bundle(root)
            output = root / "output"
            bridge.convert(bundle, camera_path, output)
            with self.assertRaisesRegex(bridge.ContractError, "overwrite"):
                bridge.convert(bundle, camera_path, output)

            camera = json.loads(camera_path.read_text())
            camera["asset_bundle_id"] = "0" * 64
            camera_path.write_bytes(self._canonical(camera))
            with self.assertRaisesRegex(bridge.ContractError, "different AssetBundle"):
                bridge.convert(bundle, camera_path, root / "other")

            model = bundle / "model.safetensors"
            model.write_bytes(model.read_bytes() + b"x")
            with self.assertRaisesRegex(bridge.ContractError, "byte count mismatch"):
                bridge.convert(bundle, camera_path, root / "tampered")

    def test_bridge_rejects_duration_below_player_floor_without_partial_output(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle, camera_path, _ = self._write_bundle(root, tiny_duration=True)
            output = root / "output"
            with self.assertRaisesRegex(bridge.ContractError, "below the player floor"):
                bridge.convert(bundle, camera_path, output)
            self.assertFalse(output.exists())
            self.assertFalse(any(root.glob(".output.tmp-*")))


if __name__ == "__main__":
    unittest.main()
