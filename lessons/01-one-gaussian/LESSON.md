# Lesson 01 — One analytic Gaussian

## Learning goal

Turn one 2D Gaussian record into an elliptical, analytic footprint. The lesson
uses one uniform buffer, one six-vertex bounding quad and a fragment shader. It
does not use a texture, mesh, asset file, sorting or instancing.

## Prerequisites

- Complete Lesson 00 or understand the WebGPU pipeline and command sequence it
  exposes.
- Know that a covariance matrix describes scale and orientation together.
- Run a current WebGPU-capable Chrome or Chromium build.

## Mathematical model

`main.js` writes five meaningful quantities to `Gaussian2D`:

$$
(W,H),\quad \boldsymbol{\mu},\quad
(\sigma_x,\sigma_y),\quad \theta,\quad o.
$$

The covariance represented by the two standard deviations and rotation is

$$
\Sigma = R(\theta)
\begin{bmatrix}\sigma_x^2&0\\0&\sigma_y^2\end{bmatrix}
R(\theta)^\mathsf{T}.
$$

For a pixel position $\mathbf{x}$, an unnormalised splat evaluates

$$
\alpha(\mathbf{x}) = o\exp\!\left(
-\frac{1}{2}(\mathbf{x}-\boldsymbol{\mu})^\mathsf{T}
\Sigma^{-1}(\mathbf{x}-\boldsymbol{\mu})
\right).
$$

The vertex shader applies $R\,\mathrm{diag}(\sigma_x,\sigma_y)$ to a quad that
extends to three standard deviations. It also passes the dimensionless local
coordinate $\mathbf{q}$ to the fragment shader, reducing the exponent to
$-\lVert\mathbf{q}\rVert^2/2$. Pixels outside $\lVert\mathbf{q}\rVert>3$ are
discarded. The shader returns premultiplied colour because the pipeline uses
`one, one-minus-src-alpha` blending.

## Open these files

1. `main.js` — the uniform, render pipeline, compute check and command sequence.
2. `one-gaussian.wgsl` — quad expansion and analytic fragment evaluation.
3. `reference.js` — the independent CPU equation.

## Run and interact

From `lessons/` run `npm run dev`, then open
`http://127.0.0.1:5173/01-one-gaussian/`.

- Drag to move the mean.
- Use the wheel to change the major standard deviation.
- Press `A`/`D` to rotate and `W`/`S` to change anisotropy.
- Press `R` to restore the initial record and `H` to toggle diagnostics.

Every interaction changes only the 48-byte uniform. The shader module,
pipelines and bind groups remain unchanged.

## Modification experiment

Change `GAUSSIAN_SAMPLE` in `reference.js` to `[2, 0]`. The rendered footprint
is unchanged, while the CPU and WGSL verification values both fall to
$o\exp(-2)$. This separates the diagnostic sample from the rendered pixels.

## Verifiable assertions

The compute entry point evaluates the same Gaussian at
$\mathbf{q}_*=(1,0.5)$ and copies the result to a four-float readback buffer.
`reference.js` evaluates that sample on the CPU. A frame passes only when

$$
\left|\alpha_{CPU}(\mathbf{q}_*)-
\alpha_{WGSL}(\mathbf{q}_*)\right| \le 10^{-6}.
$$

The complete state, covariance, two values and absolute error are exposed in
`window.__LESSON_RESULT__.details`. The corresponding
`cpuGpuAgreement` assertion is machine-readable.

## Expected failure experiment

In `analytic_alpha` and `verify_gaussian`, change the exponent coefficient from
`-0.5` to `-0.4` in only `verify_gaussian`. Saving the file reloads the page and
the independent CPU comparison reports `FAIL`. Restore `-0.5` to recover.

This experiment distinguishes a rendered image that merely looks plausible
from an implementation of the stated equation.

## Common failures

- **Ellipse is clipped:** the vertex bound and fragment cutoff must use the
  same three-sigma radius.
- **Dark fringe:** the shader emits premultiplied colour, so the blend source
  factor must remain `one`.
- **Pointer is offset after resize:** the interaction stores a normalised mean;
  pixel coordinates are derived after canvas configuration.
- **CPU/WGSL mismatch:** compare the two exponent implementations and confirm
  both use the same opacity and sample.

## Next step — Lesson 02

This covariance already lives in screen pixels. Lesson 02 starts with a 3D
mean and covariance, then derives the screen-space mean and conic with the
camera projection Jacobian.
