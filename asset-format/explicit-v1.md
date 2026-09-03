# Explicit 4D Gaussian asset v1

The manifest schema identifier is `phi.4dgs.explicit.v1`. All binary values are little-endian.

## Geometry/time file

The file begins with a 64-byte header encoded as `<8sIIIIQQQQQ`:

| Offset | Type | Meaning |
| ---: | --- | --- |
| 0 | `char[8]` | Magic `4DGSWG01` |
| 8 | `u32` | Version, exactly 1 |
| 12 | `u32` | Header bytes, exactly 64 |
| 16 | `u32` | Gaussian count |
| 20 | `u32` | Record stride, exactly 80 |
| 24 | `u64` | Payload offset, exactly 64 |
| 32 | `u64` | Payload byte count |
| 40 | `u64` | Reserved, zero |
| 48 | `u64` | Reserved, zero |
| 56 | `u64` | Reserved, zero |

Every 80-byte record contains 20 `f32` values:

```text
vec4 mean_time          = mean.xyz, time_center
vec4 log_scale_opacity  = log_scale.xyz, logit(opacity)
vec4 rotation_xyzw      = quaternion in xyzw order
vec4 velocity_gate      = velocity.xyz, raw_gate
vec4 sh0_duration       = SH0.rgb, raw_duration
```

`mean`, `velocity`, and scale are world-space quantities. The runtime
normalizes the non-zero `rotation_xyzw` quaternion and evaluates the principal
axis scales as `exp(log_scale)`. `SH0.rgb` stores coefficients, not display RGB
values.

The runtime evaluates:

```text
position(t) = mean + (t - time_center) velocity
gate        = sigmoid(20 raw_gate)
duration    = max(1e-6, max_duration / 6 sigmoid(raw_duration))
temporal    = gate + (1 - gate) exp(-0.5 ((t - time_center) / duration)^2)
opacity(t)  = sigmoid(raw_opacity) temporal
```

Every record scalar must be finite with absolute value below `1e30`. Every
quaternion must have squared length greater than `1e-12` and finite in `f32`.
Numerically degenerate projected covariance is rejected by the renderer and
reported by its invalid counter.

## SH3 sidecar

`raw-sh3` assets include one 92-byte record per Gaussian:

- 15 non-constant real spherical-harmonic coefficients;
- three channels per coefficient;
- 45 little-endian IEEE 754 binary16 scalars, coefficient-major then RGB;
- two reserved zero bytes.

The constant SH0 RGB coefficient remains in the geometry/time record. `raw-sh0`
assets do not have an appearance sidecar.

The sidecar uses the real SH basis below. Direction is the normalized vector
from the camera center to the time-evaluated Gaussian position:

```text
d = normalize(position(t) - camera_world_position) = (x, y, z)
```

Coefficient indices are coefficient-major, RGB within each coefficient, and
follow `(l,m)` with `m = -l ... +l` while omitting `(0,0)`:

| Index | `(l,m)` | Basis value |
| ---: | :---: | --- |
| 0 | `(1,-1)` | `-0.4886025119029199 y` |
| 1 | `(1,0)` | `0.4886025119029199 z` |
| 2 | `(1,1)` | `-0.4886025119029199 x` |
| 3 | `(2,-2)` | `1.0925484305920792 xy` |
| 4 | `(2,-1)` | `-1.0925484305920792 yz` |
| 5 | `(2,0)` | `0.31539156525252005 (2z²-x²-y²)` |
| 6 | `(2,1)` | `-1.0925484305920792 xz` |
| 7 | `(2,2)` | `0.5462742152960396 (x²-y²)` |
| 8 | `(3,-3)` | `-0.5900435899266435 y(3x²-y²)` |
| 9 | `(3,-2)` | `2.890611442640554 xyz` |
| 10 | `(3,-1)` | `-0.4570457994644658 y(4z²-x²-y²)` |
| 11 | `(3,0)` | `0.3731763325901154 z(2z²-3x²-3y²)` |
| 12 | `(3,1)` | `-0.4570457994644658 x(4z²-x²-y²)` |
| 13 | `(3,2)` | `1.445305721320277 z(x²-y²)` |
| 14 | `(3,3)` | `-0.5900435899266435 x(x²-3y²)` |

For coefficient vectors `c_i`, the display-space color before compositing is:

```text
max(vec3(0), vec3(0.5) + 0.28209479177387814 * SH0 + sum(c_i * basis_i(d)))
```

## Camera and display contract

The v1 native player requires one calibrated fixed camera.
`world_to_camera_row_major` is a right-handed rigid affine transform from world
space into a positive-Z camera convention. Its final row is `[0,0,0,1]`; its
upper-left 3×3 block is orthonormal with determinant `+1` (semantic validators
use tolerance `1e-3`). This constraint permits the camera center to be recovered
as `-R^T t`.

Intrinsics are `[fx, fy, cx, cy]` at `source_size`. For output size `(W,H)`, the
renderer preserves aspect ratio using `s = min(W/source_width,
H/source_height)`, scales both focal lengths by `s`, and adds half of the unused
dimension to the principal point. This is centered letterboxing/cropping-free
fit, not independent x/y stretching.

RGB values are composited in the manifest's `display-srgb` working space. A
second linear-to-sRGB transfer is not part of this contract. Output is opaque;
the fourth background component must therefore be exactly `1` and is not used
as a separate transparency channel.

The player reads all three policy values from the manifest. `temporal_threshold` controls temporal culling, `alpha_min` is the idle compositing/culling floor and the lower bound for adaptive interaction LOD, and `low_pass` is the diagonal screen-space covariance term used by the preprocess shader. They are data, not documentation-only hints.

## Integrity

The manifest pins the exact byte length and SHA-256 of every payload. URIs must be relative paths that remain inside the asset directory. Readers must validate length, hash, header, finite records, quaternion norms, and SH3 padding before allocating renderer resources.
