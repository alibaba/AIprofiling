// SPDX-License-Identifier: Apache-2.0
//
// Adapters that translate between the UI response envelope
// ({code, message, data, total}) and the underlying AIProf FastAPI
// services (collector/query/report).

'use strict';

const http = require('http');
const { URL } = require('url');
const { spawn } = require('child_process');
const os = require('os');
const path = require('path');
const fs = require('fs');
const crypto = require('crypto');

const COLLECTOR_URL = process.env.AIPROF_COLLECTOR_URL || 'http://127.0.0.1:7101';
const QUERY_URL    = process.env.AIPROF_QUERY_URL     || 'http://127.0.0.1:7102';
const REPORT_URL   = process.env.AIPROF_REPORT_URL    || 'http://127.0.0.1:7103';

const REPO_ROOT = path.resolve(__dirname, '..', '..', '..', '..');
const AGENT_SCRIPT = path.join(REPO_ROOT, 'agent-py', 'aiprof_agent.py');
const PYTHON = process.env.AIPROF_PYTHON || 'python3.11';
const CAPTURE_LOG_DIR = process.env.AIPROF_CAPTURE_LOG_DIR || '/tmp';

function httpJson(url, method = 'GET', body = null, timeoutMs = 15_000) {
    return new Promise((resolve, reject) => {
        const u = new URL(url);
        const opts = {
            method,
            hostname: u.hostname,
            port: u.port || 80,
            path: u.pathname + (u.search || ''),
            headers: { 'accept': 'application/json' },
            timeout: timeoutMs,
        };
        if (body !== null) {
            const buf = Buffer.from(typeof body === 'string' ? body : JSON.stringify(body));
            opts.headers['content-type'] = 'application/json';
            opts.headers['content-length'] = buf.length;
            var payload = buf;
        }
        const req = http.request(opts, (r) => {
            const chunks = [];
            r.on('data', (c) => chunks.push(c));
            r.on('end', () => {
                const text = Buffer.concat(chunks).toString('utf-8');
                if (r.statusCode >= 200 && r.statusCode < 300) {
                    try { resolve(JSON.parse(text)); }
                    catch { resolve(text); }
                } else {
                    reject(new Error(`HTTP ${r.statusCode}: ${text.slice(0, 200)}`));
                }
            });
        });
        req.on('error', reject);
        req.on('timeout', () => { req.destroy(new Error('timeout')); });
        if (body !== null) req.write(payload);
        req.end();
    });
}

function httpText(url, timeoutMs = 15_000) {
    return new Promise((resolve, reject) => {
        const u = new URL(url);
        const req = http.request({
            method: 'GET',
            hostname: u.hostname,
            port: u.port || 80,
            path: u.pathname + (u.search || ''),
            timeout: timeoutMs,
        }, (r) => {
            const chunks = [];
            r.on('data', (c) => chunks.push(c));
            r.on('end', () => {
                if (r.statusCode >= 200 && r.statusCode < 300) {
                    resolve(Buffer.concat(chunks).toString('utf-8'));
                } else {
                    reject(new Error(`HTTP ${r.statusCode}`));
                }
            });
        });
        req.on('error', reject);
        req.on('timeout', () => { req.destroy(new Error('timeout')); });
        req.end();
    });
}

function readJsonBody(req) {
    return new Promise((resolve, reject) => {
        const chunks = [];
        req.on('data', (c) => chunks.push(c));
        req.on('end', () => {
            const text = Buffer.concat(chunks).toString('utf-8');
            if (!text) return resolve({});
            try { resolve(JSON.parse(text)); }
            catch (e) { reject(e); }
        });
        req.on('error', reject);
    });
}

function respond(res, code, body) {
    const buf = Buffer.from(JSON.stringify(body));
    res.writeHead(code, {
        'content-type': 'application/json; charset=utf-8',
        'content-length': buf.length,
    });
    res.end(buf);
}

function shortId() {
    return crypto.randomBytes(6).toString('hex');
}

// GET /api/v1/aiprof/analysis/list?current&pageSize
async function listRecords(req, res) {
    const u = new URL(req.url, 'http://x');
    const pageSize = Math.min(500, parseInt(u.searchParams.get('pageSize') || '20', 10));
    const current = Math.max(1, parseInt(u.searchParams.get('current') || '1', 10));
    try {
        const upstream = await httpJson(`${QUERY_URL}/api/v1/aiprof/sessions?limit=${pageSize}`);
        const items = (upstream.items || []).map((s) => {
            const created = s.created_at || s.captured_at || '';
            return {
                analysisId: s.session_id,
                analysisTime: (typeof created === 'string')
                    ? created.replace('T', ' ').slice(0, 19)
                    : String(created),
                instance: s.host || 'local',
                arguments: {
                    instance: s.host || 'local',
                    channel: s.channel || 'agent',
                    pids: s.pid ? String(s.pid) : undefined,
                    comms: s.workload,
                    timeout: s.duration_ms || (s.duration_s ? s.duration_s * 1000 : undefined),
                    analysis_params: s.kind ? [s.kind] : ['adapt'],
                },
                parms: [
                    { key: 'channel', value: s.channel || 'agent' },
                    ...(s.workload ? [{ key: 'workload', value: s.workload }] : []),
                    ...(s.kind ? [{ key: 'kind', value: s.kind }] : []),
                ],
                status: 'Success',
                failedLog: '',
            };
        });
        const start = (current - 1) * pageSize;
        const paged = items.slice(start, start + pageSize);
        respond(res, 200, {
            code: 'Success',
            message: '',
            data: paged,
            total: items.length,
        });
    } catch (err) {
        respond(res, 200, { code: 'Error', message: String(err), data: [], total: 0 });
    }
}

// POST /api/v1/aiprof/analysis/result  { analysisId }
async function queryResult(req, res) {
    let body;
    try { body = await readJsonBody(req); }
    catch (e) { return respond(res, 400, { code: 'Error', message: 'bad json' }); }
    const sid = body.analysisId;
    if (!sid) return respond(res, 200, { code: 'Error', message: 'analysisId required' });

    try {
        const meta = await httpJson(`${QUERY_URL}/api/v1/aiprof/sessions/${encodeURIComponent(sid)}`);
        const artifacts = meta?.artifacts || {};
        const folded = artifacts['folded.txt']
            ? await httpText(`${QUERY_URL}/api/v1/aiprof/sessions/${encodeURIComponent(sid)}/folded`)
            : '';
        // Kernels endpoint always exists but returns [] on CPU-only sessions.
        // Skip the call entirely when the manifest says there's no trace.
        const hasTimeline = !!(artifacts['torch-trace.json'] || artifacts['timeline.json']);
        let kernels = [];
        if (hasTimeline) {
            try {
                const k = await httpJson(
                    `${QUERY_URL}/api/v1/aiprof/sessions/${encodeURIComponent(sid)}/kernels`,
                );
                kernels = Array.isArray(k?.items) ? k.items : [];
            } catch { /* keep empty */ }
        }
        let report = '';
        try {
            const r = await httpJson(`${REPORT_URL}/api/v1/aiprof/report/${encodeURIComponent(sid)}`);
            report = r?.content || '';
        } catch { /* report is optional */ }

        respond(res, 200, {
            code: 'Success',
            message: '',
            data: {
                session_id: sid,
                meta,
                folded,
                report,
                kernels,
                // BFF-relative proxy path — the browser will call our own
                // server, avoiding the query-service CORS surface entirely.
                timeline_url: hasTimeline
                    ? `/api/v1/aiprof/analysis/timeline?analysisId=${encodeURIComponent(sid)}`
                    : null,
            },
        });
    } catch (err) {
        respond(res, 200, { code: 'Error', message: String(err) });
    }
}

// GET /api/v1/aiprof/analysis/timeline?analysisId=...
// Proxies the raw chrome trace through the BFF so the browser can fetch it
// same-origin and Perfetto/download links work without a CORS pre-flight.
async function proxyTimeline(req, res) {
    const u = new URL(req.url, 'http://x');
    const sid = u.searchParams.get('analysisId');
    if (!sid) {
        respond(res, 400, { code: 'Error', message: 'analysisId required' });
        return;
    }
    const upstream = new URL(
        `${QUERY_URL}/api/v1/aiprof/sessions/${encodeURIComponent(sid)}/timeline`,
    );
    const upReq = http.request({
        method: 'GET',
        hostname: upstream.hostname,
        port: upstream.port || 80,
        path: upstream.pathname + (upstream.search || ''),
        timeout: 60_000,
    }, (r) => {
        const contentType = r.headers['content-type'] || 'application/json';
        res.writeHead(r.statusCode || 500, {
            'content-type': contentType,
            'content-disposition': `attachment; filename="${sid}-trace.json"`,
        });
        r.pipe(res);
    });
    upReq.on('error', (err) => {
        respond(res, 502, { code: 'Error', message: String(err) });
    });
    upReq.on('timeout', () => {
        upReq.destroy(new Error('upstream timeout'));
    });
    upReq.end();
}

// POST /api/v1/aiprof/analysis/start  { instance, pids, comms, timeout, channel, ... }
async function startAnalysis(req, res) {
    let body;
    try { body = await readJsonBody(req); }
    catch (e) { return respond(res, 400, { code: 'Error', message: 'bad json' }); }

    const jobId = shortId();
    const workload = body.comms || body.instance || `capture-${jobId}`;
    const durationMs = Number(body.timeout) || 3000;
    const durationSec = Math.max(1, Math.round(durationMs / 1000));
    const recordsDir = path.join(os.tmpdir(), `aiprof-capture-${jobId}`);
    const logFile = path.join(CAPTURE_LOG_DIR, `aiprof-capture-${jobId}.log`);

    const args = [
        AGENT_SCRIPT, '--once',
        '--duration', String(durationSec),
        '--workload', workload,
        '--records-dir', recordsDir,
        '--endpoint', `${COLLECTOR_URL}/api/v1/collector/upload`,
    ];
    if (body.pids) {
        const firstPid = String(body.pids).split(',')[0].trim();
        if (firstPid) args.push('--pid', firstPid);
    }
    const needsMock = !body.pids && process.getuid && process.getuid() !== 0;
    if (needsMock) args.push('--mock');

    if (!fs.existsSync(AGENT_SCRIPT)) {
        return respond(res, 200, {
            code: 'Error',
            message: `agent script not found at ${AGENT_SCRIPT}`,
        });
    }

    let logStream;
    try {
        logStream = fs.createWriteStream(logFile, { flags: 'a' });
    } catch (e) {
        return respond(res, 200, { code: 'Error', message: `cannot open ${logFile}: ${e}` });
    }
    logStream.write(`[bff] $ ${PYTHON} ${args.join(' ')}\n`);

    const child = spawn(PYTHON, args, {
        detached: true,
        stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.pipe(logStream);
    child.stderr.pipe(logStream);
    child.on('error', (err) => {
        logStream.write(`[bff] spawn error: ${err}\n`);
    });
    child.unref();

    respond(res, 200, {
        code: 'Success',
        message: 'analysis started',
        data: {
            job_id: jobId,
            log_file: logFile,
            workload,
            duration_sec: durationSec,
            mock: needsMock,
        },
    });
}

// POST /api/v1/aiprof/analysis/diff  { a, b }
async function diffAnalysis(req, res) {
    let body;
    try { body = await readJsonBody(req); }
    catch { return respond(res, 400, { code: 'Error', message: 'bad json' }); }
    const a = body.a || body.analysisIdA;
    const b = body.b || body.analysisIdB;
    if (!a || !b) {
        return respond(res, 200, {
            code: 'NotImplemented',
            message: 'diff requires two analysisIds',
            data: null,
        });
    }
    try {
        const text = await httpText(
            `${QUERY_URL}/api/v1/aiprof/diff?a=${encodeURIComponent(a)}&b=${encodeURIComponent(b)}`,
        );
        respond(res, 200, { code: 'Success', message: '', data: { diff: text } });
    } catch (err) {
        respond(res, 200, { code: 'Error', message: String(err) });
    }
}

// GET /api/v1/aiprof/clients
async function listClients(_req, res) {
    respond(res, 200, {
        code: 'Success',
        message: '',
        data: [{ id: 'local', host: os.hostname(), channel: 'agent' }],
    });
}

module.exports = {
    listRecords,
    queryResult,
    startAnalysis,
    diffAnalysis,
    listClients,
    proxyTimeline,
};
