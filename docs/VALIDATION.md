# Validation model

Each gate answers a different question. Passing one must not be reported as passing the others.

| Gate | What it proves | What it does not prove |
| --- | --- | --- |
| Asset semantic tests | Manifest, hashes, binary layout and negative cases agree | GPU rendering correctness |
| JSON Schema check | Checked-in manifests satisfy the published structural contract | Binary payload semantics |
| Evidence receipt Schema check | A reference or comparison receipt has the exact v1 field structure and JSON types | Cross-field equality, referenced frame bytes, or visual review |
| Rust tests | Runtime loader, WGSL closure, control logic and HTTP bounds compile and pass | AMD interop or browser presentation |
| Lesson build/tests | URLs, lesson contract and bundles are structurally valid | A WebGPU adapter executed the frame |
| Lesson Chrome smoke | `window.__LESSON_RESULT__.status === "PASS"` on real WebGPU | Player correctness |
| AMD one-frame evidence | Vulkan render and VA-API color roundtrip agree with a reviewed reference | Interactive WebRTC behavior |
| Player Chrome session | End-to-end encode, WebRTC, presentation and input work | Multi-user or Internet deployment |
| Release binary path scan | The stripped CI binary omits known host/workspace paths | Bit-for-bit reproducibility or binary redistribution approval |
| Dependency evidence | Checked-in locks can be enumerated and npm emits a CycloneDX build SBOM | License compatibility or runtime/system-package completeness |

## Portable commands

```bash
python3 tools/audit_public_tree.py
python3 tools/generate_synthetic_asset.py --check
python3 -m unittest discover -s tests -v
python3 tools/validate_asset.py examples/minimal-sh0/manifest.json examples/synthetic-motion-sh3/manifest.json
python3 -m pip install --require-hashes -r tools/requirements-schema.lock
python3 tools/check_json_schema.py examples/minimal-sh0/manifest.json examples/synthetic-motion-sh3/manifest.json
python3 tools/check_json_schema.py --schema evidence/remote-native-evidence-v1.schema.json --schema-only
python3 tools/check_json_schema.py \
  --schema evidence/remote-native-evidence-v1.schema.json \
  evidence/fixtures/reference-v1.example.json evidence/fixtures/comparison-v1.example.json

# Validate one or more player reference/comparison receipts.
python3 tools/check_json_schema.py \
  --schema evidence/remote-native-evidence-v1.schema.json \
  path/to/reference.rgba8.json path/to/receipt.json

cd lessons
npm ci
npm test
npm run build
```

Player source tests additionally require the Linux native packages listed in `player/README.md`:

```bash
cd player
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
```

The official Linux job uses an isolated `CARGO_HOME` and target directory,
derives `SOURCE_DATE_EPOCH` from the commit, and remaps both the checkout and
temporary roots before compiling. After stripping, it scans the executable for
`/Users/`, `/home/`, the checkout root and the runner temporary root. The job
uploads only the scan result, toolchain versions and binary digest—not the
binary itself.

All GitHub Actions used by the workflow are restricted by the public-tree audit
to a short first-party allowlist and immutable 40-character commits. A moving
major-version tag is not accepted by that gate.

Dependency evidence is generated without adding another SBOM package to the
project's dependency graph:

```bash
python3 tools/generate_dependency_inventory.py --output /tmp/lockfile-inventory.json

cd lessons
npm sbom --sbom-format cyclonedx > /tmp/lessons.cdx.json
```

The first file is a deterministic, path-free inventory of the Cargo, npm and
Python lock files. The second is npm's CycloneDX build SBOM. Both are CI
artifacts for review; neither closes the dependency-license review by itself.

Hardware evidence and browser receipts belong in CI artifacts. A golden may be created only as an explicit, non-overwriting action and must be visually reviewed before it becomes a comparison input.

The receipt Schema deliberately checks structure, ranges, fixed policy values
and discriminator fields, while the evidence procedure checks relationships
that standard JSON Schema cannot express. In particular, verify that:

- `source.shader_bundle_sha256` equals `frame.shader_bundle_sha256`;
- frame, media and raw RGBA8 dimensions/byte counts agree;
- the raw reference hashes to `reference.rgba8_sha256`, or the compared files
  hash to `image.golden_sha256` and `image.actual_sha256`;
- release evidence has a non-null `source.git_commit` naming the tested commit;
- a human inspected the converted PNG before changing `review_status` from
  `UNREVIEWED` to `REVIEWED`.

The comparison executable loads the neighboring receipt and, before rendering,
enforces reference-v1, `REVIEWED`, raw byte/hash, asset identity and requested
frame identity. It intentionally does not compare the reference's source hash
with the current source hash: cross-version comparison is the purpose of this
gate. Runtime parsing covers only that enforcement subset, so the strict Schema
command remains mandatory for complete structure and duplicate-key checking.
Schema success alone is not a visual verdict.
