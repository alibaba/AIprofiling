// SPDX-License-Identifier: Apache-2.0
'use strict';
const { describe, it, before } = require('node:test');
const assert = require('node:assert/strict');
const path = require('path');

const { ingestTrace } = require('../src/traceIngest');
const { getDataProfile } = require('../src/tools/getDataProfile');
const { analyzeStatistics } = require('../src/tools/analyzeStatistics');
const { analyzeKernel } = require('../src/tools/analyzeKernel');
const { analyzeGpuIdle } = require('../src/tools/analyzeGpuIdle');
const { analyzePeriodicity } = require('../src/tools/analyzePeriodicity');
const { analyzeMemory } = require('../src/tools/analyzeMemory');
const { analyzeLaunchOverhead } = require('../src/tools/analyzeLaunchOverhead');

const FIXTURE = path.join(__dirname, 'fixtures', 'small_trace.json');

describe('traceIngest', () => {
    it('parses container format correctly', async () => {
        const store = await ingestTrace(FIXTURE);
        assert.equal(store.meta.eventCount, 7);
        assert.equal(store.meta.catCounts.kernel, 4);
        assert.equal(store.meta.catCounts.cpu_op, 1);
        assert.equal(store.meta.catCounts.cuda_runtime, 1);
        assert.equal(store.meta.hasKernel, true);
        assert.equal(store.meta.hasMemory, true);
        assert.equal(store.meta.pids.length, 1);
        assert.equal(store.meta.devices.length, 1);
        assert.equal(store.meta.devices[0], 0);
    });

    it('handles maxEvents cap', async () => {
        const store = await ingestTrace(FIXTURE, { maxEvents: 3 });
        assert.ok(store.meta.eventCount <= 3, `got ${store.meta.eventCount}, expected <= 3`);
        assert.equal(store.meta.truncated, true);
    });
});

describe('tools', () => {
    let store;

    it('setup: load fixture', async () => {
        store = await ingestTrace(FIXTURE);
        assert.ok(store.meta.eventCount > 0);
    });

    it('getDataProfile returns correct shape', () => {
        const r = getDataProfile(store);
        assert.equal(r.ok, true);
        assert.equal(r.name, 'getDataProfile');
        assert.equal(r.evidence.eventCount, 7);
        assert.ok(r.evidence.timeSpanMs > 0);
        assert.ok(r.evidence.hasKernel);
    });

    it('analyzeStatistics returns categories sorted by sumDur', () => {
        const r = analyzeStatistics(store);
        assert.equal(r.ok, true);
        assert.ok(r.evidence.topBySumDuration.length > 0);
        const first = r.evidence.topBySumDuration[0];
        assert.ok(first.sumDurUs > 0);
        assert.ok(first.cat);
    });

    it('analyzeKernel returns top kernels', () => {
        const r = analyzeKernel(store);
        assert.equal(r.ok, true);
        assert.ok(r.evidence.uniqueKernels >= 2);
        const top = r.evidence.topKernels[0];
        assert.equal(top.name, 'sm80_xmma_gemm_f16');
        assert.ok(top.pctOfKernelTime > 50);
    });

    it('analyzeGpuIdle detects idle gaps', () => {
        const r = analyzeGpuIdle(store);
        assert.equal(r.ok, true);
        assert.ok(r.evidence.busyPct > 0);
        assert.ok(r.evidence.busyPct < 100);
        assert.ok(r.evidence.largestGaps.length > 0);
    });

    it('analyzePeriodicity runs without error', () => {
        const r = analyzePeriodicity(store);
        assert.equal(r.name, 'analyzePeriodicity');
        assert.ok(r.ok !== undefined);
    });

    it('analyzeMemory finds memcpy events', () => {
        const r = analyzeMemory(store);
        assert.equal(r.name, 'analyzeMemory');
        assert.ok(r.ok);
    });

    it('analyzeLaunchOverhead matches correlation pairs', () => {
        const r = analyzeLaunchOverhead(store);
        assert.equal(r.name, 'analyzeLaunchOverhead');
        assert.equal(r.ok, true);
        assert.equal(r.evidence.pairedCount, 1);
    });
});

describe('planner (keyword fallback)', () => {
    const { keywordFallback } = require('../src/planner');

    it('detects periodicity intent', () => {
        const intents = keywordFallback('分析周期性RT突增');
        assert.ok(intents.includes('periodicity'));
    });

    it('detects memory intent', () => {
        const intents = keywordFallback('OOM 显存不足');
        assert.ok(intents.includes('memory'));
    });

    it('defaults to overview+hotspot', () => {
        const intents = keywordFallback('帮我看看');
        assert.ok(intents.includes('overview'));
        assert.ok(intents.includes('hotspot'));
    });
});
