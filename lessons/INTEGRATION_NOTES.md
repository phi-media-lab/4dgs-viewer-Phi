# Lesson 00 integration notes

## Minimal public tree

```text
lessons/
├─ package.json
├─ package-lock.json
├─ vite.config.js
├─ index.html
├─ 00-environment/
│  ├─ LESSON.md
│  ├─ index.html
│  ├─ main.js
│  └─ environment.wgsl
├─ infra/
│  ├─ gpu.js
│  ├─ page.js
│  └─ style.css
└─ tests/
   └─ source-contract.test.js
```

The workspace and VS Code task files are convenient development metadata, not
runtime dependencies. `infra/` must stay limited to environment setup, checked
error scopes, resize, diagnostics and the result contract. Shader-module,
pipeline, encoder and queue calls stay visible in the lesson that teaches them.
Do not move the excluded client-side streaming runtime into `infra/`.

## Checks already represented in the prototype

- Relative source URLs; no `/shared`, `/shaders` or other root-absolute source
  paths.
- Vite multi-page production build with external WGSL assets.
- Default relative base and an explicit repository base.
- No Client GS/progressive/tile/telemetry imports.
- A machine-readable `window.__LESSON_RESULT__` contract.
- Captured shader, pipeline, submission, validation, out-of-memory and internal
  failures, plus a FAIL transition for uncaptured device errors.
- No continuous GPU render loop: the lesson submits an initial frame and
  redraws only when CSS size or device-pixel ratio changes. A lightweight rAF
  watcher compares the DPR scalar because browsers do not expose one reliable
  cross-platform DPR-change event.

## Merge gates to add in the public staging repository

### Source and build gate

```bash
npm ci
npm test
npm run build
npx vite build --base=/4dgs-viewer-Phi/ --outDir=dist-pages
```

Serve `dist-pages` at `/4dgs-viewer-Phi/` and require HTTP 200 for both HTML documents,
their CSS/JS bundles and the emitted `.wgsl` asset.

### Real-browser hardware gate

Run this on the Apple GPU lane, not a generic software-browser job:

1. Open `/00-environment/` and collect console/page errors.
2. Wait until `window.__LESSON_RESULT__.status` is `PASS` or `FAIL`.
3. Require every assertion to be exactly `true`.
4. Require zero uncaptured WebGPU and console errors.
5. Resize the viewport and require `details.frameCount` to increase.
6. Change one WGSL color in a temporary checkout and require Vite to reload and
   return to `PASS` without a manual refresh.

A later visual golden can sample a small center region of the RGB triangle, but
the v0 source contract should not imply that command completion alone proves
browser presentation.

## Integration rule

Treat this directory as a rewrite/reference, not an allowlisted copy of the old
lesson. Preserve its dependency boundary when branding, licensing and the other
lessons are added.
