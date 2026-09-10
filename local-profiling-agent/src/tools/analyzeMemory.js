// SPDX-License-Identifier: Apache-2.0
// Analyze memory-related events: alloc/free counts, peak, large single allocs.
'use strict';
const { isMemoryCat } = require('../eventStore');

function analyzeMemory(store, { topN = 10 } = {}) {
    const allocs = [];
    const frees = [];
    let totalAllocBytes = 0;
    let maxSingleAlloc = 0;
    let maxSingleAllocName = '';
    let memcpyCount = 0;
    let memcpyBytes = 0;
    let memsetCount = 0;

    for (const e of store.events) {
        if (!isMemoryCat(e.cat)) continue;
        const nameL = e.name.toLowerCase();
        const bytes = e.argsSummary?.bytes || 0;

        if (/alloc|malloc/i.test(nameL)) {
            allocs.push({ name: e.name, ts: e.ts, dur: e.dur, bytes });
            totalAllocBytes += bytes;
            if (bytes > maxSingleAlloc) {
                maxSingleAlloc = bytes;
                maxSingleAllocName = e.name;
            }
        } else if (/free|dealloc/i.test(nameL)) {
            frees.push({ name: e.name, ts: e.ts });
        } else if (/memcpy/i.test(nameL)) {
            memcpyCount += 1;
            memcpyBytes += bytes;
        } else if (/memset/i.test(nameL)) {
            memsetCount += 1;
        }
    }

    if (allocs.length === 0 && memcpyCount === 0 && memsetCount === 0) {
        // Fallback: count raw memory-cat events
        let count = 0;
        for (const e of store.events) {
            if (isMemoryCat(e.cat)) count += 1;
        }
        if (count === 0) {
            return { name: 'analyzeMemory', ok: false, summary: 'no memory events found', evidence: {} };
        }
        return {
            name: 'analyzeMemory',
            ok: true,
            summary: `${count} memory-related events (memcpy/memset), no explicit alloc/free`,
            evidence: { memoryEventCount: count, allocCount: 0, freeCount: 0, memcpyCount: count, memsetCount: 0 },
        };
    }

    // Top allocs by size
    allocs.sort((a, b) => b.bytes - a.bytes);
    const topAllocs = allocs.slice(0, topN).map((a) => ({
        name: a.name,
        bytes: a.bytes,
        mb: +(a.bytes / 1048576).toFixed(2),
        tsMs: +(a.ts / 1000).toFixed(2),
        durUs: a.dur,
    }));

    return {
        name: 'analyzeMemory',
        ok: true,
        summary: `${allocs.length} allocs, ${frees.length} frees, ${memcpyCount} memcpy (${(memcpyBytes/1048576).toFixed(1)} MB). Max single alloc: ${(maxSingleAlloc/1048576).toFixed(1)} MB`,
        evidence: {
            allocCount: allocs.length,
            freeCount: frees.length,
            totalAllocMB: +(totalAllocBytes / 1048576).toFixed(2),
            maxSingleAllocMB: +(maxSingleAlloc / 1048576).toFixed(2),
            maxSingleAllocName,
            memcpyCount,
            memcpyMB: +(memcpyBytes / 1048576).toFixed(2),
            memsetCount,
            topAllocs,
        },
    };
}

module.exports = { analyzeMemory };
