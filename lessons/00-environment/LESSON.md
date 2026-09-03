# Lesson 00 — WebGPU environment

## Learning goal

Build and verify the smallest useful WebGPU host chain:

```text
GPUAdapter → GPUDevice → GPUCanvasContext
           → shader module → render pipeline
           → command encoder → render pass → queue
```

The output is deliberately a plain RGB triangle. There is no Gaussian data or
4DGS renderer in this lesson.

## Prerequisites

- A current Chrome or Chromium build with hardware acceleration enabled.
- Node.js `^20.19.0` or `>=22.12.0`, matching Vite 8's supported range.
- Basic JavaScript module syntax.

## Open these files

1. `main.js` — the host-side resource and command sequence.
2. `environment.wgsl` — one vertex entry point and one fragment entry point.
3. `../infra/gpu.js` — the small reusable WebGPU boundary.
4. `index.html` — canvas, diagnostics and module entry only.

## Coordinate contract

The vertex shader emits normalized device coordinates (NDC). The viewport maps
them to canvas pixels:

$$
x_{px} = \frac{x_{ndc} + 1}{2} W, \qquad
y_{px} = \frac{1 - y_{ndc}}{2} H.
$$

No vertex buffer is needed: `@builtin(vertex_index)` selects three constant
positions in WGSL. The render pass clears the canvas and `draw(3)` emits the
triangle.

## Run and interact

From the `lessons` directory:

```bash
npm ci
npm run dev
```

Open `http://127.0.0.1:5173/00-environment/`. Resize the browser to force canvas
reconfiguration. Press `H` to hide or show diagnostics. Saving JavaScript, WGSL,
HTML or CSS triggers Vite live reload.

## Verifiable assertions

After the first submitted frame, the page publishes:

```js
window.__LESSON_RESULT__ = {
  lesson: 0,
  status: 'PASS',
  assertions: {
    webGpuAvailable: true,
    adapterCreated: true,
    deviceCreated: true,
    canvasConfigured: true,
    shaderCompiled: true,
    shaderErrorCountIsZero: true,
    pipelineCreated: true,
    frameSubmitted: true,
    gpuWorkCompleted: true,
  },
  details: {
    adapter: 'browser-reported adapter description',
    format: 'browser-preferred canvas format',
    width: 1280,
    height: 720,
    warningCount: 0,
    frameCount: 1,
  },
};
```

`PASS` means pipeline creation succeeded, a frame was submitted, and
`queue.onSubmittedWorkDone()` resolved without a captured validation or
out-of-memory error. It is an execution assertion, not a performance claim.

## Modification experiment

Change one color in the `colors` array in `environment.wgsl`, save the file and
observe both the automatic reload and the new interpolated triangle color.

## Expected failure experiment

Temporarily rename `fs_main` to `fs_broken`. The browser should display a `FAIL`
surface rather than a blank canvas. Undo the change and save; Vite should reload
the valid lesson.

## Common failures

- **WebGPU unavailable:** update the browser and confirm hardware acceleration.
- **No adapter:** inspect `chrome://gpu`; a policy or blocklist may be active.
- **WGSL line/column error:** open `environment.wgsl` at the reported location.
- **Blank output with PASS:** inspect browser compositing and canvas visibility;
  the GPU command path has completed, but a screenshot gate is still needed to
  prove final presentation.

## Interface to Lesson 01

Lesson 01 keeps this Device/Canvas/Pipeline/Pass chain. It replaces the three
constant vertices with one Gaussian record and an analytic splat footprint. It
still does not introduce radix sort, indirect draw or explicit time.
