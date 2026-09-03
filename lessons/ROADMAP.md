# Course extraction roadmap

## What is publishable now

Lesson 00 is the only extracted lesson in this staging tree. It contains the
complete host-side resource/command chain and its exact WGSL, rather than
delegating the subject to a hidden renderer. It has passed source/build checks
and an exploratory real-Chrome run; the tagged release still needs its own
hardware-browser receipt.

The former workspace directories named Lesson 01–05 are not copied here. Their
entry files were thin wrappers around a large shared renderer, so copying them
would preserve the lesson labels without exposing the implementation being
taught. The former asset lesson is also excluded because its FreeTimeGS++ input
does not have publication permission in this repository.

## Curriculum rule

Each lesson must answer one falsifiable question and expose the code that
answers it:

| Lesson | Question | Required visible result |
| --- | --- | --- |
| 00 Environment | Can this browser compile WGSL, create a pipeline and submit work? | RGB triangle and machine-readable PASS/FAIL |
| 01 One Gaussian | How does one record become an analytic elliptical footprint? | One controllable Gaussian; CPU/WGSL agreement |
| 02 Projection | How do 3D covariance and the camera Jacobian produce a 2D conic? | Near/far, rotation and anisotropy cases |
| 03 Order and blend | Why does transparent splatting require depth order? | Intentionally wrong versus correct overlap |
| 04 Explicit time | How do velocity, center, duration and opacity gate define a 4D primitive? | Scrubbable static and moving primitives |
| 05 Active set | How does the GPU reduce total records to active and visible records? | Auditable `N → A → V` counters |
| 06 Complete synthetic asset | How do loading, validation, sorting and rendering compose? | The public conformance target only |

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

## Gate for every new lesson

1. Source-contract test for imports, entry points and base-path safety.
2. Deterministic CPU reference or invariant for the mathematical step.
3. Production Vite build under both relative and repository base paths.
4. Real hardware WebGPU run that publishes `window.__LESSON_RESULT__`.
5. One deliberate failure experiment whose error is visible and recoverable
   through HMR without a manual browser refresh.
