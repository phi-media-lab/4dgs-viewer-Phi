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

## Documentation media

### SelfCap Corgi renderer preview

[`docs/assets/remote-frame-corgi-motion.webp`](docs/assets/remote-frame-corgi-motion.webp)
is a compressed documentation preview rendered by this repository's native
Rust/wgpu/WGSL Player from a 499,980-Gaussian SH3 representation of the Corgi
scene in the SelfCap Dataset. The source video, trained model, converted asset,
raw rendered frames and private validation receipts are not distributed here.

The SelfCap Dataset is copyright 2024–2025 3D Vision Group at the State Key
Lab of CAD&CG, Zhejiang University. Its terms permit use, modification and
distribution for educational, research and non-profit purposes, require
derivative modifications to remain open-source and non-commercial, and require
retention of notices. A copy of the upstream terms is preserved in
[`LICENSES/SelfCap-Dataset.txt`](LICENSES/SelfCap-Dataset.txt), with the
[canonical file hosted by the dataset publisher](https://huggingface.co/datasets/zju3dv/SelfCap-Dataset/blob/main/LICENSE).
The preview is included for public research documentation under those terms.
It is not relicensed under Apache-2.0; the SelfCap terms above apply to this
preview.

Dataset and citation information:

- [SelfCap Dataset / LongVolCap project](https://zju3dv.github.io/longvolcap/)
- Zhen Xu, Yinghao Xu, Zhiyuan Yu, Sida Peng, Jiaming Sun, Hujun Bao and
  Xiaowei Zhou, *Representing Long Volumetric Video with Temporal Gaussian
  Hierarchy*, ACM Transactions on Graphics 43(6), 2024.

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
