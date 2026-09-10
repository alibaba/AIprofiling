// SPDX-License-Identifier: Apache-2.0
// Detect periodic patterns in kernel execution via time-bucketed autocorrelation.
'use strict';
const { isKernelCat } = require('../eventStore');

function analyzePeriodicity(store, { buckets = 2048, cat } = {}) {
    const catPredicate = cat ? (c) => c === cat : isKernelCat;
    const { tsMin, tsMax } = store.meta;
    const span = tsMax - tsMin;
    if (span <= 0) {
        return { name: 'analyzePeriodicity', ok: false, summary: 'no time span', evidence: {} };
    }
    const numBuckets = Math.min(buckets, 8192);
    const bw = span / numBuckets; // µs per bucket

    // Sum durations into buckets
    const signal = new Float64Array(numBuckets);
    let totalEvents = 0;
    for (const e of store.events) {
        if (!catPredicate(e.cat) || e.dur <= 0) continue;
        totalEvents += 1;
        const idx = Math.min(numBuckets - 1, Math.max(0, Math.floor((e.ts - tsMin) / bw)));
        signal[idx] += e.dur;
    }
    if (totalEvents < 10) {
        return { name: 'analyzePeriodicity', ok: false, summary: 'too few events for periodicity analysis', evidence: {} };
    }

    // Compute mean/std
    let sum = 0, sumSq = 0;
    for (let i = 0; i < numBuckets; i++) { sum += signal[i]; sumSq += signal[i] * signal[i]; }
    const mean = sum / numBuckets;
    const variance = sumSq / numBuckets - mean * mean;
    const std = Math.sqrt(Math.max(0, variance));

    if (std < 1e-9) {
        return { name: 'analyzePeriodicity', ok: true, summary: 'signal is flat — no periodicity detected', evidence: { flat: true, totalEvents } };
    }

    // Normalized autocorrelation for lags 1..numBuckets/2
    const maxLag = Math.floor(numBuckets / 2);
    let peakLag = 0, peakCorr = 0;
    const topPeaks = [];
    for (let lag = 1; lag <= maxLag; lag++) {
        let corr = 0;
        for (let i = 0; i < numBuckets - lag; i++) {
            corr += (signal[i] - mean) * (signal[i + lag] - mean);
        }
        corr /= (numBuckets - lag) * variance;
        if (corr > peakCorr) {
            peakCorr = corr;
            peakLag = lag;
        }
        // Collect local peaks (corr > 0.3 and higher than neighbors)
        if (corr > 0.3 && topPeaks.length < 5) {
            if (topPeaks.length === 0 || lag - topPeaks[topPeaks.length - 1].lag > 3) {
                topPeaks.push({ lag, corr: +corr.toFixed(4), periodMs: +(lag * bw / 1000).toFixed(2) });
            }
        }
    }

    const peakPeriodMs = +(peakLag * bw / 1000).toFixed(2);
    const detected = peakCorr > 0.25;

    // Find the top-N spike buckets (bursts)
    const spikes = [];
    const threshold = mean + 2 * std;
    for (let i = 0; i < numBuckets; i++) {
        if (signal[i] > threshold) {
            spikes.push({ bucket: i, tsMs: +((tsMin + i * bw) / 1000).toFixed(2), value: +signal[i].toFixed(0) });
        }
    }
    spikes.sort((a, b) => b.value - a.value);

    return {
        name: 'analyzePeriodicity',
        ok: true,
        summary: detected
            ? `Periodic pattern detected: period ~${peakPeriodMs}ms (strength=${peakCorr.toFixed(2)}), ${spikes.length} spike buckets`
            : `No strong periodicity (best corr=${peakCorr.toFixed(2)} at lag=${peakLag})`,
        evidence: {
            detected,
            peakLag,
            peakCorr: +peakCorr.toFixed(4),
            peakPeriodMs,
            bucketWidthMs: +(bw / 1000).toFixed(3),
            numBuckets,
            totalEvents,
            signalMean: +mean.toFixed(1),
            signalStd: +std.toFixed(1),
            spikeCount: spikes.length,
            topSpikes: spikes.slice(0, 10),
            topPeaks,
        },
    };
}

module.exports = { analyzePeriodicity };
