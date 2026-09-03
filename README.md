# 4DGS Viewer Phi

> Pre-release source tree. Do not cut a release until the licensing and provenance gates in `docs/RELEASE_BLOCKERS.md` are closed.

This repository is extracting two independently runnable artifacts from a 4DGS research workspace:

- `player/`: a Linux Vulkan/DMA-BUF/VA-API Remote Frame Mode Player;
- `lessons/`: a first-principles WebGPU + WGSL course viewed as code in VS Code and as rendered output in a browser.

The repository intentionally does not contain the client-side Gaussian
streaming experiment, training code, real-person checkpoints, private network
receipts, or machine configuration.

## Current staging milestone

The first slice establishes:

- an isolated tree from which a clean repository history can be created;
- a strict explicit 4DGS asset contract;
- deterministic procedural SH0 and SH3 samples;
- a Player run script that defaults to the synthetic SH3 sample;
- a self-contained Lesson 00 with no client-side streaming dependency.

This is not a release yet. The public project identity is now
`phi-media-lab/4dgs-viewer-Phi`; the OSI license and copyright owner still need
maintainer approval.

## Repository map

```text
player/        Rust/wgpu/WGSL renderer and thin WebRTC browser receiver
lessons/       Browser WebGPU course; Lesson 00 is the first extracted lesson
asset-format/  Manifest and binary format contract
examples/      Procedurally generated, deterministic assets
tools/         Asset generation and validation
evidence/      Strict reference/comparison receipt contract
LICENSES/      License texts required by incorporated third-party files
docs/          Supported scope, provenance and release blockers
```

Start with [`docs/OPEN_SOURCE_PACKAGING.md`](docs/OPEN_SOURCE_PACKAGING.md) and
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), then use
[`docs/VALIDATION.md`](docs/VALIDATION.md) to distinguish source, GPU and browser
evidence. The exact platform claim is in
[`docs/SUPPORTED_PLATFORMS.md`](docs/SUPPORTED_PLATFORMS.md).

## Generate and validate samples

```bash
python3 tools/generate_synthetic_asset.py --check
python3 tools/validate_asset.py examples/minimal-sh0/manifest.json
python3 tools/validate_asset.py examples/synthetic-motion-sh3/manifest.json
python3 -m pip install --require-hashes -r tools/requirements-schema.lock
python3 tools/check_json_schema.py examples/*/manifest.json
python3 tools/audit_public_tree.py
```

Use `--force` only when intentionally replacing the checked-in generated files. Payloads and the manifest are atomically replaced one file at a time, with the manifest published last so an interrupted update fails integrity validation rather than appearing valid.

## Run Lesson 00

```bash
cd lessons
npm ci
npm run dev
```

Open the URL printed by Vite and select Lesson 00. VS Code remains the code surface; the browser shows only the canvas, a small status line, and errors.

## Player support boundary

The v0.1 target renderer profile is Ubuntu 24.04 x86_64, AMD RADV/VA-API and
GStreamer 1.24. Its extracted one-frame Vulkan/DMA-BUF/VA-API gate has run on
that profile; a fresh end-to-end Chrome/Chromium session remains a release
gate. Other Linux GPUs are unverified. macOS is receiver-only and Windows is
out of scope.

The player defaults to loopback and must fail closed when a linear DMA-BUF path
is unavailable. It must never silently replace the streaming path with CPU
pixel readback.
