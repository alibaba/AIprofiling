// SPDX-License-Identifier: Apache-2.0
const test = require('node:test');
const assert = require('node:assert');

function loadOwnership(env = {}) {
    const saved = {};
    for (const k of Object.keys(env)) { saved[k] = process.env[k]; process.env[k] = env[k]; }
    delete require.cache[require.resolve('./ownership.js')];
    const mod = require('./ownership.js');
    for (const k of Object.keys(saved)) {
        if (saved[k] === undefined) delete process.env[k]; else process.env[k] = saved[k];
    }
    return mod;
}

test('ownerOf reads created_by', () => {
    const o = loadOwnership();
    assert.strictEqual(o.ownerOf({ created_by: 'alice' }), 'alice');
});

test('ownerOf ignores the legacy system placeholder', () => {
    const o = loadOwnership();
    assert.strictEqual(o.ownerOf({ created_by: 'system' }), null);
});

test('ownerOf falls back to AIPROF_LEGACY_OWNER when unset', () => {
    const o = loadOwnership({ AIPROF_LEGACY_OWNER: 'root' });
    assert.strictEqual(o.ownerOf({}), 'root');
    assert.strictEqual(o.ownerOf({ created_by: 'system' }), 'root');
    assert.strictEqual(o.ownerOf(null), 'root');
});

test('ownerOf returns null for unowned data with no legacy owner', () => {
    const o = loadOwnership();
    assert.strictEqual(o.ownerOf({}), null);
    assert.strictEqual(o.ownerOf(null), null);
});

test('a real owner beats the legacy fallback', () => {
    const o = loadOwnership({ AIPROF_LEGACY_OWNER: 'root' });
    assert.strictEqual(o.ownerOf({ created_by: 'alice' }), 'alice');
});

test('canAccess lets the owner through', () => {
    const o = loadOwnership();
    assert.ok(o.canAccess({ ownerId: 'alice', userId: 'alice', isAdmin: false }));
});

test('canAccess blocks a different user', () => {
    const o = loadOwnership();
    assert.strictEqual(o.canAccess({ ownerId: 'alice', userId: 'bob', isAdmin: false }), false);
});

test('canAccess lets an admin through regardless of owner', () => {
    const o = loadOwnership();
    assert.ok(o.canAccess({ ownerId: 'alice', userId: 'bob', isAdmin: true }));
});

test('unowned data is admin-only', () => {
    const o = loadOwnership();
    assert.strictEqual(o.canAccess({ ownerId: null, userId: 'alice', isAdmin: false }), false);
    assert.ok(o.canAccess({ ownerId: null, userId: 'root', isAdmin: true }));
});
