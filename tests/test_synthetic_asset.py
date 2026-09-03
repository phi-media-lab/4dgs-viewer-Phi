from __future__ import annotations

import hashlib
import json
import math
import struct
import tempfile
import unittest
from pathlib import Path

from tools import generate_synthetic_asset as generate
from tools import validate_asset


class SyntheticAssetTest(unittest.TestCase):
    def generate_tree(self, root: Path) -> None:
        generate.write_asset(
            root, "minimal-sh0", generate.minimal_records(), sh3=False, seed=17
        )
        generate.write_asset(
            root,
            "synthetic-motion-sh3",
            generate.motion_records(96, 17),
            sh3=True,
            seed=17,
        )

    @staticmethod
    def tree_digest(root: Path) -> dict[str, str]:
        return {
            str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(root.rglob("*"))
            if path.is_file()
        }

    def test_generation_is_byte_deterministic(self) -> None:
        with (
            tempfile.TemporaryDirectory() as first,
            tempfile.TemporaryDirectory() as second,
        ):
            self.generate_tree(Path(first))
            self.generate_tree(Path(second))
            self.assertEqual(
                self.tree_digest(Path(first)), self.tree_digest(Path(second))
            )

    def test_generated_sh0_and_sh3_assets_validate(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.generate_tree(root)
            sh0 = validate_asset.validate(root / "minimal-sh0/manifest.json")
            sh3 = validate_asset.validate(root / "synthetic-motion-sh3/manifest.json")
            self.assertEqual(sh0["color"], "raw-sh0")
            self.assertEqual(sh3["color"], "raw-sh3")
            self.assertEqual(sh3["gaussian_count"], 96)

    def test_checked_in_assets_validate_and_match_default_generation(self) -> None:
        root = Path(__file__).resolve().parents[1]
        checked_in = root / "examples"
        self.assertEqual(
            validate_asset.validate(checked_in / "minimal-sh0/manifest.json")[
                "gaussian_count"
            ],
            3,
        )
        self.assertEqual(
            validate_asset.validate(checked_in / "synthetic-motion-sh3/manifest.json")[
                "gaussian_count"
            ],
            4096,
        )
        with tempfile.TemporaryDirectory() as directory:
            generated = Path(directory)
            generate.write_asset(
                generated,
                "minimal-sh0",
                generate.minimal_records(),
                sh3=False,
                seed=20260903,
            )
            generate.write_asset(
                generated,
                "synthetic-motion-sh3",
                generate.motion_records(4096, 20260903),
                sh3=True,
                seed=20260903,
            )
            for relative, digest in self.tree_digest(generated).items():
                self.assertEqual(
                    hashlib.sha256((checked_in / relative).read_bytes()).hexdigest(),
                    digest,
                    relative,
                )

    def test_camera_is_fixed_and_positive_z_scene_is_finite(self) -> None:
        records = generate.motion_records(32, 9)
        self.assertTrue(
            all(all(math.isfinite(value) for value in record) for record in records)
        )
        self.assertTrue(all(record[2] > 0.05 for record in records))
        camera = generate.fixed_camera()
        self.assertEqual(camera["world_to_camera_row_major"][2][2], 1.0)
        self.assertLess(camera["near"], min(record[2] for record in records))

    def test_calibration_regions_have_exact_stable_allocation(self) -> None:
        groups = generate.calibration_group_counts(4096)
        self.assertEqual(
            groups,
            dict(generate.CALIBRATION_GROUP_WEIGHTS),
        )
        self.assertEqual(sum(groups.values()), 4096)
        small = generate.calibration_group_counts(96)
        self.assertEqual(sum(small.values()), 96)
        self.assertTrue(all(value > 0 for value in small.values()))

    def test_calibration_target_exercises_depth_motion_and_gate(self) -> None:
        count = 4096
        records = generate.motion_records(count, 20260903)

        axes = generate.calibration_group_range(count, "camera-axes")
        self.assertTrue(
            any(
                abs(records[index][4] - records[index][5]) > 0.1
                and abs(records[index][10]) > 0.01
                for index in axes
            )
        )

        depth = generate.calibration_group_range(count, "depth-order")
        first_depths = [records[index][2] for index in list(depth)[:3]]
        self.assertGreater(first_depths[0], first_depths[1])
        self.assertGreater(first_depths[1], first_depths[2])
        self.assertGreater(max(records[index][2] for index in depth), 3.5)
        self.assertLess(min(records[index][2] for index in depth), 2.8)

        timeline = generate.calibration_group_range(count, "timeline")
        moving = [records[index] for index in timeline if abs(records[index][12]) > 1.0]
        gated = [records[index] for index in timeline if records[index][15] < 0.0]
        self.assertTrue(moving)
        self.assertTrue(gated)
        self.assertEqual({round(item[3], 1) for item in gated}, {0.2, 0.5, 0.8})

    def test_only_sh_probe_region_has_nonconstant_coefficients(self) -> None:
        count = 4096
        records = generate.motion_records(count, 20260903)
        appearance = generate.encode_sh3(records, 20260903)
        sh_region = generate.calibration_group_range(count, "sh-view")
        first_sh_record = sh_region.start * generate.SH3_RECORD_BYTES
        self.assertEqual(
            appearance[:first_sh_record],
            bytes(first_sh_record),
        )
        nonzero_degrees = set()
        for index in sh_region:
            offset = index * generate.SH3_RECORD_BYTES
            values = struct.unpack_from("<45e", appearance, offset)
            for coefficient, value in enumerate(values):
                if value != 0.0:
                    nonzero_degrees.add(
                        1 if coefficient // 3 < 3 else 2 if coefficient // 3 < 8 else 3
                    )
            self.assertEqual(appearance[offset + 90 : offset + 92], b"\0\0")
        self.assertEqual(nonzero_degrees, {1, 2, 3})

    def test_manifest_provenance_maps_every_calibration_region(self) -> None:
        records = generate.motion_records(4096, 20260903)
        geometry = generate.encode_geometry(records)
        appearance = generate.encode_sh3(records, 20260903)
        manifest = generate.manifest(
            "synthetic-motion-sh3",
            geometry,
            len(records),
            appearance=appearance,
            seed=20260903,
        )
        purpose = manifest["provenance"]["purpose"]
        self.assertEqual(purpose["id"], "4d-calibration-target")
        declared = {
            region["id"]: region["gaussian_count"] for region in purpose["regions"]
        }
        self.assertEqual(declared, generate.calibration_group_counts(4096))
        self.assertTrue(all(region["invariant"] for region in purpose["regions"]))

    def test_payload_hash_mismatch_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.generate_tree(root)
            payload = root / "minimal-sh0/gaussians.bin"
            payload.write_bytes(payload.read_bytes() + b"x")
            with self.assertRaisesRegex(ValueError, "byte count mismatch"):
                validate_asset.validate(root / "minimal-sh0/manifest.json")

    def test_non_finite_record_is_rejected_even_with_updated_hash(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.generate_tree(root)
            asset = root / "minimal-sh0"
            payload_path = asset / "gaussians.bin"
            payload = bytearray(payload_path.read_bytes())
            struct.pack_into("<f", payload, generate.HEADER_BYTES, math.nan)
            payload_path.write_bytes(payload)
            manifest_path = asset / "manifest.json"
            manifest = json.loads(manifest_path.read_text())
            manifest["binary"]["sha256"] = hashlib.sha256(payload).hexdigest()
            manifest_path.write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, "non-finite"):
                validate_asset.validate(manifest_path)

    def test_shader_unsafe_record_is_rejected_even_with_updated_hash(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.generate_tree(root)
            asset = root / "minimal-sh0"
            payload_path = asset / "gaussians.bin"
            payload = bytearray(payload_path.read_bytes())
            struct.pack_into("<f", payload, generate.HEADER_BYTES, 1.0e30)
            payload_path.write_bytes(payload)
            manifest_path = asset / "manifest.json"
            manifest = json.loads(manifest_path.read_text())
            manifest["binary"]["sha256"] = hashlib.sha256(payload).hexdigest()
            manifest_path.write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, "shader-unsafe"):
                validate_asset.validate(manifest_path)

    def test_sh3_padding_is_rejected_even_with_updated_hash(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.generate_tree(root)
            asset = root / "synthetic-motion-sh3"
            payload_path = asset / "sh3.f16"
            payload = bytearray(payload_path.read_bytes())
            payload[90] = 1
            payload_path.write_bytes(payload)
            manifest_path = asset / "manifest.json"
            manifest = json.loads(manifest_path.read_text())
            manifest["appearance"]["sha256"] = hashlib.sha256(payload).hexdigest()
            manifest_path.write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, "padding"):
                validate_asset.validate(manifest_path)

    def test_payload_path_escape_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.generate_tree(root)
            manifest_path = root / "minimal-sh0/manifest.json"
            manifest = json.loads(manifest_path.read_text())
            manifest["binary"]["uri"] = "../outside.bin"
            manifest_path.write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, "escapes asset directory"):
                validate_asset.validate(manifest_path)

    def assert_manifest_mutation_rejected(
        self,
        mutate,
        message: str,
        *,
        asset_name: str = "minimal-sh0",
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.generate_tree(root)
            manifest_path = root / asset_name / "manifest.json"
            manifest = json.loads(manifest_path.read_text())
            mutate(manifest)
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, message):
                validate_asset.validate(manifest_path)

    def test_manifest_semantics_reject_schema_invalid_values(self) -> None:
        cases = [
            (lambda value: value.__setitem__("name", 7), "name must be"),
            (
                lambda value: value["representation"].__setitem__(
                    "rotation", "normalized-wxyz"
                ),
                "unsupported rotation",
            ),
            (
                lambda value: value["policy"].__setitem__(
                    "temporal_threshold", "0.002"
                ),
                "temporal_threshold",
            ),
            (
                lambda value: value["render"].__setitem__(
                    "working_space", "linear-srgb"
                ),
                "working_space",
            ),
            (
                lambda value: value["render"].__setitem__("background", [0]),
                "background",
            ),
            (lambda value: value.__setitem__("unknown", True), "unknown fields"),
            (
                lambda value: value.__setitem__("provenance", None),
                "provenance must be an object",
            ),
            (
                lambda value: value["camera"]["fixed"]["world_to_camera_row_major"][
                    0
                ].__setitem__(0, 2.0),
                "right-handed rigid affine",
            ),
            (
                lambda value: value["render"]["background"].__setitem__(3, 0.5),
                "alpha must be exactly 1",
            ),
        ]
        for mutate, message in cases:
            with self.subTest(message=message):
                self.assert_manifest_mutation_rejected(mutate, message)

        self.assert_manifest_mutation_rejected(
            lambda value: value["appearance"].__setitem__("encoding", "float32"),
            "appearance encoding",
            asset_name="synthetic-motion-sh3",
        )

    def test_non_standard_and_duplicate_json_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.generate_tree(root)
            manifest_path = root / "minimal-sh0/manifest.json"
            text = manifest_path.read_text(encoding="utf-8")
            manifest_path.write_text(
                text.replace('"max_duration": 1.2', '"max_duration": Infinity')
            )
            with self.assertRaisesRegex(ValueError, "strict JSON"):
                validate_asset.validate(manifest_path)

            manifest_path.write_text(
                text.replace('"version": 1,', '"version": 1,\n  "version": 1,')
            )
            with self.assertRaisesRegex(ValueError, "duplicate JSON key"):
                validate_asset.validate(manifest_path)


if __name__ == "__main__":
    unittest.main()
