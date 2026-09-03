# Validation model

Each gate answers a different question. Passing one must not be reported as passing the others.

| Gate | What it proves | What it does not prove |
| --- | --- | --- |
| AssetBundle bridge tests | Source hash closure, tensor/camera validation and deterministic semantic mapping agree | GPU rendering correctness |
| Asset semantic tests | Manifest, hashes, binary layout and negative cases agree | GPU rendering correctness |
| JSON Schema check | Checked-in manifests satisfy the published structural contract | Binary payload semantics |
| Evidence receipt Schema check | A reference or comparison receipt has the exact v1 field structure and JSON types | Cross-field equality, referenced frame bytes, or visual review |
| Rust tests | Runtime loader, WGSL closure, control logic and HTTP bounds compile and pass | AMD interop or browser presentation |
| Lesson build/tests | All seven entries own their command chain, use relative URLs, recursively contain no model/media payload and bundle successfully | A WebGPU adapter executed the frame |
| Lesson hardware Chrome smoke | Lessons 00–06 each publish `window.__LESSON_RESULT__.status === "PASS"` on a controlled real GPU adapter | Player correctness or production-scale performance |
| AMD one-frame evidence | Vulkan render and VA-API color roundtrip agree with a reviewed reference | Interactive WebRTC behavior |
| Cross-render one-frame comparison | Pixel4DGS and Phi agree numerically at the same model, camera, time and resolution | Temporal playback or network behavior |
| Player Chrome session | End-to-end encode, WebRTC, presentation and input work | Multi-user or Internet deployment |

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
npx vite build --base=/4dgs-viewer-Phi/ --outDir=dist-pages
```

## WebGPU lesson checks

Run `npm run dev:open` from `lessons/` to open the course catalog. Each page
publishes its result at `window.__LESSON_RESULT__` after these checks complete:

| Lesson | Runtime check |
| --- | --- |
| 00 | Adapter/device creation, WGSL compilation, pipeline creation and completed submission |
| 01 | Analytic Gaussian alpha agrees between CPU reference and a WGSL compute readback |
| 02 | Four projection cases and their conics agree between CPU and GPU |
| 03 | Near-to-far order is monotonic and a rendered center pixel matches front-to-back CPU compositing |
| 04 | The 32-byte time-uniform layout, static/moving invariants and explicit-time evaluations agree between CPU and WGSL |
| 05 | GPU counters, compacted indices and indirect instance count match the CPU active-set reference |
| 06 | Procedural input validation, projection, deterministic order and compositing agree across the complete pipeline |

Lessons use constants or procedural records, so browser validation performs no
external model or media request. `npm test` and the two production builds are
portable structural checks; the browser result additionally requires a real
WebGPU adapter.

`surface.pass(details, assertions)` accepts only a non-empty assertion object
whose values are all exactly `true`. Any other value publishes a structured
`FAIL` result and throws; a truthy value cannot produce a false PASS. The same
result is available in three forms:

```text
window.__LESSON_RESULT__
html[data-lesson-status="PASS|FAIL"]
script#lesson-result[type="application/json"]
```

The hosted GitHub Actions job performs source-contract tests and production
builds, but does not claim WebGPU execution. Run the hardware Chrome smoke
locally or on a controlled GPU browser runner and record the adapter alongside
the seven structured results.

Player source tests additionally require the Linux native packages listed in `player/README.md`:

```bash
cd player
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
```

## Pixel4DGS bridge validation

The bridge workflow is documented in
[`P2G_ASSET_BRIDGE.md`](P2G_ASSET_BRIDGE.md). A source-render comparison must
use the same AssetBundle, camera frame, normalized time, output dimensions,
raster profile and photometric path. Keep the independently produced source
frame distinct from a Phi `--write-golden` capture; relabeling one receipt as
the other would turn a cross-implementation test into a self-reference.

At minimum preserve these identities beside a private comparison:

- source bundle and model SHA-256;
- camera-path SHA-256, selected frame and timestamp;
- source renderer implementation/runtime identity;
- converted manifest, geometry and appearance SHA-256;
- Phi source and shader bundle identity;
- both raw RGBA8 hashes, dimensions and numerical metrics.

Restricted real-world assets and their frames remain outside the repository.
Only generic tools, Schemas and synthetic examples may be committed.

For the initial 1280 x 720 Pixel4DGS/Phi closure, declare and run the image
gate before reading its metrics:

```bash
python3 tools/compare_rgba8.py SOURCE.rgba8 PHI.rgba8 \
  --width 1280 --height 720 \
  --min-psnr-db 40 --max-mean-abs 1 --max-rmse 2.55 \
  --output cross-render-comparison.json
```

The tool additionally requires opaque alpha and writes a non-overwriting,
SHA-256-bound JSON receipt. Exit `0` is a pass, `1` is a declared metric failure
and `2` is invalid input. At least one RGB threshold or `--require-rgb-exact`
is mandatory, so alpha opacity alone cannot create a passing receipt. A
threshold must not be relaxed after seeing a failed result and then reported as
the original gate.

A golden must be created as an explicit, non-overwriting action and visually
reviewed before it becomes a comparison input.

The receipt Schema checks structure, ranges, fixed policy values and
discriminator fields, while the evidence procedure checks relationships
that standard JSON Schema cannot express. In particular, verify that:

- `source.shader_bundle_sha256` equals `frame.shader_bundle_sha256`;
- frame, media and raw RGBA8 dimensions/byte counts agree;
- the raw reference hashes to `reference.rgba8_sha256`, or the compared files
  hash to `image.golden_sha256` and `image.actual_sha256`;
- revision-bound evidence has a non-null `source.git_commit` naming the tested
  commit;
- a human inspected the converted PNG before changing `review_status` from
  `UNREVIEWED` to `REVIEWED`.

The comparison executable loads the neighboring receipt and, before rendering,
enforces reference-v1, `REVIEWED`, raw byte/hash, asset identity and requested
frame identity. It does not compare the reference's source hash
with the current source hash: cross-version comparison is the purpose of this
gate. Runtime parsing covers only that enforcement subset, so the strict Schema
command remains mandatory for complete structure and duplicate-key checking.
Schema success alone is not a visual verdict.
