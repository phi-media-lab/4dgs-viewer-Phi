# Course roadmap

## Curriculum

Each lesson must answer one falsifiable question and expose the code that
answers it:

| Lesson | Question | Required visible result | Status |
| --- | --- | --- | --- |
| 00 Environment | Can this browser compile WGSL, create a pipeline and submit work? | RGB triangle and machine-readable PASS/FAIL | Implemented |
| 01 One Gaussian | How does one record become an analytic elliptical footprint? | One controllable Gaussian; CPU/WGSL agreement | Planned |
| 02 Projection | How do 3D covariance and the camera Jacobian produce a 2D conic? | Near/far, rotation and anisotropy cases | Planned |
| 03 Order and blend | Why does transparent splatting require depth order? | Intentionally wrong versus correct overlap | Planned |
| 04 Explicit time | How do velocity, center, duration and opacity gate define a 4D primitive? | Scrubbable static and moving primitives | Planned |
| 05 Active set | How does the GPU reduce total records to active and visible records? | Auditable `N → A → V` counters | Planned |
| 06 Complete synthetic asset | How do loading, validation, sorting and rendering compose? | Synthetic conformance asset | Planned |

The order is conceptual, not a feature checklist. Radix sorting is introduced
only after a small fixed-order example makes the correctness requirement
visible. Remote streaming belongs to the Player documentation, not inside the
browser WebGPU renderer lessons.

## Code-surface rule

- VS Code is the code and explanation surface; the browser is the render and
  interaction surface.
- `infra/` may contain only adapter/device setup, checked error scopes, canvas
  resize and the PASS/FAIL contract.
- Mathematical data structures, reference equations and shader stages taught
  by a lesson must be reachable directly from that lesson's `main.js`.
- A lesson may build on the preceding lesson, but must not call a preassembled
  full renderer that hides the current topic.
- Browser chrome stays limited to the canvas, interaction hints, status and
  actionable errors. No simulated editor, file tree, cards or dashboard.

## Acceptance criteria

1. Source-contract test for imports, entry points and base-path safety.
2. Deterministic CPU reference or invariant for the mathematical step.
3. Production Vite build under both relative and repository base paths.
4. Real hardware WebGPU run that publishes `window.__LESSON_RESULT__`.
5. One deliberate failure experiment whose error is visible and recoverable
   through live reload without a manual browser refresh.
