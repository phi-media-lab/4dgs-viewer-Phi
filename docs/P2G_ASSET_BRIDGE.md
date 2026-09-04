# Pixel4DGS AssetBundle bridge

`tools/convert_p2g_asset.py` is an offline contract boundary between a verified
[Pixel4DGS](https://github.com/phi-media-lab/4dgs-reconstruction-phi)
inference asset and the Phi Remote Frame Mode Player. It is not a checkpoint
importer and does not require PyTorch, Safetensors, a GPU, or the Pixel4DGS
source tree. The producer-side profile, export, and camera-path commands are in
the
[Pixel4DGS Viewer interoperability guide](https://github.com/phi-media-lab/4dgs-reconstruction-phi/blob/main/docs/VIEWER_INTEROP.md).

```text
p2g.asset_bundle.v1 + p2g.camera_path.v1
                  │ verify exact bytes and semantics
                  │ normalize time, reorder quaternion, quantize SH3
                  ▼
phi.4dgs.explicit.v1
  ├─ manifest.json
  ├─ gaussians.bin
  └─ sh3.f16
```

## Accepted source profile

The converter deliberately accepts one narrow, executable profile:

- `p2g.asset_bundle.v1`, format major version 1;
- `p2g.asset_model.v1` with the exact 14-plane tensor catalog;
- `p2g.linear_motion_gaussian_gate.v1` with learned persistence;
- real SH degree 3 using `gsplat_real_sh_v1`;
- pre-undistorted pinhole cameras, OpenCV axes and world-to-camera extrinsics;
- `p2g.gsplat_rocm.v1`, `radius_clip == 0`, finite near/far/`eps2d`, and
  clamped output RGB.

Anything outside that profile is rejected. Unknown training state is never
guessed. The source bundle must contain exactly `asset.json`, `manifest.json`
and `model.safetensors`; all file lengths, SHA-256 values, the bundle ID, the
Safetensors catalog and its metadata must close before conversion starts.

The camera path is separately hash-bound to the bundle. Every frame—not only
the selected one—is checked for finite pinhole intrinsics, a rigid right-handed
world-to-camera matrix, nondecreasing timestamps and membership in the asset's
valid time interval.

## Semantic mapping

Let the source interval be `[t0, t1]` seconds and `D = t1 - t0`. Phi time is
normalized to `u = (t - t0) / D`.

```text
mean_phi       = mean_p2g
center_phi     = (center_seconds - t0) / D
velocity_phi   = velocity_per_second * D
sigma_phi      = sigma_seconds / D
raw_gate_phi   = raw_gate_p2g * p2g_gate_scale / 20
quaternion_phi = (x, y, z, w) from p2g (w, x, y, z)
```

The duration logit is retained when all source bounds use the FTGS++ common
`[0, sigma_max]` form. Otherwise it is reparameterized so the reconstructed
physical sigma is unchanged. A source sigma below the Player's `1e-6`
normalized floor is rejected rather than silently widened.

SH0 remains `f32`. The 15 non-constant SH3 RGB coefficients are converted from
`f32` to IEEE binary16 and written coefficient-major. This is the only intended
lossy parameter conversion; the receipt reports maximum and mean absolute SH
quantization error.

The selected camera retains its source calibration. At a different output
resolution the Player applies the explicit centered aspect-fit rule documented
in `asset-format/explicit-v1.md`. The bridge stores the selected camera
timestamp as normalized `time.initial` in the output manifest, so the Player
uses it automatically. The conversion receipt repeats the same value as
`initial_normalized_time` for audit and explicit `--time` overrides.

## Raster and color ABI

Pixel4DGS AssetBundle v1 uses gsplat classic rasterization. The bridge records
that behavior as executable manifest policy:

```json
{
  "opacity_compensation": "none",
  "alpha_cap": 0.999,
  "pixel_alpha_min": 0.00392156862745098,
  "transmittance_epsilon": 0.0001
}
```

The compositor therefore checks the candidate `next_T` and terminates before
adding the candidate when `next_T <= 1e-4`, matching the source ABI. Existing
Phi assets that omit the explicit raster trio retain their legacy behavior.
The explicit profile also bypasses Phi's historical `2048 px` projected-radius
cap because the accepted source contract requires `radius_clip == 0` and does
not declare such a maximum-radius approximation.

For `output_photometric_space: "srgb_reference_profile"`, values are composited
and emitted as display sRGB without a second transfer. For `linear_rgb`, Phi
composites in linear RGB and applies the IEC sRGB transfer once after the
background resolve.

## Convert

The output path must not exist:

```bash
python3 tools/convert_p2g_asset.py \
  /path/to/asset-bundle-v1 \
  /path/to/camera_path.json \
  /new/private/explicit-v1 \
  --camera-frame 0 \
  --name example
```

The command writes through a sibling staging directory, validates the completed
explicit-v1 asset, syncs it, and atomically publishes the directory. Repeating
the conversion with identical source bytes and converter revision produces
identical payload bytes and manifest content. Existing destinations and source
or camera symlinks are refused.

Render the selected camera frame without a time argument; the Player starts at
the manifest's `time.initial` value:

```bash
cd player
cargo run --release --locked -- \
  --manifest /new/private/explicit-v1/manifest.json \
  --shaders shaders \
  --width 1280 --height 720 \
  --write-golden /new/private/capture/reference.rgba8 \
  --output-dir /new/private/capture
```

Pass `--time NORMALIZED_TIME` only for an intentional override. The receipt's
`initial_normalized_time` is an audit copy of the manifest value, not a required
launch argument.

`--write-golden` here is only a non-overwriting Phi frame capture. It does not
prove cross-renderer parity. A valid comparison uses an independently rendered,
same-camera, same-time, same-resolution source RGBA8 frame and records both
hashes plus numerical image metrics.

Declare the acceptance gate before inspecting the result. The project uses this
high-fidelity gate for the first 1280 x 720 cross-render check:

```bash
python3 tools/compare_rgba8.py \
  /private/source/reference.rgba8 \
  /private/phi/target.rgba8 \
  --width 1280 --height 720 \
  --min-psnr-db 40 \
  --max-mean-abs 1 \
  --max-rmse 2.55 \
  --output /private/phi/cross-render-comparison.json
```

Exit status `0` means every declared threshold and the opaque-alpha invariant
passed, `1` is a metric failure, and `2` is an invalid or unsafe input. The
tool requires at least one RGB acceptance threshold (or
`--require-rgb-exact`); opaque alpha alone can never produce a passing receipt.
The receipt is created exclusively and binds both raw inputs by SHA-256. Do not
lower a failed threshold after seeing the metrics; fix or document the rendering
difference and create a new, explicitly named experiment instead.

## Validation layers

Run portable checks first:

```bash
python3 -m unittest discover -s tests -v
python3 tools/validate_asset.py /new/private/explicit-v1/manifest.json
```

Then run the Linux Player checks and one-frame render described in
`docs/VALIDATION.md`. Finally compare the independently produced source and Phi
RGBA8 frames. These gates answer different questions:

1. source closure proves the converter read the intended model and camera;
2. explicit-v1 validation proves the output is structurally loadable;
3. parameter error metrics quantify the bridge's f32/f16 transformation;
4. cross-render image metrics test camera, time, projection, SH, compositing and
   color behavior together;
5. a Chrome WebRTC session tests streaming, decode, presentation and controls.

A passing structural test must not be reported as a passing image or streaming
test.

## Rights and scale

The converter copies the source `rights` object into provenance but cannot
grant new rights. Keep restricted source and converted payloads outside a Git
tree. This repository intentionally ships only synthetic conformance assets.

An SH3 conversion uses approximately 172 payload bytes per Gaussian: 80 bytes
of geometry/time plus 92 bytes of appearance, before manifest and runtime GPU
working memory. Output resolution also affects the Player's tile-rank masks;
validate the intended resolution on the actual renderer instead of inferring it
from model size alone.
