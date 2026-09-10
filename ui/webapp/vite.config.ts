// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'node:path';

const apiTarget = process.env.AIPROF_DASH_URL || 'http://127.0.0.1:7000';

// BASE_PATH mirrors the server's BASE_PATH env. When set (e.g. "/aiprof-open"),
// Vite emits absolute asset URLs prefixed with it, React Router mounts under
// it, and every /api/* /ws /resource/* URL the client builds carries the same
// prefix so a `location /aiprof-open/` reverse proxy hits the right server.
const rawBase = (process.env.BASE_PATH || '').replace(/\/+$/, '');
const base = rawBase ? `${rawBase}/` : '/';

export default defineConfig({
  plugins: [react()],
  base,
  resolve: {
    alias: {
      '@': path.resolve(__dirname, 'src'),
      // aiprof-perfetto: local Perfetto UI shim (embeds ui.perfetto.dev via <iframe>).
      'aiprof-perfetto': path.resolve(__dirname, 'src/stubs/aiprof-perfetto.ts'),
    },
  },
  build: {
    outDir: 'dist',
    sourcemap: false,
    chunkSizeWarningLimit: 2048,
  },
  server: {
    host: '0.0.0.0',
    port: 5173,
    proxy: {
      // dev-server proxies both / and /${BASE_PATH} through to the backend so
      // dev parity holds regardless of whether BASE_PATH is set for the build.
      '/api': { target: apiTarget, changeOrigin: true },
      ...(rawBase ? { [rawBase]: { target: apiTarget, changeOrigin: true } } : {}),
    },
  },
});
