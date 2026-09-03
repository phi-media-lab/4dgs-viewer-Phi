# 4DGS Remote Frame Mode Player

This directory contains a Linux reference implementation of remote 4D Gaussian rendering:

```text
explicit 4DGS asset
  → Rust + wgpu + WGSL
  → Vulkan linear BGRA image
  → DMA-BUF
  → VA-API NV12/H.264
  → WebRTC
  → browser <video>
```

The browser is a thin receiver. It decodes and presents H.264, sends camera/time controls, and reports receiver progress. It does not download Gaussian data, compile renderer WGSL, or create a `GPUDevice`.

## Supported profile

The reference renderer profile is:

- Ubuntu 24.04 x86_64;
- AMD RADV Vulkan with external-memory DMA-BUF;
- a linear DRM AR24 modifier;
- GStreamer 1.24 with legacy `vaapipostproc` and `vaapih264enc`.

The receiver requires Chrome/Chromium with H.264 WebRTC support.

Other vendors and distributions are unverified. The HTTP signaling server is
single-peer and loopback-only in v0.1. On a reachable LAN, an SSH local port
forward can carry signaling; WebRTC media and DataChannels still use a direct
UDP host-candidate path to the renderer. This is not an Internet-facing or
multi-tenant service.

## Prerequisites

Install Rust as pinned by `rust-toolchain.toml`, Vulkan/RADV, VA-API, and GStreamer development/runtime packages. On Ubuntu 24.04:

```bash
sudo apt-get update
sudo apt-get install --yes \
  build-essential pkg-config libvulkan-dev vulkan-tools mesa-vulkan-drivers \
  libva-dev mesa-va-drivers vainfo ffmpeg \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  libgstreamer-plugins-bad1.0-dev \
  gstreamer1.0-tools gstreamer1.0-plugins-base \
  gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
  gstreamer1.0-nice gstreamer1.0-vaapi
```

The Player uses the GStreamer runtime supplied by the host distribution.

## Build and test

```bash
cargo build --locked
cargo test --locked
```

## Import an inference AssetBundle

From the repository root, convert a verified Pixel4DGS AssetBundle and its
bound camera path into a new explicit-v1 directory:

```bash
python3 tools/convert_p2g_asset.py \
  /path/to/asset-bundle-v1 \
  /path/to/camera_path.json \
  /new/private/explicit-v1 \
  --camera-frame 0 \
  --name example
```

The converter stores the selected camera timestamp as manifest `time.initial`
and repeats it in the receipt as `initial_normalized_time`. The Player uses the
manifest value unless `--time` explicitly overrides it. Conversion is offline
and CPU-only; rendering still occurs here on the Linux GPU. See
[`../docs/P2G_ASSET_BRIDGE.md`](../docs/P2G_ASSET_BRIDGE.md) for the accepted
source profile and semantic mapping.

## Establish and validate a one-frame reference

Creating a reference is an explicit, non-overwriting operation. Run it on the
renderer host. The command writes both raw RGBA8 and a neighboring
`.rgba8.json` receipt whose initial review status is `UNREVIEWED`:

```bash
cargo run --release --locked -- \
  --manifest ../examples/synthetic-motion-sh3/manifest.json \
  --write-golden validation/reference.rgba8 \
  --width 640 --height 360 \
  --output-dir validation
```

Convert the raw frame into a viewable PNG without overwriting an existing
review file:

```bash
ffmpeg -v error -n \
  -f rawvideo -pixel_format rgba -video_size 640x360 \
  -i validation/reference.rgba8 \
  -frames:v 1 validation/reference.png
```

Inspect that PNG and the receipt's adapter, driver, source and asset hashes.
Reference creation is not validation: only a separately reviewed reference may
be used by the comparison command below.

Review a reference:

1. View the PNG at native size and compare it with the expected calibration
   regions in `../examples/README.md`.
2. Confirm that the raw file hash, asset hashes, dimensions, time, adapter and
   driver in the neighboring receipt describe the frame you inspected.
3. In the receipt, change only `"review_status": "UNREVIEWED"` to
   `"review_status": "REVIEWED"`; do not regenerate either file.
4. Run the strict receipt Schema check below again. The later comparison also
   rechecks the raw bytes and all cross-file identities before rendering.

Both the reference receipt and the later comparison receipt have a strict,
versioned structural contract in
`../evidence/remote-native-evidence-v1.schema.json`. Validate either form with:

```bash
python3 ../tools/check_json_schema.py \
  --schema ../evidence/remote-native-evidence-v1.schema.json \
  validation/reference.rgba8.json
```

The reference form uses schema identifier
`phi.4dgs.remote-native.reference.v1` and ends in `reference`. The comparison
form uses `phi.4dgs.remote-native.receipt.v1` and ends in `image` plus
`transport`. Both forms pin the Rust toolchain, source bundle, shader bundle,
browser receiver build, asset manifest, geometry and optional appearance by
value or SHA-256. `git_commit` may be null for local captures. To bind a receipt
to a revision, set `PHI_GIT_COMMIT` to that commit **when compiling the
binary** (`option_env!` captures it at build time). The actual
`rustc --version` release is injected by `build.rs`; evidence mode refuses a
binary whose compiler release differs from the pinned `1.95.0` toolchain.

Normal evidence mode consumes that reviewed reference:

```bash
cargo run --release --locked -- \
  --manifest ../examples/synthetic-motion-sh3/manifest.json \
  --golden validation/reference.rgba8 \
  --width 640 --height 360 \
  --output-dir validation
```

The comparison command requires the neighboring `.rgba8.json` receipt and
refuses to render unless it is reference v1 with `review_status == "REVIEWED"`.
Before GPU work it verifies the raw byte count and hash, asset name and hashes,
and frame width, height and time. It does not require the old
reference's source hash to equal the current renderer, because that would make
cross-version regression comparison impossible. Runtime enforcement checks
only this required subset; run the strict Schema command above to reject other
structural errors and duplicate keys before accepting revision-bound evidence.

`--write-golden` refuses to overwrite either an existing frame or receipt. Do
not create and compare a new golden in the same run.

## Run the WebRTC player

```bash
./scripts/run.sh
```

Defaults:

```text
asset       ../examples/synthetic-motion-sh3/manifest.json
listen      127.0.0.1:4191
resolution  1280×720
cadence     30 fps
```

Override the asset without changing the script:

```bash
PHI_MANIFEST=/absolute/path/to/manifest.json ./scripts/run.sh
```

Pass `--time NORMALIZED_TIME` only when intentionally overriding the
manifest's initial time.

For a remote host, forward the loopback listener:

```bash
ssh -L 4192:127.0.0.1:4191 user@renderer-host
```

Then open `http://127.0.0.1:4192/?jitter_buffer_ms=browser` in Chrome.

`ssh -L` forwards only HTTP signaling. It does not relay WebRTC media or
DataChannels. The browser must be able to reach one of the renderer's advertised
host ICE candidates over UDP, and the renderer firewall must admit that path.
NAT, cloud and otherwise non-direct networks normally require a TURN relay;
TURN configuration is not implemented in v0.1, so those deployments fail
closed rather than falling back through the SSH TCP tunnel.

## Experimental LAN transport knobs

The default path leaves WebRTC priority at `inherit` and does not install the
custom sleep-based nicesink pad probe. RTX/NACK recovery remains enabled. The
probe is not a congestion controller and is not part of renderer correctness.

For a measured, isolated LAN experiment only, opt in explicitly:

```bash
PHI_EXPERIMENTAL_LAN_PACER=1 \
PHI_WEBRTC_PACER_BITRATE_BPS=100000000 \
PHI_WEBRTC_PACER_BURST_BYTES=8192 \
PHI_WEBRTC_VIDEO_PRIORITY=high \
./scripts/run.sh
```

`PHI_WEBRTC_VIDEO_PRIORITY=high` asks GStreamer/libnice for high-priority
video (commonly AF42), but only a packet capture proves the applied wire DSCP.
Test these knobs against the default; do not treat them as portable latency or
quality improvements.

## Protocol compatibility

The HTTP offer shape, two DataChannel roles and their JSON message variants are
implementation-internal v0.1 interfaces between the server and its embedded
`web/client.js`; they are not a stable public SDK. Deploy the browser file and
server from the same revision.

- `control` accepts orbit, zoom and camera-state messages, with receiver stats
  and progress accepted as a fallback.
- `config` accepts reliable camera-state tails, reset, time/playback changes,
  keyframe requests, receiver stats and receiver progress.

Unknown or unlabeled DataChannels are ignored. Incoming string messages are
capped at 64 KiB before JSON parsing and reject unknown JSON fields.

## Edit/restart loop

On the Linux renderer, `./scripts/watch.sh` hashes the Rust, WGSL, browser and
script inputs and restarts the single-peer process when an edited file changes.
This is intended for VS Code Remote SSH or another tool that writes directly to
the renderer checkout. It is process restart, not state-preserving shader HMR.

On the Mac receiver, keep the signaling SSH forward running and use:

```bash
./scripts/open-preview-macos.sh
```

That helper only validates the URL and opens Chrome; it neither creates the
tunnel, relays WebRTC UDP, nor owns the renderer.

## Safety and failure behavior

- The HTTP/signaling listener rejects non-loopback bind addresses in v0.1.
- The renderer requires Vulkan external-memory support.
- Only linear modifier `0` is accepted by the reference media path.
- Unsupported interop fails closed; no hidden CPU pixel-copy fallback exists.
- Network tuning and public deployment are not supported in v0.1.
