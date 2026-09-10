// SPDX-License-Identifier: Apache-2.0
// TraceStore —— in-memory normalized event table + light indices.
// Consumers iterate via iter({cats, pidTid, tsRange}) and bucketize().
// One row per event; fields intentionally narrow to keep RSS bounded on
// multi-million-event traces.

'use strict';

class TraceStore {
    constructor() {
        this.events = [];  // TraceEvent[] — flat array, insertion order preserves time order per stream
        this.meta = {
            filePath: '',
            fileSize: 0,
            eventCount: 0,
            tsMin: Infinity,
            tsMax: -Infinity,
            catCounts: Object.create(null),
            phCounts: Object.create(null),
            pids: [],
            tidsByPid: Object.create(null),
            devices: [],
            hasKernel: false,
            hasMemory: false,
            hasNccl: false,
            hasCudaRuntime: false,
            ingestMs: 0,
            truncated: false,
        };
        this._pidSet = new Set();
        this._tidByPid = Object.create(null);
        this._devSet = new Set();
    }

    push(evt) {
        this.events.push(evt);
        const m = this.meta;
        m.eventCount += 1;
        if (Number.isFinite(evt.ts)) {
            if (evt.ts < m.tsMin) m.tsMin = evt.ts;
            const end = evt.ts + (evt.dur || 0);
            if (end > m.tsMax) m.tsMax = end;
        }
        const cat = evt.cat || '';
        m.catCounts[cat] = (m.catCounts[cat] || 0) + 1;
        const ph = evt.ph || '';
        m.phCounts[ph] = (m.phCounts[ph] || 0) + 1;
        if (Number.isFinite(evt.pid)) this._pidSet.add(evt.pid);
        if (Number.isFinite(evt.pid) && Number.isFinite(evt.tid)) {
            const set = this._tidByPid[evt.pid] || (this._tidByPid[evt.pid] = new Set());
            set.add(evt.tid);
        }
        if (Number.isFinite(evt.deviceId)) this._devSet.add(evt.deviceId);
    }

    finalize() {
        const m = this.meta;
        m.pids = Array.from(this._pidSet).sort((a, b) => a - b);
        m.tidsByPid = {};
        for (const pid of m.pids) {
            m.tidsByPid[pid] = Array.from(this._tidByPid[pid] || []).sort((a, b) => a - b);
        }
        m.devices = Array.from(this._devSet).sort((a, b) => a - b);
        if (!Number.isFinite(m.tsMin)) m.tsMin = 0;
        if (!Number.isFinite(m.tsMax)) m.tsMax = 0;

        const cats = m.catCounts;
        m.hasKernel = Object.keys(cats).some(isKernelCat);
        m.hasMemory = Object.keys(cats).some(isMemoryCat);
        m.hasNccl = Object.keys(cats).some((c) => /nccl|comm/i.test(c));
        m.hasCudaRuntime = Object.keys(cats).some((c) => /cuda[_ ]?runtime|hip[_ ]?runtime/i.test(c));

        delete this._pidSet;
        delete this._tidByPid;
        delete this._devSet;
    }

    *iter({ cats, catPredicate, pidTid, tsRange, phs } = {}) {
        const catSet = cats ? new Set(cats) : null;
        const phSet = phs ? new Set(phs) : null;
        const [tsLo, tsHi] = tsRange || [-Infinity, Infinity];
        const [pFilter, tFilter] = pidTid || [null, null];
        for (const e of this.events) {
            if (catSet && !catSet.has(e.cat)) continue;
            if (catPredicate && !catPredicate(e.cat)) continue;
            if (phSet && !phSet.has(e.ph)) continue;
            if (e.ts < tsLo || e.ts > tsHi) continue;
            if (pFilter != null && e.pid !== pFilter) continue;
            if (tFilter != null && e.tid !== tFilter) continue;
            yield e;
        }
    }

    // Bucketize accumulates a numeric reduction per bucket across the full
    // [tsMin, tsMax] span. `reducer(bucketArr, evt, bucketStart, bucketEnd)`
    // is called per matching event; returns the Float64Array.
    bucketize(numBuckets, filter, reducer) {
        const arr = new Float64Array(numBuckets);
        const { tsMin, tsMax } = this.meta;
        const span = tsMax - tsMin;
        if (span <= 0 || numBuckets <= 0) return arr;
        const bw = span / numBuckets;
        for (const e of this.iter(filter || {})) {
            const idx = Math.min(numBuckets - 1, Math.max(0, Math.floor((e.ts - tsMin) / bw)));
            reducer(arr, e, idx, bw);
        }
        return arr;
    }
}

// Category classifiers — chrome-tracing produces a zoo of names across kineto
// versions ("kernel", "cuda_kernel", "gpu_kernel", "ai_kernel", "cuda_gpu_kernel", ...).
// Keep the predicates permissive; a false-positive kernel is much better than
// silently skipping a whole trace flavor.
function isKernelCat(c) {
    if (!c) return false;
    const s = String(c).toLowerCase();
    return s === 'kernel' || s === 'cuda' || s === 'gpu'
        || /kernel/.test(s) || /gpu[_ ]?op/.test(s);
}
function isMemoryCat(c) {
    if (!c) return false;
    const s = String(c).toLowerCase();
    return /memory|memcpy|memset|alloc|free/.test(s);
}
function isCudaRuntimeCat(c) {
    if (!c) return false;
    const s = String(c).toLowerCase();
    return /cuda[_ ]?runtime|hip[_ ]?runtime|cuda[_ ]?driver/.test(s);
}
function isCpuOpCat(c) {
    if (!c) return false;
    const s = String(c).toLowerCase();
    return s === 'cpu_op' || s === 'user_annotation' || s === 'python_function' || /cpu[_ ]?op/.test(s);
}

module.exports = {
    TraceStore,
    isKernelCat,
    isMemoryCat,
    isCudaRuntimeCat,
    isCpuOpCat,
};
