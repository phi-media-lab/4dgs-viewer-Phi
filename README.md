# 4DGS Viewer Phi

4DGS Viewer Phi contains two independently runnable components:

- `player/`: a Linux Remote Frame Mode Player built with Rust, wgpu, WGSL,
  Vulkan, DMA-BUF, VA-API and WebRTC;
- `lessons/`: seven first-principles WebGPU + WGSL lessons covering the path
  from device creation to a complete synthetic 4DGS rendering pipeline.

The Player renders explicit 4D Gaussian assets on a Linux GPU and sends encoded
frames to a thin browser receiver. The course exposes its JavaScript host code,
WGSL stages, GPU commands and numerical checks directly. VS Code is the code
surface and the browser is the rendering surface. Training and client-side
Gaussian streaming are outside this repository. A deterministic offline bridge
imports a verified Pixel4DGS AssetBundle into the Player's explicit-v1 format.

## WebGPU course

Requirements: Node.js `^20.19.0` or `>=22.12.0` and a WebGPU-capable
Chrome/Chromium build.

```bash
code-insiders lessons/4dgs-viewer-phi.code-workspace
cd lessons
npm ci
npm run dev:open
```

The catalog at `http://127.0.0.1:5173/` links to all seven lessons:

```text
00 Environment       WebGPU device, shader, pipeline and command submission
01 One Gaussian      Analytic Gaussian footprint and CPU/WGSL agreement
02 Projection        3D covariance, camera Jacobian and 2D conic
03 Order and blend   Correct and deliberately reversed transparent order
04 Explicit time     Static/moving primitives and temporal opacity
05 Active set        GPU active/visible compaction and indirect draw
06 Complete pipeline Validation, projection, sorting and rendering together
```

Every lesson owns its WebGPU pipeline and provides a falsifiable
`window.__LESSON_RESULT__`. Course inputs are source constants or procedural
JavaScript records; running the lessons requires no model, media file or
external asset request. Vite updates the browser after source changes without a
manual refresh. See
[`lessons/README.md`](lessons/README.md) for the course and verification
commands.

## Remote Frame Mode Player

The reference renderer profile is Ubuntu 24.04 x86_64 with AMD RADV/VA-API and
GStreamer 1.24. macOS is receiver-only; other renderer GPUs are unverified and
Windows is out of scope.

After installing the packages listed in [`player/README.md`](player/README.md):

```bash
cd player
cargo test --locked
./scripts/run.sh
```

The server listens on `127.0.0.1:4191`. For a receiver on another machine,
forward signaling with SSH and open the forwarded URL in Chrome:

```bash
ssh -L 4192:127.0.0.1:4191 user@renderer-host
```

```text
http://127.0.0.1:4192/?jitter_buffer_ms=browser
```

The SSH tunnel carries HTTP signaling only. WebRTC media and controls still
require a direct UDP path from the browser to the renderer; see
[`player/README.md`](player/README.md#run-the-webrtc-player).

The browser sends camera and time controls while the Linux process owns
rendering, encoding and frame scheduling. The GPU frame crosses the
Vulkan/VA-API boundary as a linear DMA-BUF; unsupported interop fails instead
of silently copying pixels through the CPU.

## Asset conformance

The repository includes a strict explicit-4DGS asset format and two
deterministic synthetic examples:

```bash
python3 tools/generate_synthetic_asset.py --check
python3 -m unittest discover -s tests -v
python3 tools/validate_asset.py \
  examples/minimal-sh0/manifest.json \
  examples/synthetic-motion-sh3/manifest.json
```

The full contract is documented in
[`asset-format/explicit-v1.md`](asset-format/explicit-v1.md). The calibration
regions and their expected visual invariants are described in
[`examples/README.md`](examples/README.md).

### Import a Pixel4DGS AssetBundle

The bridge accepts an inference-only `p2g.asset_bundle.v1` directory and its
hash-bound `p2g.camera_path.v1`; it does not accept a training checkpoint:

```bash
python3 tools/convert_p2g_asset.py \
  /path/to/asset-bundle-v1 \
  /path/to/camera_path.json \
  /new/private/output-directory \
  --name my-4dgs-asset
```

It verifies the source hash closure and tensor/camera semantics before writing,
refuses to overwrite an existing output, maps the Pixel4DGS classic raster ABI
explicitly, and stores the selected normalized timestamp as manifest
`time.initial` (also repeated in the conversion receipt).
Source redistribution restrictions remain attached to the output provenance;
the repository contains no converted third-party model. See
[`docs/P2G_ASSET_BRIDGE.md`](docs/P2G_ASSET_BRIDGE.md).

## Repository map

```text
player/        Remote renderer and thin WebRTC browser receiver
lessons/       WebGPU course source and development environment
asset-format/  Explicit 4DGS manifest and binary contract
examples/      Deterministic synthetic conformance assets
tools/         Asset conversion, comparison and Schema validation
evidence/      Native reference/comparison receipt Schema
docs/          Architecture, platform support and validation model
```

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md),
[`docs/SUPPORTED_PLATFORMS.md`](docs/SUPPORTED_PLATFORMS.md) and
[`docs/VALIDATION.md`](docs/VALIDATION.md) for the complete technical boundary.
