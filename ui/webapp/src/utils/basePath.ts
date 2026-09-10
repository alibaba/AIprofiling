// SPDX-License-Identifier: Apache-2.0
//
// BASE_PATH-aware URL helper. Every server-relative URL the webapp builds
// ("/api/...", "/ws", "/resource/...") must carry the deployment prefix so a
// reverse proxy at `location /aiprof-open/` reaches the right server. Vite
// injects the trailing-slash-normalized prefix via `import.meta.env.BASE_URL`
// (defaults to "/" when unset), so `withBase('/api/x')` yields "/api/x" for
// root-mounted builds and "/aiprof-open/api/x" for prefixed builds.

// Vite guarantees a trailing slash. Strip it so "/aiprof-open/" + "/api" =
// "/aiprof-open/api", not "/aiprof-open//api" (nginx tolerates it, some
// clients don't).
const BASE = (import.meta.env.BASE_URL || '/').replace(/\/+$/, '');

export function withBase(path: string): string {
  if (!path) return BASE || '/';
  if (/^(https?:|wss?:|blob:|data:)/i.test(path)) return path;
  if (path.startsWith(BASE + '/') || path === BASE) return path;
  if (path.startsWith('/')) return `${BASE}${path}`;
  return `${BASE}/${path}`;
}
