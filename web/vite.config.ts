import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// The BlackBox server (`blackbox serve`) listens on 127.0.0.1:8787 and owns everything under /api.
// It refuses requests whose Host or Origin is not its own (403 `forbidden`). `changeOrigin`
// rewrites the proxied Host header to the target; the browser's `Origin: http://localhost:5173`
// is still forwarded, so start the server with `blackbox serve --allow-origin http://localhost:5173`
// for `npm run dev` (`http://localhost:4173` for `npm run preview`).
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
});
