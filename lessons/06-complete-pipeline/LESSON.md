# Lesson 06 — Complete synthetic pipeline

## Learning goal

Compose the earlier concepts without hiding them behind a renderer framework:

```text
manifest slot → records → validation → explicit time
              → covariance projection → N → A → V compaction
              → far-to-near order → drawIndirect(V)
              → analytic footprint → alpha blend → pixel audit
```

The checked-in external asset slot is intentionally empty. A 32-record
procedural fallback passes through the same manifest/record validation boundary,
so the entire pipeline runs without a model, checkpoint or binary payload.

## Prerequisites

- Complete Lessons 00–05, or be comfortable with WebGPU compute/render passes,
  explicit time, covariance projection, transparent ordering and alpha blend.
- A current Chrome or Chromium build with hardware acceleration enabled.
- Node.js `^20.19.0` or `>=22.12.0`.

## Open these files

1. `asset-contract.js` — the empty external slot, loader, strict validation and fallback.
2. `reference.js` — CPU time, projection, sorting and compositing equations.
3. `complete-pipeline.wgsl` — GPU time, covariance projection and bitonic order.
4. `complete-pipeline-render.wgsl` — read-only sorted input and analytic splat stages.
5. `main.js` — resource ownership, command order, draw and GPU readback audit.
6. `index.html` — canvas, diagnostics and module entry only.

## Asset input contract

`ASSET_INPUT.manifestUrl` is `null` in the repository. In that state,
`loadLessonAsset` performs no network asset request and returns the procedural
envelope:

```js
{
  manifest: {
    schema: 'phi.4dgs.lesson-manifest.v1',
    time: { start, end, initial },
    camera: { focalY, near, far, minSigmaNdc },
    render: { alphaMin },
    records: { encoding: 'json-array-f32-v1', count: 32, uri: null },
  },
  records: [{ id, center, velocity, scale, color,
              opacity, timeCenter, timeSigma }, ...],
  source: { kind: 'procedural', manifestUrl: null, recordUrl: null },
}
```

When a URL is deliberately populated, the manifest must provide a relative
`records.uri`. That JSON document has schema `phi.4dgs.lesson-records.v1` and a
`records` array with the same record shape. Both sources run through
`validateAssetEnvelope` before GPU allocation. Lesson 06's bitonic network
requires a power-of-two count no greater than 256; that restriction is explicit
in the validator rather than silently padded.

`records.uri` cannot be absolute, root-relative or protocol-relative. All
manifest and record numbers must survive conversion to a finite IEEE-754 `f32`;
a finite JavaScript number that overflows `Math.fround` is rejected.

This teaching contract is not the Player's binary explicit-v1 format. A future
adapter can decode that format into this lesson-owned record boundary without
changing projection or rendering stages.

## Mathematical model

### Explicit time

Each mean moves linearly from its time center:

$$
\boldsymbol{\mu}_i(t) = \boldsymbol{\mu}_{i,0}
+ \mathbf{v}_i(t-\mu_{t,i}).
$$

Opacity is gated by a temporal Gaussian:

$$
\alpha_i(t) = o_i \exp\left[-\frac{1}{2}
\left(\frac{t-\mu_{t,i}}{\sigma_{t,i}}\right)^2\right].
$$

Records below $\alpha_{\min}$ or outside the camera depth interval do not enter
the active set $A$.

### Perspective covariance projection

For camera-space point $(x,y,z)$, vertical focal factor $f_y$ and aspect ratio
$a$, the NDC mean is

$$
\mathbf{m} = \begin{bmatrix}
(f_y/a)x/z \\
f_y y/z
\end{bmatrix}.
$$

The corresponding Jacobian is

$$
J = \begin{bmatrix}
\dfrac{f_y}{az} & 0 & -\dfrac{f_y x}{a z^2} \\
0 & \dfrac{f_y}{z} & -\dfrac{f_y y}{z^2}
\end{bmatrix}.
$$

For the lesson's diagonal 3D covariance
$\Sigma_3=\operatorname{diag}(\sigma_x^2,\sigma_y^2,\sigma_z^2)$,

$$
\Sigma_2 = J\Sigma_3J^T + \sigma_{min}^2 I,
\qquad Q = \Sigma_2^{-1}.
$$

The vertex stage emits a conservative $3\sigma$ quad. The fragment stage
evaluates $q=\Delta^TQ\Delta$ and $\alpha=\alpha_i(t)e^{-q/2}$, discarding
fragments outside $q=9$.

### Compaction, order and blend

The reset pass clears the counters and initializes the sort array. The projection
pass counts time-and-depth-active records, projects them, performs the viewport
test and atomically appends only visible `(depth, sourceIndex)` entries:

$$
N \longrightarrow A \longrightarrow V, \qquad V \le A \le N.
$$

A lesson-owned bitonic network orders the compact visible prefix by decreasing
camera-space depth, with source index as a deterministic tie-breaker. The same
GPU-written $V$ is the `instance_count` consumed by both `drawIndirect` calls;
neither render pass substitutes a CPU count.

With far records rendered first, each near fragment applies

$$
\mathbf{C}_{new}=\alpha\mathbf{c}+(1-\alpha)\mathbf{C}_{old}.
$$

The procedural center cluster deliberately interleaves depth and color in source
order. `reference.js` composites the center once in source order and once in
depth order; a nonzero $\Delta RGB$ is the visible witness that sorting changes
the result.

## Run and interact

From `lessons/`:

```bash
npm ci
npm run dev
```

Open `http://127.0.0.1:5173/06-complete-pipeline/`.

- `←` / `→` scrubs explicit time.
- `H` hides or shows diagnostics.
- Saving JavaScript or WGSL triggers Vite live reload.

## Verifiable assertions

Every redraw reads back this 32-record teaching workload. The page publishes
`window.__LESSON_RESULT__.status === 'PASS'` only when:

- the manifest and every record pass the strict input contract;
- its source-specific assertion describes the source actually loaded: procedural
  fallback with no asset fetch, or an external manifest and record document;
- GPU `total → active → visible` counters agree with the CPU reference, and the
  indirect instance count equals `visible`;
- every GPU projected field—mean, extent, conic, opacity, color, depth, source
  index and validity—agrees with the CPU reference, including invalid records;
- the first $V$ GPU sort entries equal the deterministic CPU visible order;
- correct and source-order compositing differ at the center witness;
- a 1×1 offscreen render at NDC center agrees with CPU compositing within the
  documented UNORM tolerance;
- frame encoding, both render passes, copies and submission complete inside
  checked WebGPU error scopes.

The main canvas and 1×1 audit use the same pipeline, bindings, sorted buffer and
indirect arguments. The readback therefore detects render-shader, conic, color,
blend and ordering errors rather than stopping at compute output. It is
intentionally not the frame loop of a production-scale asset.

## Modification experiment

Change `camera.focalY` in `createProceduralAsset` from `1.35` to `1.0`. The
projected arrangement widens, while GPU projection and CPU reference should
continue to agree. This tests that the camera value flows through the manifest,
uniform buffer and both mathematical implementations.

## Expected failure experiment

In `createProceduralAsset`, temporarily make one scale component negative. The
page must report `FAIL` during input validation, before it creates a GPU record
buffer. Restore the positive value and save; Vite reloads the valid pipeline.

For a semantic GPU failure, reverse the depth comparison in `comes_before` in
`complete-pipeline.wgsl`. Shader compilation still succeeds, but the GPU/CPU
order audit must fail.

To exercise the final-pixel audit, change `input.color` to `vec3<f32>(1.0)` in
`complete-pipeline-render.wgsl`. Projection and sorting remain correct, but the
offscreen GPU/CPU pixel comparison must fail.

## Common failures

- **Manifest loads but records do not:** `records.uri` resolves relative to the
  manifest URL, not the page URL.
- **Record count rejected:** the teaching bitonic network requires a power of two.
- **Projection mismatch:** check NDC aspect scaling and the two $z^{-2}$ Jacobian
  terms before loosening the comparison tolerance.
- **Correct order but wrong color:** verify the render pipeline uses
  `src-alpha` / `one-minus-src-alpha` and draws far to near.
- **Center-pixel mismatch:** check the fragment conic, color and blend state. The
  audit texture is linear `rgba8unorm` or `bgra8unorm`, so comparison includes a
  per-channel tolerance of $5/255$ for accumulated UNORM quantization.

## Next step

The lesson ends at a validated in-memory record boundary and a complete small
GPU render path. Scaling it requires format adapters, chunked transfer, scalable
visible-set ordering and removal of synchronous frame-loop readback; those
systems can change independently without changing the projection, compaction and
blend equations verified here.
