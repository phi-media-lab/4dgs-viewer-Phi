# Lesson 03 — Order and blend

## Learning goal

Make the order dependency of transparent Gaussian splats observable. The page
renders the same three procedural records with a correct near-to-far order and
with its deliberately wrong reverse. No image, model or hidden renderer is
loaded.

## Prerequisites

- Lessons 00–02, especially analytic Gaussian evaluation and projected depth.
- A current Chrome or Chromium build with WebGPU enabled.
- Node.js `^20.19.0` or `>=22.12.0`.

## Open these files

1. `main.js` — buffers, blend state, draw order and center-pixel readback.
2. `order-blend.wgsl` — quad expansion, Gaussian evaluation and background fill.
3. `reference.js` — pure CPU ordering and compositing equations.
4. `index.html` — canvas and the one order switch.

## Mathematical model

Each 48-byte record contains a 2D mean, 2D standard deviation, RGB color,
camera-space depth and opacity:

$$
G_i = (\boldsymbol\mu_i, \boldsymbol\sigma_i,
       \mathbf c_i, z_i, o_i).
$$

The fragment at position $\mathbf p$ evaluates

$$
\alpha_i(\mathbf p) =
\operatorname{clamp}\!\left(
o_i\exp\left[-\frac{1}{2}\left\|
\frac{\mathbf p-\boldsymbol\mu_i}{\boldsymbol\sigma_i}
\right\|^2\right], 0, 0.999\right).
$$

## The exact blend convention

This lesson uses **custom front-to-back transmittance accumulation**, not the
usual back-to-front source-over convention. Records are sorted by increasing
camera-space depth. With premultiplied source color
$\mathbf S_i=\alpha_i\mathbf c_i$, the recurrence is

$$
\mathbf C_{i+1}=\mathbf C_i+(1-A_i)\mathbf S_i,
\qquad
A_{i+1}=A_i+(1-A_i)\alpha_i.
$$

The WebGPU attachment implements that recurrence with

```js
srcFactor: 'one-minus-dst-alpha'
dstFactor: 'one'
```

for both color and alpha. It starts at $(\mathbf 0,0)$. An opaque background is
drawn last, so it contributes only through the remaining transmittance
$T=1-A$. Reversing the records while keeping this blend operation is wrong and
therefore changes the overlap color.

## Host and shader responsibilities

`main.js` owns the storage buffers, explicit bind-group layout, two render
pipelines, order updates and command submission. `order-blend.wgsl` owns the
quad expansion and analytic Gaussian evaluation. `reference.js` is a pure CPU
statement of the same order and blend equations.

## Run and interact

From `lessons`:

```bash
npm ci
npm run dev
```

Open `http://127.0.0.1:5173/03-order-blend/`. Press Space or use the button to
switch between `NEAR → FAR · CORRECT` and `FAR → NEAR · WRONG`. Press `H` to
hide diagnostics.

## Verifiable assertions

The page renders the correct order into a 64 × 64 `rgba8unorm` texture, copies
the center pixel to a mapped buffer, and compares it with `reference.js`. A pass
requires:

- a 48-byte CPU/WGSL record stride;
- monotonic near-to-far depth indices;
- a visible numerical difference between correct and reversed order; and
- GPU center color within $2.5/255$ Euclidean RGB distance of the CPU result.

The result is available as `window.__LESSON_RESULT__`; `details.order` reports
the depth sequence currently displayed.

## Modification experiment

Change the near red record's opacity in `main.js` from `0.82` to `0.35`. Both
modes update after Vite reload, but the separation between their overlap colors
shrinks because more transmittance reaches the later records. The CPU reference
and GPU readback continue to agree without a tolerance change.

## Expected failure experiment

In `setMode`, replace `correctOrder` with `wrongOrder` for the supposedly
correct branch. The image still renders, but `details.order` exposes decreasing
depths. Then make the same replacement inside `validateGpuCenterPixel`: the GPU
readback no longer matches the CPU front-to-back reference, and the lesson
enters `FAIL`. This demonstrates why successful submission alone cannot verify
transparent rendering.

## Common failures

- **Both modes look identical:** ensure all three splats overlap and the order
  buffer is rewritten when the switch changes.
- **Dark fringes:** `fs_gaussian` returns premultiplied RGB; returning straight
  RGB applies the opacity convention incorrectly.
- **GPU/CPU center mismatch:** include the half-pixel center when converting the
  validation pixel to NDC and keep the validation texture `rgba8unorm`.
- **Background replaces the splats:** it must use the same remaining-
  transmittance blend and be drawn last.

## Next step — Lesson 04

The lesson uses CPU sorting because three records make the ordering rule easy
to inspect. Parallel key generation, radix sort and indirect drawing belong to
the later performance path. Lesson 04 keeps this compositing equation and adds
an explicit time coordinate to each primitive.
