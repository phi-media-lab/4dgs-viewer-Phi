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
      },
    },
  },
});
