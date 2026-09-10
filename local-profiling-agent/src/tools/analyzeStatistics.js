// SPDX-License-Identifier: Apache-2.0
'use strict';

function analyzeStatistics(store, { topN = 20 } = {}) {
    const byCat = Object.create(null);
    for (const e of store.events) {
        if (e.dur <= 0) continue;
        let rec = byCat[e.cat];
        if (!rec) { rec = byCat[e.cat] = { cat: e.cat, count: 0, sumDur: 0, durations: [] }; }
        rec.count += 1;
        rec.sumDur += e.dur;
        rec.durations.push(e.dur);
    }

    const results = [];
    for (const rec of Object.values(byCat)) {
        rec.durations.sort((a, b) => a - b);
        const n = rec.durations.length;
        const p50 = rec.durations[Math.floor(n * 0.5)] || 0;
        const p95 = rec.durations[Math.floor(n * 0.95)] || 0;
        const p99 = rec.durations[Math.floor(n * 0.99)] || 0;
        const max = rec.durations[n - 1] || 0;
        const mean = n > 0 ? rec.sumDur / n : 0;
        results.push({
            cat: rec.cat,
            count: rec.count,
            sumDurUs: +rec.sumDur.toFixed(0),
            sumDurMs: +(rec.sumDur / 1000).toFixed(2),
            meanUs: +mean.toFixed(1),
            p50Us: p50,
            p95Us: p95,
            p99Us: p99,
            maxUs: max,
        });
    }
    results.sort((a, b) => b.sumDurUs - a.sumDurUs);
    const top = results.slice(0, topN);

    return {
        name: 'analyzeStatistics',
        ok: true,
        summary: `${results.length} categories; top by total duration: ${top.slice(0,3).map(r => r.cat).join(', ')}`,
        evidence: {
            totalCategories: results.length,
            topBySumDuration: top,
        },
    };
}

module.exports = { analyzeStatistics };
