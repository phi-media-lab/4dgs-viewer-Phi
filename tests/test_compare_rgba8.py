from __future__ import annotations

import hashlib
import json
import math
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from tools import compare_rgba8


class CompareRgba8Test(unittest.TestCase):
    def test_exact_metrics_are_strict_json_and_hash_bound(self) -> None:
        payload = bytes([0, 10, 255, 255, 40, 50, 60, 255])
        receipt = compare_rgba8.compare_payloads(
            payload,
            payload,
            width=2,
            height=1,
            require_rgb_exact=True,
        )

        self.assertEqual(receipt["status"], "PASS")
        self.assertTrue(receipt["metrics"]["rgb_exact"])
        self.assertEqual(receipt["metrics"]["changed_pixels"], 0)
        self.assertEqual(receipt["metrics"]["changed_pixel_ratio"], 0.0)
        self.assertEqual(receipt["metrics"]["rgb"]["max_abs"], 0)
        self.assertEqual(receipt["metrics"]["rgb"]["rmse"], 0.0)
        self.assertIsNone(receipt["metrics"]["rgb"]["psnr_db"])
        self.assertTrue(receipt["metrics"]["rgb"]["psnr_infinite"])
        digest = hashlib.sha256(payload).hexdigest()
        self.assertEqual(receipt["inputs"]["golden"]["sha256"], digest)
        self.assertEqual(receipt["inputs"]["actual"]["sha256"], digest)
        encoded = compare_rgba8.canonical_json_bytes(receipt)
        self.assertEqual(json.loads(encoded), receipt)
        self.assertNotIn(b"NaN", encoded)
        self.assertNotIn(b"Infinity", encoded)

    def test_rgb_metrics_and_thresholds_cover_pixels_and_channels(self) -> None:
        golden = bytes([0, 10, 20, 255, 100, 110, 120, 255])
        actual = bytes([1, 10, 18, 255, 100, 114, 120, 255])
        receipt = compare_rgba8.compare_payloads(
            golden,
            actual,
            width=2,
            height=1,
            min_psnr_db=40.0,
            max_abs=3,
            max_mean_abs=1.0,
            max_rmse=2.0,
            max_changed_pixel_ratio=1.0,
        )

        self.assertEqual(receipt["status"], "FAIL")
        self.assertFalse(receipt["metrics"]["rgb_exact"])
        self.assertEqual(receipt["metrics"]["changed_pixels"], 2)
        self.assertEqual(receipt["metrics"]["changed_pixel_ratio"], 1.0)
        self.assertEqual(receipt["metrics"]["rgb"]["max_abs"], 4)
        self.assertAlmostEqual(receipt["metrics"]["rgb"]["mean_abs"], 7 / 6)
        self.assertAlmostEqual(receipt["metrics"]["rgb"]["rmse"], math.sqrt(21 / 6))
        self.assertEqual(receipt["metrics"]["channels"]["r"]["max_abs"], 1)
        self.assertEqual(receipt["metrics"]["channels"]["g"]["max_abs"], 4)
        self.assertEqual(receipt["metrics"]["channels"]["b"]["max_abs"], 2)
        failed = [check["check"] for check in receipt["checks"] if not check["passed"]]
        self.assertEqual(failed, ["maximum-absolute-error", "maximum-mean-absolute-error"])

    def test_non_opaque_alpha_is_an_unconditional_failure(self) -> None:
        golden = bytes([0, 0, 0, 254])
        actual = bytes([0, 0, 0, 255])
        receipt = compare_rgba8.compare_payloads(
            golden,
            actual,
            width=1,
            height=1,
            require_rgb_exact=True,
        )

        self.assertEqual(receipt["status"], "FAIL")
        self.assertEqual(receipt["alpha"]["golden_non_opaque_pixels"], 1)
        self.assertEqual(receipt["alpha"]["actual_non_opaque_pixels"], 0)
        self.assertTrue(receipt["metrics"]["rgb_exact"])

    def test_files_reject_wrong_length_non_regular_and_symlink_inputs(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            valid = root / "valid.rgba8"
            short = root / "short.rgba8"
            link = root / "link.rgba8"
            valid.write_bytes(bytes([0, 0, 0, 255]))
            short.write_bytes(b"\x00\x00\x00")
            link.symlink_to(valid)

            with self.assertRaisesRegex(compare_rgba8.ComparisonError, "byte length mismatch"):
                compare_rgba8.compare_files(short, valid, width=1, height=1)
            with self.assertRaisesRegex(compare_rgba8.ComparisonError, "regular file"):
                compare_rgba8.compare_files(root, valid, width=1, height=1)
            with self.assertRaisesRegex(compare_rgba8.ComparisonError, "must not be a symlink"):
                compare_rgba8.compare_files(link, valid, width=1, height=1)

    def test_cli_exit_codes_and_receipt_output_is_exclusive(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            golden = root / "golden.rgba8"
            actual = root / "actual.rgba8"
            receipt_path = root / "receipt.json"
            golden.write_bytes(bytes([0, 0, 0, 255]))
            actual.write_bytes(bytes([1, 0, 0, 255]))
            command = [
                sys.executable,
                str(Path(compare_rgba8.__file__)),
                str(golden),
                str(actual),
                "--width",
                "1",
                "--height",
                "1",
                "--max-abs",
                "0",
                "--output",
                str(receipt_path),
            ]
            environment = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1"}

            first = subprocess.run(command, capture_output=True, check=False, env=environment)
            self.assertEqual(first.returncode, 1, first.stderr.decode())
            self.assertEqual(first.stdout, receipt_path.read_bytes())
            self.assertEqual(json.loads(first.stdout)["status"], "FAIL")

            original = receipt_path.read_bytes()
            second = subprocess.run(command, capture_output=True, check=False, env=environment)
            self.assertEqual(second.returncode, 2)
            self.assertIn(b"output must be a new file", second.stderr)
            self.assertEqual(receipt_path.read_bytes(), original)

            passing = subprocess.run(
                [
                    sys.executable,
                    str(Path(compare_rgba8.__file__)),
                    str(golden),
                    str(golden),
                    "--width",
                    "1",
                    "--height",
                    "1",
                    "--require-rgb-exact",
                ],
                capture_output=True,
                check=False,
                env=environment,
            )
            self.assertEqual(passing.returncode, 0, passing.stderr.decode())
            self.assertEqual(json.loads(passing.stdout)["status"], "PASS")

            unassessed = subprocess.run(
                [
                    sys.executable,
                    str(Path(compare_rgba8.__file__)),
                    str(golden),
                    str(actual),
                    "--width",
                    "1",
                    "--height",
                    "1",
                ],
                capture_output=True,
                check=False,
                env=environment,
            )
            self.assertEqual(unassessed.returncode, 2)
            self.assertIn(b"at least one RGB acceptance threshold", unassessed.stderr)


if __name__ == "__main__":
    unittest.main()
