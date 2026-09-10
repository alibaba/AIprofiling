// SPDX-License-Identifier: Apache-2.0
// TypeScript declarations for `aiprof-perfetto` — the vite alias points at
// src/stubs/aiprof-perfetto.ts, which embeds the public Perfetto UI.
declare module 'aiprof-perfetto' {
  import * as React from 'react';
  export function InitPerfetto(
    opts?: { callback?: () => void },
    extra?: unknown,
  ): Promise<void>;
  export function OpenTraceFromUrl(url: string, extra?: unknown): void;
  export const Tracing: React.FC<
    { url?: string; style?: React.CSSProperties } & Record<string, unknown>
  >;
  const def: {
    InitPerfetto: typeof InitPerfetto;
    OpenTraceFromUrl: typeof OpenTraceFromUrl;
    Tracing: typeof Tracing;
  };
  export default def;
}
