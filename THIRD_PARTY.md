# Third-party inventory

## Source incorporated with modification

### gsplat

`player/shaders/preprocess.wgsl` incorporates and modifies the perspective
covariance projection from
[`gsplat/cuda/include/Utils.cuh` at commit `90d7b4b349e379ccf9ee6a8cef76aa40f48bb32e`](https://github.com/nerfstudio-project/gsplat/blob/90d7b4b349e379ccf9ee6a8cef76aa40f48bb32e/gsplat/cuda/include/Utils.cuh#L584-L605).
The local file is distributed in its entirety under Apache-2.0 to keep the
modified-file boundary unambiguous. Its header retains the upstream copyright
notices and marks the WGSL/4D/WebGPU changes. The required license copy is
[`LICENSES/Apache-2.0.txt`](LICENSES/Apache-2.0.txt). The pinned upstream tree
does not contain a `NOTICE` file.

## Rust dependencies

- `wgpu` and Naga
- `ash`
- `gstreamer-rs` crates
- `anyhow`, `bytemuck`, `clap`, `pollster`, `serde`, `serde_json`, `sha2`

Exact transitive Cargo package versions, sources and crate checksums are in
`player/Cargo.lock`.

## JavaScript and Python build dependencies

The interactive lesson build is locked by `lessons/package-lock.json`. JSON
Schema validation dependencies are hash-locked in
`tools/requirements-schema.lock`.

## Runtime/system dependencies

- Vulkan loader and driver
- Mesa/RADV for the reference AMD profile
- GStreamer 1.24 core, base, bad, WebRTC, RTP, SRTP and VA-API components
- VA-API driver
- libnice and libsrtp

These system libraries are supplied by the host distribution and are not
included in the source tree.
