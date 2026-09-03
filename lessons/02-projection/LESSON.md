# Lesson 02 — Projecting a 3D Gaussian

## Learning goal

Derive the screen-space ellipse of a 3D Gaussian from a pinhole camera and its
projection Jacobian. Four tiny records are created in JavaScript; there is no
model or asset file.

## Prerequisites

- Complete Lesson 01 or understand the analytic 2D footprint it implements.
- Be comfortable multiplying small matrices and reading camera-space
  coordinates.
- Run a current WebGPU-capable Chrome or Chromium build.

## Mathematical projection

For camera-space mean $\boldsymbol{\mu}=(X,Y,Z)$ and pixel focal lengths
$f_x,f_y$, the projected mean is

$$
\boldsymbol{\mu}' =
\begin{bmatrix}
f_x X/Z+c_x\\
f_y Y/Z+c_y
\end{bmatrix}.
$$

The first-order change around that mean is described by

$$
J =
\begin{bmatrix}
f_x/Z & 0 & -f_xX/Z^2\\
0 & f_y/Z & -f_yY/Z^2
\end{bmatrix}.
$$

## Covariance and conic

A scale vector $\mathbf{s}$ and orientation $R$ define the camera-space 3D
covariance

$$
\Sigma_{3D}=R\,\operatorname{diag}(\mathbf{s}^2)R^\mathsf{T}.
$$

Its screen-space first-order approximation is

$$
\Sigma_{2D}=J\Sigma_{3D}J^\mathsf{T}+\sigma_{min}^2 I.
$$

This lesson uses $\sigma_{min}^2=1\ \mathrm{px}^2$ so a tiny projection remains
invertible and cannot disappear through sub-pixel collapse. The fragment shader
uses the inverse covariance, or conic,

$$
Q=\Sigma_{2D}^{-1},\qquad
\alpha(\mathbf{x})=o\exp\!\left(-\frac12
(\mathbf{x}-\boldsymbol{\mu}')^\mathsf{T}
Q(\mathbf{x}-\boldsymbol{\mu}')\right).
$$

The vertex shader emits an axis-aligned three-standard-deviation bound. That
rectangle is only conservative geometry; the conic evaluated by each fragment
produces the rotated ellipse.

## The four cases

The procedural records make four consequences visible at once:

1. **Near isotropic** and **far isotropic** use the same world-space scale. The
   near footprint is larger because the Jacobian contains $1/Z$.
2. **Rotated anisotropic** produces a non-zero off-diagonal covariance term and
   therefore a rotated screen ellipse.
3. **Off-axis depth tilt** has covariance coupled to the third dimension. The
   $-fX/Z^2$ and $-fY/Z^2$ Jacobian terms affect its footprint.

Use `[` and `]` to change focal length, `R` to reset it, and `H` to toggle the
diagnostics. Changing focal length rebuilds only the small procedural records
and camera uniform; it does not rebuild a shader or pipeline.

## Open these files

1. `reference.js` — CPU covariance construction, Jacobian and projection.
2. `projection.wgsl` — the same equations used by compute and render stages.
3. `main.js` — storage layouts, four-case draw and readback comparison.

## Run and interact

Run `npm run dev` from `lessons/` and open
`http://127.0.0.1:5173/02-projection/`.

## Modification experiment

Press `[` repeatedly to shorten the focal length. All means remain in their
assigned quadrants because the procedural scene rebuilds their $X,Y$ positions,
while the footprints shrink with the focal length. Press `R` to restore the
reference camera.

## Verifiable assertions

The compute shader writes eight values for every case:

$$
(u,v,\Sigma_{xx},\Sigma_{xy},\Sigma_{yy},Q_{xx},Q_{xy},Q_{yy}).
$$

The CPU reference consumes the exact float32 camera and Gaussian values sent to
WebGPU, then independently evaluates the equations. Each value uses a combined
absolute and relative tolerance,

$$
\lvert x_{GPU}-x_{CPU}\rvert
\le 5\times10^{-4}+2\times10^{-6}\lvert x_{CPU}\rvert,
$$

so the check remains meaningful from ordinary windows through high-DPI,
multi-thousand-pixel canvases. It also asserts that the near trace is larger
than the far trace and that the rotated case has a visible cross term. Inputs,
Jacobians, CPU results, GPU results and per-case errors are available in
`window.__LESSON_RESULT__.details.cases`.

## Expected failure experiment

In `projection.wgsl`, remove the `b * d * s22` term from the `xy` equation and
save. The simple centred isotropic cases may still look reasonable, but the
off-axis case disagrees with `reference.js` and produces a deterministic
`FAIL`. Restore the term to recover.

This is why the lesson includes an off-axis, depth-tilted covariance rather than
validating only a centred sphere.

## Common failures

- **Ellipses contain NaNs:** verify $Z>0$ and that the covariance determinant is
  positive before inversion.
- **Rotation is mirrored:** keep the covariance row-major convention identical
  in `reference.js`, the packed storage record and WGSL.
- **Off-axis CPU/WGSL mismatch:** the $XZ$, $YZ$ and $ZZ$ terms couple through
  the third Jacobian column; centred test cases do not exercise all of them.
- **Far splat vanishes:** retain the minimum screen-space variance before
  inverting the covariance.

## Next step — Lesson 03

All four footprints are separate, so their draw order cannot yet change the
image. Lesson 03 introduces overlapping translucent Gaussians and makes the
ordering requirement observable.
