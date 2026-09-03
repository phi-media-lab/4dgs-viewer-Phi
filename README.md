# 4DGS Viewer Phi

4DGS Viewer Phi contains two independently runnable components:

- `player/`: a Linux Remote Frame Mode Player built with Rust, wgpu, WGSL,
  Vulkan, DMA-BUF, VA-API and WebRTC;
- `lessons/`: a first-principles WebGPU + WGSL course, currently containing
  Lesson 00, in which VS Code is the code surface and the browser is the
  rendering surface.

The Player renders explicit 4D Gaussian assets on a Linux GPU and sends encoded
frames to a thin browser receiver. Lesson 00 exposes the minimal WebGPU host and
shader chain directly as an RGB triangle. Training, model
conversion and client-side Gaussian streaming are outside this repository.

## WebGPU Lesson 00

Requirements: Node.js `^20.19.0` or `>=22.12.0` and a WebGPU-capable
Chrome/Chromium build.

```bash
code-insiders lessons/4dgs-viewer-phi.code-workspace
cd lessons
npm ci
npm run dev
```

Open `http://127.0.0.1:5173/00-environment/`. Edit
[`lessons/00-environment/main.js`](lessons/00-environment/main.js) and
[`lessons/00-environment/environment.wgsl`](lessons/00-environment/environment.wgsl)
in VS Code; Vite updates the rendered result in the browser.
See [`lessons/README.md`](lessons/README.md) for the verification commands and
[`lessons/00-environment/LESSON.md`](lessons/00-environment/LESSON.md) for the
lesson.

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

## Repository map

```text
player/        Remote renderer and thin WebRTC browser receiver
lessons/       WebGPU course source and development environment
asset-format/  Explicit 4DGS manifest and binary contract
examples/      Deterministic synthetic conformance assets
tools/         Asset and Schema validation
evidence/      Native reference/comparison receipt Schema
docs/          Architecture, platform support and validation model
```

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md),
[`docs/SUPPORTED_PLATFORMS.md`](docs/SUPPORTED_PLATFORMS.md) and
[`docs/VALIDATION.md`](docs/VALIDATION.md) for the complete technical boundary.
