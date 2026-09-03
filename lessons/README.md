# 4DGS WebGPU lessons

This staging slice contains only Lesson 00. The extraction/rewrite decision for
the remaining curriculum is recorded in [`ROADMAP.md`](ROADMAP.md). Lesson 00
intentionally has no dependency
on client-side Gaussian streaming, progressive loading, 4DGS assets or a UI
framework.

```bash
code-insiders 4dgs-viewer-phi.code-workspace # or: code ...
npm ci
npm run dev
```

Read the source in VS Code and open
`http://127.0.0.1:5173/00-environment/` in the browser. Vite owns development
serving and live reload; the lesson remains native JavaScript WebGPU plus WGSL.

Verification:

```bash
npm test
npm run build
npx vite build --base=/4dgs-viewer-Phi/ --outDir=dist-pages
```

The last command exercises a GitHub Pages-style repository base path. A real GPU
browser smoke should wait for `window.__LESSON_RESULT__.status` and require
`PASS`; source tests and a software browser cannot certify hardware WebGPU.
