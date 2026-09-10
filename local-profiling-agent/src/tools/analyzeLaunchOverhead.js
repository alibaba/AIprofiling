// SPDX-License-Identifier: Apache-2.0
// Measure kernel launch overhead by correlating cuda_runtime → kernel via
// the `correlation` field in args (PyTorch/kineto convention).
'use strict';
const { isKernelCat, isCudaRuntimeCat } = require('../eventStore');

function analyzeLaunchOverhead(store, { topN = 10 } = {}) {
    // Build correlation → cuda_runtime event map
    const runtimeByCor = Object.create(null);
    for (const e of store.events) {
        if (!isCudaRuntimeCat(e.cat)) continue;
        const cor = e.argsSummary?.correlation;
        if (cor != null) runtimeByCor[cor] = e;
    }

    // Match kernels to their launch event
    const pairs = [];
    for (const e of store.events) {
        if (!isKernelCat(e.cat) || e.dur <= 0) continue;
        const cor = e.argsSummary?.correlation;
        if (cor == null) continue;
        const launch = runtimeByCor[cor];
        if (!launch) continue;
        // Overhead = time between launch start and kernel start
        const overhead = e.ts - launch.ts;
        if (overhead >= 0) {
            pairs.push({ kernelName: e.name, launchName: launch.name, overheadUs: overhead, kernelDurUs: e.dur, launchDurUs: launch.dur || 0 });
        }
    }

    if (pairs.length === 0) {
        return {
            name: 'analyzeLaunchOverhead',
            ok: false,
            summary: 'no correlation-matched launch→kernel pairs found (trace may lack correlation args)',
            evidence: { pairedCount: 0 },
        };
    }

    pairs.sort((a, b) => a.overheadUs - b.overheadUs);
    const n = pairs.length;
    const overheads = pairs.map((p) => p.overheadUs);
    const sum = overheads.reduce((s, v) => s + v, 0);
    const mean = sum / n;
    const p50 = overheads[Math.floor(n * 0.5)];
    const p95 = overheads[Math.floor(n * 0.95)];
    const p99 = overheads[Math.floor(n * 0.99)];
    const max = overheads[n - 1];

    // Top-N by overhead
    const topByOverhead = pairs.slice(-topN).reverse().map((p) => ({
        kernelName: p.kernelName.slice(0, 80),
        launchName: p.launchName,
        overheadUs: +p.overheadUs.toFixed(1),
        kernelDurUs: p.kernelDurUs,
    }));

    return {
        name: 'analyzeLaunchOverhead',
        ok: true,
        summary: `${n} paired launches. Overhead: mean=${mean.toFixed(1)}µs, p50=${p50}µs, p95=${p95}µs, max=${max}µs`,
        evidence: {
            pairedCount: n,
            overheadStats: {
                meanUs: +mean.toFixed(1),
                p50Us: p50,
                p95Us: p95,
                p99Us: p99,
                maxUs: max,
            },
            totalLaunchOverheadMs: +(sum / 1000).toFixed(2),
            topByOverhead,
        },
    };
}

module.exports = { analyzeLaunchOverhead };
