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
