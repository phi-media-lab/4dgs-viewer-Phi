# Third-party inventory

Status: engineering inventory only. CI exports a deterministic inventory of all
three checked-in lock files and an npm-generated CycloneDX build SBOM. These
artifacts enumerate resolved packages but do not replace the required license
compatibility and runtime redistribution review.

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

Exact transitive Cargo package versions, sources and crate checksums come from
`player/Cargo.lock` and are exported into the CI lockfile inventory. Cargo
license expressions are not present in that lock file, so a separate Cargo
license report remains a release blocker.

## JavaScript and Python build dependencies

The interactive lesson build is locked by `lessons/package-lock.json`; CI also
uses npm itself to emit a CycloneDX 1.5 build SBOM after `npm ci`. JSON Schema
validation dependencies are hash-locked in `tools/requirements-schema.lock`.
Both ecosystems are included in the path-free lockfile inventory.

## CI actions

The source workflow permits only these GitHub-maintained actions, each pinned
to the immutable commit shown in `.github/workflows/ci.yml`:

- `actions/checkout` — `11d5960a326750d5838078e36cf38b85af677262`;
- `actions/setup-python` — `a26af69be951a213d495a4c3e4e4022e16d87065`;
- `actions/setup-node` — `49933ea5288caeca8642d1e84afbd3f7d6820020`;
- `actions/upload-artifact` — `ea165f8d65b6e75b540449e92b4886f43607fa02`.

`tools/audit_public_tree.py` rejects moving tags and action repositories outside
that allowlist.

## Runtime/system dependencies

- Vulkan loader and driver
- Mesa/RADV for the currently validated AMD profile
- GStreamer 1.24 core, base, bad, WebRTC, RTP, SRTP and VA-API components
- VA-API driver
- libnice and libsrtp

The source release and any future binary/runtime bundle have different redistribution obligations. A binary bundle must not be published until its LGPL and system-package compliance has been reviewed.

## Research/data dependencies

No third-party model or dataset is included. FreeTimeGS++, gsplat and SelfCap may be referenced for interoperability or evaluation provenance, but their code and data remain under their own terms.
