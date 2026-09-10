// SPDX-License-Identifier: Apache-2.0
// Detect GPU idle gaps — periods where no kernel runs on a given device.
'use strict';
const { isKernelCat } = require('../eventStore');

function analyzeGpuIdle(store, { topGaps = 10 } = {}) {
    // Collect kernel intervals sorted by start time
    const intervals = [];
    for (const e of store.events) {
        if (!isKernelCat(e.cat) || e.dur <= 0) continue;
        intervals.push({ ts: e.ts, end: e.ts + e.dur, name: e.name });
    }
    if (intervals.length === 0) {
        return { name: 'analyzeGpuIdle', ok: false, summary: 'no kernel events to analyze', evidence: {} };
    }
    intervals.sort((a, b) => a.ts - b.ts);

    // Merge overlapping intervals → busy spans
    const busy = [{ ts: intervals[0].ts, end: intervals[0].end }];
    for (let i = 1; i < intervals.length; i++) {
        const last = busy[busy.length - 1];
        if (intervals[i].ts <= last.end) {
            last.end = Math.max(last.end, intervals[i].end);
        } else {
            busy.push({ ts: intervals[i].ts, end: intervals[i].end });
        }
    }

    const { tsMin, tsMax } = store.meta;
    const totalSpan = tsMax - tsMin;
    const busyTime = busy.reduce((s, b) => s + (b.end - b.ts), 0);
    const idleTime = totalSpan - busyTime;
    const busyPct = totalSpan > 0 ? +(busyTime / totalSpan * 100).toFixed(2) : 0;

    // Collect gaps between busy spans
    const gaps = [];
    for (let i = 1; i < busy.length; i++) {
        const gapDur = busy[i].ts - busy[i - 1].end;
        if (gapDur > 0) {
            gaps.push({ ts: busy[i - 1].end, durUs: gapDur });
        }
    }
    // Also consider leading/trailing gaps
    if (busy[0].ts > tsMin) {
        gaps.push({ ts: tsMin, durUs: busy[0].ts - tsMin, note: 'leading' });
    }
    if (busy[busy.length - 1].end < tsMax) {
        gaps.push({ ts: busy[busy.length - 1].end, durUs: tsMax - busy[busy.length - 1].end, note: 'trailing' });
    }
    gaps.sort((a, b) => b.durUs - a.durUs);
    const largestGaps = gaps.slice(0, topGaps).map((g) => ({
        tsUs: +g.ts.toFixed(0),
        durUs: +g.durUs.toFixed(0),
        durMs: +(g.durUs / 1000).toFixed(2),
        note: g.note || '',
    }));

    return {
        name: 'analyzeGpuIdle',
        ok: true,
        summary: `GPU busy ${busyPct}%, idle ${(100-busyPct).toFixed(2)}%. Largest gap ${(gaps[0]?.durUs||0)/1000} ms`,
        evidence: {
            busyPct,
            idlePct: +(100 - busyPct).toFixed(2),
            totalSpanUs: +totalSpan.toFixed(0),
            busyTimeUs: +busyTime.toFixed(0),
            idleTimeUs: +idleTime.toFixed(0),
            busySpanCount: busy.length,
            gapCount: gaps.length,
            largestGaps,
        },
    };
}

module.exports = { analyzeGpuIdle };
