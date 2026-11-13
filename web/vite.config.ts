import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// The BlackBox server (`blackbox serve`) listens on 127.0.0.1:8787 and owns everything under /api.
const API_TARGET = 'http://127.0.0.1:8787';

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': { target: API_TARGET, changeOrigin: false },
    },
  },
  preview: {
    proxy: {
      '/api': { target: API_TARGET, changeOrigin: false },
    },
  },
});
