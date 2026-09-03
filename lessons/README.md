# 4DGS WebGPU lessons

Lesson 00 implements the minimal WebGPU host/shader pipeline with native
JavaScript and WGSL. The planned progression from one Gaussian to a complete
synthetic 4DGS asset is listed in [`ROADMAP.md`](ROADMAP.md).

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

The last command exercises a GitHub Pages-style repository base path. On a
WebGPU-capable browser, wait for `window.__LESSON_RESULT__.status` and require
`PASS` to confirm that the GPU command chain completed.

Workflow inspiration: [WebGPU Unleashed](https://github.com/shi-yan/webgpuunleashed)
by Shi Yan.
