// SPDX-License-Identifier: Apache-2.0
'use strict';
const { isKernelCat } = require('../eventStore');

function analyzeKernel(store, { topN = 15, sortBy = 'sumDur' } = {}) {
    const byName = Object.create(null);
    for (const e of store.events) {
        if (!isKernelCat(e.cat) || e.dur <= 0) continue;
        let rec = byName[e.name];
        if (!rec) { rec = byName[e.name] = { name: e.name, count: 0, sumDur: 0, durations: [] }; }
        rec.count += 1;
        rec.sumDur += e.dur;
        rec.durations.push(e.dur);
    }

    const totalKernelTime = Object.values(byName).reduce((s, r) => s + r.sumDur, 0);

    const results = [];
    for (const rec of Object.values(byName)) {
        rec.durations.sort((a, b) => a - b);
        const n = rec.durations.length;
        const mean = rec.sumDur / n;
        const p50 = rec.durations[Math.floor(n * 0.5)] || 0;
        const p95 = rec.durations[Math.floor(n * 0.95)] || 0;
        const p99 = rec.durations[Math.floor(n * 0.99)] || 0;
        const max = rec.durations[n - 1] || 0;
        results.push({
            name: rec.name,
            count: rec.count,
            sumDurUs: +rec.sumDur.toFixed(0),
            sumDurMs: +(rec.sumDur / 1000).toFixed(2),
            pctOfKernelTime: +(rec.sumDur / totalKernelTime * 100).toFixed(2),
            meanUs: +mean.toFixed(1),
            p50Us: p50,
            p95Us: p95,
            p99Us: p99,
            maxUs: max,
        });
    }

    if (sortBy === 'count') results.sort((a, b) => b.count - a.count);
    else if (sortBy === 'maxDur') results.sort((a, b) => b.maxUs - a.maxUs);
    else results.sort((a, b) => b.sumDurUs - a.sumDurUs);

    const top = results.slice(0, topN);

    return {
        name: 'analyzeKernel',
        ok: true,
        summary: `${results.length} unique kernels, total kernel time ${(totalKernelTime/1000).toFixed(1)}ms. Top: ${top.slice(0,3).map(r => r.name.slice(0,40)).join(', ')}`,
        evidence: {
            uniqueKernels: results.length,
            totalKernelTimeUs: +totalKernelTime.toFixed(0),
            totalKernelTimeMs: +(totalKernelTime / 1000).toFixed(2),
            sortBy,
            topKernels: top,
        },
    };
}

module.exports = { analyzeKernel };
