// SPDX-License-Identifier: Apache-2.0
const test = require('node:test');
const assert = require('node:assert');
const { CollectionQueue } = require('./queue.js');

function mk(opts = {}) {
    const dispatched = [];
    const q = new CollectionQueue({
        maxPerClient: opts.maxPerClient ?? 5,
        itemTtlMs: opts.itemTtlMs ?? 1800000,
        now: opts.now ?? (() => 1000),
        onDispatch: opts.onDispatch ?? (async (clientId, item) => {
            dispatched.push([clientId, item.analysisId]);
        }),
        onPersist: opts.onPersist ?? (() => {}),
    });
    return { q, dispatched };
}

const item = (id, owner = 'alice') => ({ analysisId: id, owner, profilingConfig: {}, argsForCache: {} });

test('enqueue accepts and numbers positions from 1', () => {
    const { q } = mk();
    assert.deepStrictEqual(q.enqueue('c1', item('a')), { accepted: true, position: 1, reason: null });
    assert.deepStrictEqual(q.enqueue('c1', item('b')), { accepted: true, position: 2, reason: null });
});

test('queues are independent per client', () => {
    const { q } = mk();
    q.enqueue('c1', item('a'));
    assert.strictEqual(q.enqueue('c2', item('b')).position, 1);
    assert.strictEqual(q.depth('c1'), 1);
    assert.strictEqual(q.depth('c2'), 1);
});

test('enqueue refuses past maxPerClient', () => {
    const { q } = mk({ maxPerClient: 2 });
    q.enqueue('c1', item('a'));
    q.enqueue('c1', item('b'));
    const r = q.enqueue('c1', item('c'));
    assert.strictEqual(r.accepted, false);
    assert.match(r.reason, /繁忙/);
    assert.strictEqual(q.depth('c1'), 2);
});

test('pump dispatches the head and marks it in flight', async () => {
    const { q, dispatched } = mk();
    q.enqueue('c1', item('a'));
    q.enqueue('c1', item('b'));
    await q.pump('c1');
    assert.deepStrictEqual(dispatched, [['c1', 'a']]);
    assert.strictEqual(q.inFlight('c1'), 'a');
    assert.strictEqual(q.depth('c1'), 1);
});

test('pump is a no-op while a task is in flight', async () => {
    const { q, dispatched } = mk();
    q.enqueue('c1', item('a'));
    q.enqueue('c1', item('b'));
    await q.pump('c1');
    await q.pump('c1');
    assert.deepStrictEqual(dispatched, [['c1', 'a']]);
});

test('markDone frees the slot so the next pump dispatches', async () => {
    const { q, dispatched } = mk();
    q.enqueue('c1', item('a'));
    q.enqueue('c1', item('b'));
    await q.pump('c1');
    q.markDone('c1', 'a');
    assert.strictEqual(q.inFlight('c1'), null);
    await q.pump('c1');
    assert.deepStrictEqual(dispatched, [['c1', 'a'], ['c1', 'b']]);
});

test('FIFO order is preserved across many items', async () => {
    const { q, dispatched } = mk();
    for (const id of ['a', 'b', 'c']) q.enqueue('c1', item(id));
    for (let i = 0; i < 3; i++) {
        await q.pump('c1');
        q.markDone('c1', dispatched[dispatched.length - 1][1]);
    }
    assert.deepStrictEqual(dispatched.map((d) => d[1]), ['a', 'b', 'c']);
});

test('the dispatched item keeps its enqueue-time owner', async () => {
    let seen = null;
    const { q } = mk({ onDispatch: async (_c, it) => { seen = it.owner; } });
    q.enqueue('c1', item('a', 'bob'));
    await q.pump('c1');
    assert.strictEqual(seen, 'bob');
});

test('positionOf reports the live queue position', () => {
    const { q } = mk();
    q.enqueue('c1', item('a'));
    q.enqueue('c1', item('b'));
    assert.strictEqual(q.positionOf('c1', 'b'), 2);
    assert.strictEqual(q.positionOf('c1', 'zzz'), null);
});

test('a failing dispatch frees the slot instead of wedging the queue', async () => {
    const { q } = mk({ onDispatch: async () => { throw new Error('ws gone'); } });
    q.enqueue('c1', item('a'));
    await q.pump('c1');
    assert.strictEqual(q.inFlight('c1'), null);
});

test('sweepExpired drops items past the TTL', () => {
    let t = 1000;
    const { q } = mk({ itemTtlMs: 500, now: () => t });
    q.enqueue('c1', item('a'));
    t = 2000;
    const dropped = q.sweepExpired();
    assert.strictEqual(dropped.length, 1);
    assert.strictEqual(dropped[0].item.analysisId, 'a');
    assert.strictEqual(q.depth('c1'), 0);
});

test('sweepExpired keeps items inside the TTL', () => {
    let t = 1000;
    const { q } = mk({ itemTtlMs: 5000, now: () => t });
    q.enqueue('c1', item('a'));
    t = 1200;
    assert.strictEqual(q.sweepExpired().length, 0);
    assert.strictEqual(q.depth('c1'), 1);
});

test('toJSON/fromJSON round-trips pending items for restart recovery', () => {
    const { q } = mk();
    q.enqueue('c1', item('a'));
    q.enqueue('c1', item('b'));
    const json = JSON.parse(JSON.stringify(q.toJSON()));
    const restored = CollectionQueue.fromJSON(json, {
        maxPerClient: 5, itemTtlMs: 1800000, now: () => 1000,
        onDispatch: async () => {}, onPersist: () => {},
    });
    assert.strictEqual(restored.depth('c1'), 2);
    assert.strictEqual(restored.positionOf('c1', 'b'), 2);
});

test('restore clears in-flight state — the agent is gone after a restart', async () => {
    const { q, dispatched } = mk();
    q.enqueue('c1', item('a'));
    await q.pump('c1');
    const json = JSON.parse(JSON.stringify(q.toJSON()));
    const restored = CollectionQueue.fromJSON(json, {
        maxPerClient: 5, itemTtlMs: 1800000, now: () => 1000,
        onDispatch: async () => dispatched.push(['restored', 'x']), onPersist: () => {},
    });
    assert.strictEqual(restored.inFlight('c1'), null);
});

test('enqueue persists so a restart does not lose the queue', () => {
    let persisted = 0;
    const { q } = mk({ onPersist: () => { persisted += 1; } });
    q.enqueue('c1', item('a'));
    assert.ok(persisted > 0);
});
