# Lesson 05 — Active-set compaction

## Learning goal

Make the reduction from all records to submitted primitives observable:

```text
N total records → A temporally active records → V viewport-visible records
                                                ↓
                                         drawIndirect(V)
```

This lesson owns both compute passes, their atomic counters, the compacted index
buffers, the indirect draw arguments and the render pass. The procedural input
contains 64 records and no external asset. Its canvas is an order-independent
additive diagnostic of the selected set, not transparent source-over output.

## Prerequisites

- Complete Lessons 00–04, or be comfortable with WebGPU storage buffers,
  compute dispatch and the explicit-time opacity model.
- A current Chrome or Chromium build with hardware acceleration enabled.
- Node.js `^20.19.0` or `>=22.12.0`.

## Open these files

1. `reference.js` — deterministic records and the CPU classification oracle.
2. `active-set.wgsl` — reset, active compaction and visible compaction stages.
3. `active-set-render.wgsl` — read-only compacted indices and analytic splat stages.
4. `main.js` — buffer ownership, pass order, indirect draw and readback audit.
5. `index.html` — canvas, diagnostics and module entry only.

## Mathematical model

For record opacity $o_i$, temporal center $\mu_{t,i}$ and temporal standard
deviation $\sigma_{t,i}$, the opacity at time $t$ is

$$
\alpha_i(t) = o_i \exp\left[-\frac{1}{2}
\left(\frac{t-\mu_{t,i}}{\sigma_{t,i}}\right)^2\right].
$$

The first compute pass appends index $i$ when

$$
\alpha_i(t) \ge \alpha_{min}.
$$

The second pass tests the $3\sigma$ axis-aligned footprint against the NDC
viewport. For center $(x_i,y_i)$ and scale $(\sigma_{x,i},\sigma_{y,i})$:

$$
|x_i| \le 1 + 3\sigma_{x,i}, \qquad
|y_i| \le 1 + 3\sigma_{y,i}.
$$

Only an active record that satisfies both inequalities enters the visible list.

## Buffer and command contract

- Every source record is read once by `active_main`.
- `atomicAdd(counters.active_count)` reserves one unique active-list slot.
- `visible_main` reads exactly the first `active` slots.
- Its two atomic increments keep `counters.visible_count` and indirect
  `instance_count` equal.
- Separate compute passes establish the order `reset → active → visible`.
- The render pass calls `drawIndirect`; it never substitutes the CPU count.

Atomic allocation does not promise a stable index order. The audit therefore
sorts the two read-back index sets before comparing them with the CPU reference.
It verifies membership and uniqueness without inventing an ordering guarantee.

For the same reason, this lesson must not apply order-dependent transparent
source-over to the compacted list. The fragment shader emits premultiplied
$\alpha_i\mathbf{c}_i$, and the attachment accumulates

$$
\mathbf{C}_{new}=\mathbf{C}_{old}+\alpha_i\mathbf{c}_i.
$$

Addition makes the diagnostic independent of atomic slot order. It deliberately
shows contribution density rather than physically correct transparency. Lesson
06 first establishes far-to-near depth order and only then uses source-over.

## Run and interact

From `lessons/`:

```bash
npm ci
npm run dev
```

Open `http://127.0.0.1:5173/05-active-set/`.

- `←` / `→` changes time and reruns both compaction passes.
- `H` hides or shows diagnostics.
- Saving a source file triggers Vite live reload.

The HUD reports the audited `N → A → V` values. The canvas contains only the
visible set consumed by the indirect draw, accumulated as an additive preview.

## Verifiable assertions

After each completed frame, `window.__LESSON_RESULT__` reports `PASS` only when:

- the three GPU counters equal the CPU reference counts;
- both compacted GPU index sets equal their CPU reference sets;
- the indirect instance count equals the visible count;
- the display pipeline uses an order-independent premultiplied additive blend;
- submitted GPU work completes without a captured error.

This is a correctness check for a small teaching input, not a throughput
benchmark. Production renderers normally avoid readback in their frame loop.

## Modification experiment

Change `ALPHA_MIN` in `reference.js` from `0.08` to `0.16`. The active and
visible counts decrease, while the CPU/GPU set audit should continue to pass.
This separates a policy change from a compaction bug.

## Expected failure experiment

In `active-set.wgsl`, temporarily remove the line in `visible_main` that
increments `draw_args.instance_count`. WebGPU can still execute the shader, but the page
must report `FAIL` because the visible counter and indirect draw count diverge.
Restore the line and save; live reload returns to the audited path.

## Common failures

- **Counters stay at zero:** confirm the reset, active and visible passes use
  separate command passes in that order.
- **Random index order:** expected; atomic slot allocation is unordered. Compare
  sets unless a later stage explicitly sorts them.
- **Overlap looks brighter than source-over:** expected; additive blending is a
  diagnostic chosen because this lesson has not established depth order.
- **Validation error on indirect draw:** the arguments buffer needs both
  `STORAGE` and `INDIRECT` usage.
- **Canvas and HUD disagree:** the audit treats the indirect instance count as
  part of the same contract as the visible counter.

## Interface to Lesson 06

Lesson 06 replaces these already-projected screen-space records with validated
3D records. It applies explicit time and covariance projection, assigns invalid
records a zero-contribution sort entry, establishes far-to-near order and only
then applies transparent source-over. The small fixed sort in that lesson remains
inspectable; a production renderer can place this lesson's compaction stage in
front of a scalable ordering stage.
