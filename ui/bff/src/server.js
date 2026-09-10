// SPDX-License-Identifier: Apache-2.0
//
// AIProf BFF — a small Node.js proxy that fronts the three FastAPI
// services with a single origin (so the browser sees /api/... and no
// CORS gymnastics). Written in plain Node stdlib to keep the OSS
// dependency graph tiny.

'use strict';

const http = require('http');
const { URL } = require('url');
const fs   = require('fs');
const path = require('path');

const adapters = require('./adapters/uiImages');
const staticServer = require('./static');

const PORT       = parseInt(process.env.AIPROF_BFF_PORT || process.env.PORT || '9100', 10);
const QUERY_URL  = process.env.AIPROF_QUERY_URL  || 'http://localhost:7102';
const REPORT_URL = process.env.AIPROF_REPORT_URL || 'http://localhost:7103';

// Prefer the built webapp; fall back to the legacy vanilla SPA if not built.
const WEBAPP_DIST = path.join(__dirname, '..', '..', 'webapp', 'dist');
const LEGACY_DIST = path.join(__dirname, '..', '..', 'frontend', 'dist');
const STATIC_DIR = process.env.AIPROF_STATIC_DIR
    || (fs.existsSync(path.join(WEBAPP_DIST, 'index.html')) ? WEBAPP_DIST : LEGACY_DIST);

// Very small proxy: forward method/body/headers, stream response back.
function proxy(target, req, res) {
    const u = new URL(target);
    const opts = {
        method: req.method,
        hostname: u.hostname,
        port: u.port || (u.protocol === 'https:' ? 443 : 80),
        path: u.pathname + u.search,
        headers: { ...req.headers, host: u.host },
    };
    const upstream = http.request(opts, (r) => {
        res.writeHead(r.statusCode || 502, r.headers);
        r.pipe(res);
    });
    upstream.on('error', (err) => {
        res.writeHead(502, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ error: 'upstream_error', detail: String(err) }));
    });
    req.pipe(upstream);
}

const server = http.createServer((req, res) => {
    const url = req.url || '/';
    const method = req.method || 'GET';

    // Legacy adapter routes (envelope: {code, message, data, total})
    if (url.startsWith('/api/v1/aiprof/analysis/list') && method === 'GET') {
        return adapters.listRecords(req, res);
    }
    if (url.startsWith('/api/v1/aiprof/analysis/timeline') && method === 'GET') {
        return adapters.proxyTimeline(req, res);
    }
    if (url === '/api/v1/aiprof/analysis/result' && method === 'POST') {
        return adapters.queryResult(req, res);
    }
    if (url === '/api/v1/aiprof/analysis/start' && method === 'POST') {
        return adapters.startAnalysis(req, res);
    }
    if (url === '/api/v1/aiprof/analysis/diff' && method === 'POST') {
        return adapters.diffAnalysis(req, res);
    }
    if (url === '/api/v1/aiprof/clients' && method === 'GET') {
        return adapters.listClients(req, res);
    }

    // Raw pass-through to backend FastAPI services (legacy contract).
    if (url.startsWith('/api/v1/aiprof/report')) {
        return proxy(REPORT_URL + url, req, res);
    }
    if (url.startsWith('/api/v1/aiprof/')) {
        return proxy(QUERY_URL + url, req, res);
    }
    if (url === '/healthz') {
        res.writeHead(200, { 'content-type': 'application/json' });
        return res.end(JSON.stringify({ ok: true, service: 'bff' }));
    }
    return staticServer.serve(STATIC_DIR, req, res);
});

server.listen(PORT, () => {
    console.log(`aiprof-bff listening on :${PORT}`);
    console.log(`  query  -> ${QUERY_URL}`);
    console.log(`  report -> ${REPORT_URL}`);
    console.log(`  static -> ${STATIC_DIR}`);
});
