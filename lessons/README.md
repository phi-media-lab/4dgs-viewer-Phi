# 4DGS WebGPU lessons

Seven directly runnable lessons build a 4D Gaussian rendering path from native
JavaScript and WGSL. Read and edit the source in VS Code; use the browser for
WebGPU execution, visualization and interaction.

| Lesson | Topic | Observable result |
| --- | --- | --- |
| [00 Environment](00-environment/LESSON.md) | Adapter, device, shader, pipeline, render pass and queue | RGB triangle and completed GPU submission |
| [01 One Gaussian](01-one-gaussian/LESSON.md) | Analytic 2D Gaussian footprint | Interactive ellipse and CPU/WGSL alpha agreement |
| [02 Projection](02-projection/LESSON.md) | Camera Jacobian and covariance projection | Near/far, rotated and anisotropic conics |
| [03 Order and blend](03-order-blend/LESSON.md) | Front-to-back transmittance | Switchable correct/reversed overlap and pixel readback |
| [04 Explicit time](04-explicit-time/LESSON.md) | Mean velocity and temporal opacity | Scrubbable static and moving primitives |
| [05 Active set](05-active-set/LESSON.md) | Active/visible compaction | Audited `N → A → V` counters and indirect draw |
| [06 Complete pipeline](06-complete-pipeline/LESSON.md) | Validation, projection, sorting and rendering | Complete synthetic 4DGS frame with GPU/CPU audit |

Each lesson owns the WebGPU resources, pipelines and command sequence needed
for its topic. Shared `infra/` code is limited to device/canvas setup, checked
error scopes and the result surface. Inputs are source constants or procedural
records; the course contains no model or media payload and makes no external
asset request by default.

## Run

Requirements: Node.js `^20.19.0` or `>=22.12.0`, plus a current WebGPU-capable
Chrome or Chromium build.

```bash
code-insiders 4dgs-viewer-phi.code-workspace # or: code ...
npm ci
npm run dev:open
```

`dev:open` opens the catalog at `http://127.0.0.1:5173/`. Choose a lesson there;
Vite reloads the page when its JavaScript, WGSL, HTML or CSS changes. Each
`LESSON.md` names the files to open, the interaction, numerical assertions and
a deliberate failure experiment.

## Verify

```bash
npm test
npm run build
npx vite build --base=/4dgs-viewer-Phi/ --outDir=dist-pages
```

The tests check all seven entries, their lesson-owned command chains, relative
resource URLs and the absence of bundled model/media assets. The two builds
exercise both relative deployment and a repository base path.

In a WebGPU browser, open each lesson and inspect:

```js
window.__LESSON_RESULT__
```

`status: 'PASS'` is published only after that lesson's stated GPU and numerical
invariants hold, and every published assertion must be exactly `true`. It is not
a performance claim. Hosted CI covers source contracts and builds; execute the
pages in local Chrome or a controlled GPU browser lane for WebGPU validation.

Workflow inspiration: [WebGPU Unleashed](https://github.com/shi-yan/webgpuunleashed)
by Shi Yan.
