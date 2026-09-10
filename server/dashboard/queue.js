// SPDX-License-Identifier: Apache-2.0
//
// Per-client FIFO collection queue.
//
// Before this module, start_ai_analysis answered 400 whenever the target agent
// was busy, so a second user simply lost their request. Tasks now queue per
// clientId — one agent runs one collection at a time, but distinct agents stay
// independent, and a full queue is the only rejection.
//
// All side effects (dispatch, persistence, clock) are injected so the queueing
// logic is testable without a WebSocket or a filesystem.

class CollectionQueue {
    constructor({ maxPerClient, itemTtlMs, now, onDispatch, onPersist }) {
        this.maxPerClient = maxPerClient;
        this.itemTtlMs = itemTtlMs;
        this.now = now;
        this.onDispatch = onDispatch;
        this.onPersist = onPersist;
        // clientId → Array<item>, head is index 0
        this.queues = new Map();
        // clientId → analysisId currently dispatched (null when the agent is free)
        this.running = new Map();
    }

    depth(clientId) { return (this.queues.get(clientId) || []).length; }

    inFlight(clientId) { return this.running.get(clientId) || null; }

    enqueue(clientId, item) {
        const q = this.queues.get(clientId) || [];
        if (q.length >= this.maxPerClient) {
            return {
                accepted: false,
                position: null,
                reason: `当前采集繁忙，前面还有 ${q.length} 个任务排队，请稍后再试`,
            };
        }
        q.push({ ...item, enqueuedAt: this.now() });
        this.queues.set(clientId, q);
        this.onPersist();
        return { accepted: true, position: q.length, reason: null };
    }

    positionOf(clientId, analysisId) {
        const q = this.queues.get(clientId) || [];
        const idx = q.findIndex((it) => it.analysisId === analysisId);
        return idx < 0 ? null : idx + 1;
    }

    // Dispatch the head when the agent is free. A dispatch failure releases the
    // slot rather than pinning the queue behind a task that never started.
    async pump(clientId) {
        if (this.running.get(clientId)) return;
        const q = this.queues.get(clientId) || [];
        const item = q.shift();
        if (!item) return;
        this.queues.set(clientId, q);
        this.running.set(clientId, item.analysisId);
        this.onPersist();
        try {
            await this.onDispatch(clientId, item);
        } catch (err) {
            console.error(`[queue] dispatch of ${item.analysisId} to ${clientId} failed: ${err.message}`);
            this.running.delete(clientId);
            this.onPersist();
        }
    }

    markDispatched(clientId, analysisId) {
        this.running.set(clientId, analysisId);
        this.onPersist();
    }

    markDone(clientId, analysisId) {
        if (this.running.get(clientId) === analysisId) {
            this.running.delete(clientId);
            this.onPersist();
        }
    }

    // Drop items that waited past the TTL. Without this a wedged agent would
    // hold a queue of stale tasks that each still look pending to their owner.
    sweepExpired() {
        const dropped = [];
        const cutoff = this.now() - this.itemTtlMs;
        for (const [clientId, q] of this.queues.entries()) {
            const kept = [];
            for (const it of q) {
                if (it.enqueuedAt <= cutoff) dropped.push({ clientId, item: it });
                else kept.push(it);
            }
            this.queues.set(clientId, kept);
        }
        if (dropped.length) this.onPersist();
        return dropped;
    }

    toJSON() {
        return { queues: Object.fromEntries(this.queues) };
    }

    // In-flight state is deliberately not restored: the agent that was running
    // a task does not survive a server restart, and reviving the flag would
    // block the queue until a watchdog cleared it.
    static fromJSON(json, opts) {
        const q = new CollectionQueue(opts);
        for (const [clientId, items] of Object.entries(json?.queues || {})) {
            if (Array.isArray(items)) q.queues.set(clientId, items);
        }
        return q;
    }
}

module.exports = { CollectionQueue };
