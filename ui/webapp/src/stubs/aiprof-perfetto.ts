// SPDX-License-Identifier: Apache-2.0
// aiprof-perfetto — embeds the public Perfetto UI (https://ui.perfetto.dev)
// via an <iframe> and drives it with the documented postMessage handshake:
//   https://perfetto.dev/docs/visualization/deep-linking-to-perfetto-ui
//
// Exports:
//   - Tracing        : React component that mounts the Perfetto iframe.
//   - InitPerfetto   : records `callback` (invoked when the iframe is ready)
//                      and resolves so the caller can render <Tracing />.
//   - OpenTraceFromUrl(url): fetches `url` as ArrayBuffer (same-origin, so no
//                      CORS on the trace file is needed) and posts the buffer
//                      to the Perfetto iframe via the PING/PONG handshake.
import React, { useEffect, useRef } from 'react';

const PERFETTO_ORIGIN = 'https://ui.perfetto.dev';

type ModuleState = {
  iframeWindow: Window | null;
  iframeReady: boolean;
  pendingTraceUrl: string | null;
  readyCallbacks: Array<() => void>;
};

const state: ModuleState = {
  iframeWindow: null,
  iframeReady: false,
  pendingTraceUrl: null,
  readyCallbacks: [],
};

function log(...args: unknown[]) {
  // eslint-disable-next-line no-console
  console.info('[perfetto-shim]', ...args);
}

async function flushPendingTrace() {
  if (!state.iframeReady || !state.iframeWindow || !state.pendingTraceUrl) {
    return;
  }
  const url = state.pendingTraceUrl;
  state.pendingTraceUrl = null;
  try {
    log('fetching trace', url);
    const resp = await fetch(url);
    if (!resp.ok) {
      throw new Error(`fetch trace failed: ${resp.status} ${resp.statusText}`);
    }
    const buffer = await resp.arrayBuffer();
    const fileName = url.split('/').pop() || 'trace.json';
    log(`posting ${(buffer.byteLength / 1024).toFixed(1)} KB buffer to perfetto`);
    state.iframeWindow.postMessage(
      {
        perfetto: {
          buffer,
          title: `AIProf: ${fileName}`,
          fileName,
        },
      },
      PERFETTO_ORIGIN,
    );
  } catch (err) {
    log('failed to load trace:', err);
  }
}

function markReady(win: Window) {
  if (state.iframeReady && state.iframeWindow === win) return;
  state.iframeWindow = win;
  state.iframeReady = true;
  log('perfetto iframe ready');
  state.readyCallbacks.slice().forEach((cb) => {
    try {
      cb();
    } catch (e) {
      log('ready callback threw:', e);
    }
  });
  void flushPendingTrace();
}

function markUnmounted(win: Window) {
  if (state.iframeWindow === win) {
    state.iframeWindow = null;
    state.iframeReady = false;
  }
}

// InitPerfetto({ callback }, _extra) — records the callback (invoked once the
// iframe is ready, or immediately if it already is) and resolves so the caller
// can proceed to render <Tracing />. The real package returns a Promise.
export function InitPerfetto(
  opts: { callback?: () => void } = {},
  _extra?: unknown,
): Promise<void> {
  const cb = opts?.callback;
  if (typeof cb === 'function') {
    if (state.iframeReady) {
      queueMicrotask(cb);
    } else {
      state.readyCallbacks.push(cb);
    }
  }
  return Promise.resolve();
}

// OpenTraceFromUrl(url) — stores the URL. If the iframe is already ready, load
// immediately; otherwise the URL is loaded as soon as the iframe reports ready.
export function OpenTraceFromUrl(url: string, _extra?: unknown): void {
  if (!url || typeof url !== 'string') {
    log('OpenTraceFromUrl: invalid url', url);
    return;
  }
  state.pendingTraceUrl = url;
  if (state.iframeReady) {
    void flushPendingTrace();
  } else {
    log('OpenTraceFromUrl queued (iframe not yet ready):', url);
  }
}

type TracingProps = {
  url?: string;
  style?: React.CSSProperties;
  [k: string]: unknown;
};

export const Tracing: React.FC<TracingProps> = ({ url, style }) => {
  const iframeRef = useRef<HTMLIFrameElement | null>(null);
  const pingRef = useRef<number | null>(null);
  const readyRef = useRef(false);

  useEffect(() => {
    const onMessage = (evt: MessageEvent) => {
      if (evt.origin !== PERFETTO_ORIGIN) return;
      if (evt.data !== 'PONG') return;
      if (!iframeRef.current) return;
      const win = iframeRef.current.contentWindow;
      if (!win) return;
      readyRef.current = true;
      if (pingRef.current !== null) {
        window.clearInterval(pingRef.current);
        pingRef.current = null;
      }
      markReady(win);
    };
    window.addEventListener('message', onMessage);
    return () => {
      window.removeEventListener('message', onMessage);
      if (pingRef.current !== null) {
        window.clearInterval(pingRef.current);
        pingRef.current = null;
      }
      const win = iframeRef.current?.contentWindow;
      if (win) markUnmounted(win);
    };
  }, []);

  useEffect(() => {
    if (url) OpenTraceFromUrl(url);
  }, [url]);

  const handleLoad = () => {
    if (pingRef.current !== null) window.clearInterval(pingRef.current);
    const win = iframeRef.current?.contentWindow;
    if (!win) return;
    pingRef.current = window.setInterval(() => {
      if (readyRef.current) return;
      try {
        win.postMessage('PING', PERFETTO_ORIGIN);
      } catch {
        /* iframe unloaded */
      }
    }, 100);
  };

  return React.createElement(
    'div',
    {
      style: {
        width: '100%',
        height: '100%',
        minHeight: 600,
        display: 'flex',
        ...(style ?? {}),
      },
    },
    React.createElement('iframe', {
      ref: iframeRef,
      src: PERFETTO_ORIGIN,
      onLoad: handleLoad,
      title: 'Perfetto UI',
      style: { width: '100%', minHeight: 600, border: 'none', flex: 1 },
      allow: 'cross-origin-isolated',
    }),
  );
};

export default { InitPerfetto, OpenTraceFromUrl, Tracing };
