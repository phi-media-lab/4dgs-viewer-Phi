import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';

export default defineConfig({
  // A relative default works both at / and below a repository path. A release
  // can still set an explicit path with: vite build --base=/repository-name/
  base: './',
  server: {
    host: '127.0.0.1',
    port: 5173,
    strictPort: true,
  },
  preview: {
    host: '127.0.0.1',
    port: 4173,
    strictPort: true,
  },
  build: {
    assetsInlineLimit: 0,
    rollupOptions: {
      input: {
        index: fileURLToPath(new URL('./index.html', import.meta.url)),
        lesson00: fileURLToPath(new URL('./00-environment/index.html', import.meta.url)),
        lesson01: fileURLToPath(new URL('./01-one-gaussian/index.html', import.meta.url)),
        lesson02: fileURLToPath(new URL('./02-projection/index.html', import.meta.url)),
        lesson03: fileURLToPath(new URL('./03-order-blend/index.html', import.meta.url)),
        lesson04: fileURLToPath(new URL('./04-explicit-time/index.html', import.meta.url)),
        lesson05: fileURLToPath(new URL('./05-active-set/index.html', import.meta.url)),
        lesson06: fileURLToPath(new URL('./06-complete-pipeline/index.html', import.meta.url)),
      },
    },
  },
});
