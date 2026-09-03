# Course map

## Curriculum

All seven lessons are implemented and directly runnable. Each answers one
falsifiable question and exposes the code that answers it:

| Lesson | Question | Required visible result | Status |
| --- | --- | --- | --- |
| 00 Environment | Can this browser compile WGSL, create a pipeline and submit work? | RGB triangle and machine-readable PASS/FAIL | Implemented |
| 01 One Gaussian | How does one record become an analytic elliptical footprint? | One controllable Gaussian; CPU/WGSL agreement | Implemented |
| 02 Projection | How do 3D covariance and the camera Jacobian produce a 2D conic? | Near/far, rotation and anisotropy cases | Implemented |
| 03 Order and blend | Why does transparent splatting require depth order? | Intentionally wrong versus correct overlap | Implemented |
| 04 Explicit time | How do velocity, center, duration and opacity gate define a 4D primitive? | Scrubbable static and moving primitives | Implemented |
| 05 Active set | How does the GPU reduce total records to active and visible records? | Auditable `N → A → V` counters | Implemented |
| 06 Complete synthetic pipeline | How do input validation, projection, sorting and rendering compose? | Procedural record envelope through the complete pipeline | Implemented |

The order is conceptual, not a feature checklist. Lesson 03 makes ordering
correctness visible with three CPU-ordered records; Lesson 06 implements a
lesson-owned bitonic GPU sort. Remote streaming belongs to the Player
documentation, not inside the browser WebGPU renderer lessons.

## Asset boundary

Lessons 00–06 use source constants and deterministic procedural records. No
model, checkpoint, image or video payload is bundled. Lesson 06 defines an
external manifest slot, leaves it `null`, and runs its strict loader and record
validation against a procedural envelope. Loading a real asset is an adapter at
that boundary, not a prerequisite for the course.

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

## Verification contract

1. Source-contract test for imports, entry points and base-path safety.
2. Deterministic CPU reference or invariant for the mathematical step.
3. Production Vite build under both relative and repository base paths.
4. Real hardware WebGPU run that publishes `window.__LESSON_RESULT__`.
5. One deliberate failure experiment whose error is visible and recoverable
   through live reload without a manual browser refresh.
