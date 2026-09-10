// SPDX-License-Identifier: Apache-2.0
// traceIngest.js — Streaming chrome-tracing JSON parser.
// Handles multi-hundred-MB files without loading them entirely into memory.
// Uses stream-json's StreamArray to iterate over elements in "traceEvents"
// and normalizes each into the TraceEvent shape consumed by the tool layer.

'use strict';

const fs = require('fs');
const path = require('path');
const { parser } = require('stream-json');
const { pick } = require('stream-json/filters/Pick');
const { streamArray } = require('stream-json/streamers/StreamArray');
const { TraceStore } = require('./eventStore');

const WHITELIST_ARGS = new Set([
    'stream', 'device', 'correlation', 'bytes', 'grid', 'block',
    'external_id', 'op_count', 'src', 'dst', 'name', 'nccl_type',
]);

function extractDevice(evt) {
    if (evt.args) {
        if (Number.isFinite(evt.args.device)) return evt.args.device;
        if (Number.isFinite(evt.args.stream)) return Math.floor(evt.args.stream / 1000);
    }
    return undefined;
}

function filterArgs(args) {
    if (!args || typeof args !== 'object') return undefined;
    let out;
    for (const k of Object.keys(args)) {
        if (WHITELIST_ARGS.has(k)) {
            if (!out) out = {};
            out[k] = args[k];
        }
    }
    return out;
}

function normalizeEvent(raw) {
    const ph = raw.ph || '';
    // Skip metadata events — they carry process/thread name labels, not timing data.
    if (ph === 'M' || ph === 's' || ph === 'f' || ph === 't') return null;
    const ts = Number(raw.ts);
    if (!Number.isFinite(ts)) return null;
    const dur = Number(raw.dur) || 0;
    const cat = (raw.cat || '').split(',')[0].trim();
    const name = raw.name || '';
    const pid = Number(raw.pid) || 0;
    const tid = Number(raw.tid) || 0;
    const deviceId = extractDevice(raw);
    const argsSummary = filterArgs(raw.args);
    return { ts, dur, ph, cat, name, pid, tid, deviceId, argsSummary };
}

// Detect if first bytes look like a bare array (starts with '[') or a container
// object (starts with '{'). Returns 'array' | 'container' | 'unknown'.
async function detectFormat(filePath) {
    const fd = await fs.promises.open(filePath, 'r');
    try {
        const buf = Buffer.alloc(256);
        const { bytesRead } = await fd.read(buf, 0, 256, 0);
        const head = buf.slice(0, bytesRead).toString('utf-8').trimStart();
        if (head.startsWith('[')) return 'array';
        if (head.startsWith('{')) return 'container';
        return 'unknown';
    } finally {
        await fd.close();
    }
}

// Main entry point. Streams through traceEvents and returns a populated TraceStore.
async function ingestTrace(filePath, { maxEvents = 5_000_000, onProgress } = {}) {
    const stat = await fs.promises.stat(filePath);
    const format = await detectFormat(filePath);
    if (format === 'unknown') throw new Error(`Unsupported trace format in ${path.basename(filePath)}`);

    const store = new TraceStore();
    store.meta.filePath = filePath;
    store.meta.fileSize = stat.size;

    const t0 = Date.now();
    let bytesRead = 0;
    let lastProgressPct = 0;

    await new Promise((resolve, reject) => {
        const fileStream = fs.createReadStream(filePath, { highWaterMark: 256 * 1024 });
        fileStream.on('error', reject);

        // Track bytes for progress
        fileStream.on('data', (chunk) => {
            bytesRead += chunk.length;
            if (onProgress) {
                const pct = Math.floor(bytesRead / stat.size * 100);
                if (pct > lastProgressPct) {
                    lastProgressPct = pct;
                    onProgress({ phase: 'ingest', pct, eventCount: store.meta.eventCount });
                }
            }
        });

        let pipeline;
        if (format === 'container') {
            // Standard {traceEvents: [...], ...} — pick the array
            pipeline = fileStream
                .pipe(parser())
                .pipe(pick({ filter: 'traceEvents' }))
                .pipe(streamArray());
        } else {
            // Bare array [...]
            pipeline = fileStream
                .pipe(parser())
                .pipe(streamArray());
        }

        let truncated = false;
        let settled = false;
        const settle = (fn) => { if (!settled) { settled = true; fn(); } };

        pipeline.on('data', ({ value }) => {
            if (truncated) return;
            if (store.meta.eventCount >= maxEvents) {
                truncated = true;
                store.meta.truncated = true;
                fileStream.destroy();
                settle(() => resolve());
                return;
            }
            const evt = normalizeEvent(value);
            if (evt) store.push(evt);
        });
        pipeline.on('end', () => settle(() => resolve()));
        pipeline.on('close', () => settle(() => resolve()));
        pipeline.on('error', (err) => {
            // stream-json may throw on malformed tail; tolerate if we got events
            settle(() => store.meta.eventCount > 0 ? resolve() : reject(err));
        });
    });

    store.meta.ingestMs = Date.now() - t0;
    store.finalize();
    return store;
}

module.exports = { ingestTrace };
