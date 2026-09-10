// SPDX-License-Identifier: Apache-2.0
'use strict';

function getDataProfile(store) {
    const m = store.meta;
    const timeSpanMs = (m.tsMax - m.tsMin) / 1000;
    const catBreakdown = {};
    for (const [cat, count] of Object.entries(m.catCounts)) {
        catBreakdown[cat] = { count, pctOfTotal: +(count / m.eventCount * 100).toFixed(1) };
    }
    const quality = [];
    if (m.truncated) quality.push('truncated — max events cap reached');
    if (!m.hasKernel) quality.push('no GPU kernel events detected');
    if (m.eventCount < 100) quality.push('very few events — trace may be incomplete');

    return {
        name: 'getDataProfile',
        ok: true,
        summary: `${m.eventCount} events, ${timeSpanMs.toFixed(1)}ms span, ${Object.keys(m.catCounts).length} categories`,
        evidence: {
            eventCount: m.eventCount,
            timeSpanMs: +timeSpanMs.toFixed(2),
            categories: Object.keys(m.catCounts).length,
            catBreakdown,
            pids: m.pids,
            devices: m.devices,
            hasKernel: m.hasKernel,
            hasMemory: m.hasMemory,
            hasNccl: m.hasNccl,
            hasCudaRuntime: m.hasCudaRuntime,
            quality,
        },
    };
}

module.exports = { getDataProfile };
