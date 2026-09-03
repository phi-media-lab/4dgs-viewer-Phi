# Lesson 04 — Explicit time

## Learning goal

Add time as an inspectable field of every Gaussian primitive. The scene is
procedural: four static records remain fixed while three moving records change
position and temporal opacity as the timeline advances. The slider and play
button control the same scalar sent to WGSL.

## Prerequisites

- Lesson 03's near-to-far transmittance convention.
- A current Chrome or Chromium build with WebGPU enabled.
- Node.js `^20.19.0` or `>=22.12.0`.

## Open these files

1. `main.js` — procedural records, timeline input, buffers and GPU commands.
2. `explicit-time.wgsl` — shared render/validation time evaluation.
3. `reference.js` — record packing and pure CPU equations.
4. `index.html` — canvas, play button and time slider.

## Mathematical model — a 64-byte explicit record

Each storage record is

$$
P_i=(\boldsymbol\mu_i^0,\mathbf v_i,\mathbf c_i,
t_i^c,d_i,o_i,m_i,\boldsymbol\sigma_i,z_i),
$$

where $m_i\in\{0,1\}$ selects static or moving behavior. `reference.js` packs
these values into 16 `f32` lanes (64 bytes), exactly matching
`Primitive4D` in `explicit-time.wgsl`.

At requested time $t$, define

$$
\Delta t_i=t-t_i^c,
\qquad
\boldsymbol\mu_i(t)=\boldsymbol\mu_i^0+m_i\mathbf v_i\Delta t_i,
$$

and the temporal opacity gate

$$
g_i(t)=(1-m_i)+m_i\exp\left[-\frac12
\left(\frac{\Delta t_i}{d_i}\right)^2\right].
$$

The spatial fragment opacity is

$$
\alpha_i(\mathbf p,t)=
\min\!\left(0.999,
\max\!\left(0,
o_i g_i(t)\exp\left[-\frac12\left\|
\frac{\mathbf p-\boldsymbol\mu_i(t)}{\boldsymbol\sigma_i}
\right\|^2\right]
\right)\right).
$$

Consequently a static record ($m_i=0$) has
$\boldsymbol\mu_i(t)=\boldsymbol\mu_i^0$ and $g_i(t)=1$ for every time. A
moving record has linear mean motion and is most visible at its time center.
This explicit first-order model is deliberately inspectable; it is not a
learned deformation network.

## GPU command path

`main.js` creates the primitive storage buffer, 32-byte `TimeState` uniform
binding, bind groups, render pipeline and command encoder directly. WGSL aligns
the `vec3<f32>` padding member to byte 16, so the structure occupies 32 bytes
even though each update writes only the first 16 bytes. The records are stored in
near-to-far depth order and use Lesson 03's front-to-back transmittance blend.
Changing the slider calls `queue.writeBuffer` for the time uniform; it does not
rebuild a pipeline or upload new primitives.

## Run and interact

From `lessons`:

```bash
npm ci
npm run dev
```

Open `http://127.0.0.1:5173/04-explicit-time/`. Drag the time slider, click
play/pause, or press Space. Left and Right Arrow step by 0.02. The blue reference
splats stay fixed; orange, green and pink splats move and fade.

## Verifiable assertions

`evaluatePrimitive` in `reference.js` is the CPU reference. Before rendering,
the page dispatches `evaluate_for_validation` for every primitive at
$t\in\{0.25,0.5,0.75\}$, copies the storage output to a mapped buffer and
compares every mean, gate and opacity value. A pass requires maximum absolute
CPU/WGSL error below $10^{-4}$ and also checks:

- record stride is 64 bytes and every duration is positive;
- depth records are monotonic near-to-far;
- static means and gates are time-invariant;
- every moving mean changes over the reference interval; and
- a moving gate is symmetric around its time center and peaks there.

The full result is published as `window.__LESSON_RESULT__`. Its details include
the current time, static/moving counts, record stride and measured CPU/WGSL
error.

## Modification experiment

Set the orange primitive's velocity to `[0.3, 0.8]` in `main.js`. Scrub from
0 to 1 and observe the changed linear path while the static references remain
fixed. The GPU/CPU comparison continues to pass because the record, reference
equation and shader all receive the new value through the existing data path.

## Expected failure experiment

In `explicit-time.wgsl`, change

```wgsl
let normalized_time = delta_time / safe_duration;
```

to `let normalized_time = delta_time;`. The renderer still produces moving
splats, so visual inspection alone can miss the changed units. The compute
readback disagrees with `reference.js`, `cpuAndWgslEvaluationsMatch` becomes
false, and the lesson reports `FAIL`.

## Common failures

- **Everything moves:** static records must set `moving: 0`; zero velocity alone
  does not state the temporal-gate invariant.
- **Static splats fade:** the static branch must select a gate of exactly one.
- **Scrubbing uploads the scene:** only the first 16 bytes of the 32-byte time
  binding should be written; the primitive storage buffer is immutable after
  initialization.
- **CPU/WGSL mismatch near a time center:** compare absolute float error and
  retain the same positive-duration clamp on both sides.

## Next step — Lesson 05

This lesson establishes the temporal data contract and evaluation invariant.
It does not prescribe how records were trained, streamed or compressed. A
production 4DGS system may replace linear velocity with deformation bases or a
network while retaining the same test pattern: define the reference equation,
evaluate both sides at fixed times, and compare observable outputs. Lesson 05
will use the gate and view bounds to construct a compact active set.
