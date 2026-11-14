import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

// The API client calls `VITE_API_BASE_URL` (the hosted API on Render, inlined at build time) or,
// when it is unset, the same-origin `/api/v1` of a local `blackbox serve` on 127.0.0.1:8787.
// The server refuses requests whose Host or Origin is not its own (403 `forbidden`).
// `changeOrigin` rewrites the proxied Host header to the target; the browser's
// `Origin: http://localhost:5173` is still forwarded, so allow that origin on the server for
// `npm run dev` (`http://localhost:4173` for `npm run preview`).
const API_TARGET = 'http://127.0.0.1:8787';

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': { target: API_TARGET, changeOrigin: true },
    },
  },
  preview: {
    proxy: {
      '/api': { target: API_TARGET, changeOrigin: true },
    },
  },
  test: {
    include: ['src/**/*.test.ts'],
    environment: 'node',
  },
});
