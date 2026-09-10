// SPDX-License-Identifier: Apache-2.0
const test = require('node:test');
const assert = require('node:assert');
const path = require('node:path');

// auth.js reads env at require time, so each mode needs a fresh module registry.
function loadAuth(env) {
    const saved = {};
    for (const k of Object.keys(env)) { saved[k] = process.env[k]; process.env[k] = env[k]; }
    delete require.cache[require.resolve('./auth.js')];
    const mod = require('./auth.js');
    for (const k of Object.keys(saved)) {
        if (saved[k] === undefined) delete process.env[k]; else process.env[k] = saved[k];
    }
    return mod;
}

test('none mode resolves everyone to the anonymous user', async () => {
    const auth = loadAuth({ AUTH_MODE: 'none' });
    const user = await auth.resolveUser({ headers: {} });
    assert.strictEqual(user.userId, 'anonymous');
});

test('none mode requires no configuration', () => {
    const auth = loadAuth({ AUTH_MODE: 'none' });
    assert.doesNotThrow(() => auth.assertConfigured());
});

test('dev mode resolves to the dev user', async () => {
    const auth = loadAuth({ AUTH_MODE: 'dev' });
    const user = await auth.resolveUser({ headers: {} });
    assert.strictEqual(user.userId, 'dev');
});

test('external mode without AUTH_PROVIDER_MODULE fails assertConfigured', () => {
    const auth = loadAuth({ AUTH_MODE: 'external', AUTH_PROVIDER_MODULE: '' });
    assert.throws(() => auth.assertConfigured(), /AUTH_PROVIDER_MODULE/);
});

test('external mode delegates resolveUser to the loaded provider', async () => {
    const fixture = path.join(__dirname, 'test-fixtures', 'provider-ok.js');
    const auth = loadAuth({ AUTH_MODE: 'external', AUTH_PROVIDER_MODULE: fixture });
    const user = await auth.resolveUser({ headers: {} });
    assert.strictEqual(user.userId, 'alice');
    assert.strictEqual(auth.loginUrl(), 'https://example.test/login');
});

test('external provider returning null means not logged in', async () => {
    const fixture = path.join(__dirname, 'test-fixtures', 'provider-anon.js');
    const auth = loadAuth({ AUTH_MODE: 'external', AUTH_PROVIDER_MODULE: fixture });
    assert.strictEqual(await auth.resolveUser({ headers: {} }), null);
});

test('requireAuth sets req.user and calls next on success', async () => {
    const auth = loadAuth({ AUTH_MODE: 'dev' });
    const req = { headers: {} };
    let nexted = false;
    await auth.requireAuth(req, { status: () => ({ json: () => {} }) }, () => { nexted = true; });
    assert.ok(nexted);
    assert.strictEqual(req.user.userId, 'dev');
});

test('requireAuth answers 401 with a login url when unresolved', async () => {
    const fixture = path.join(__dirname, 'test-fixtures', 'provider-anon.js');
    const auth = loadAuth({ AUTH_MODE: 'external', AUTH_PROVIDER_MODULE: fixture });
    let code = 0; let body = null;
    const res = { status: (c) => { code = c; return { json: (b) => { body = b; } }; } };
    let nexted = false;
    await auth.requireAuth({ headers: {} }, res, () => { nexted = true; });
    assert.strictEqual(nexted, false);
    assert.strictEqual(code, 401);
    assert.ok(body.loginUrl);
});

test('isAdmin reads AIPROF_ADMIN_USERS as a comma list', () => {
    const auth = loadAuth({ AUTH_MODE: 'dev', AIPROF_ADMIN_USERS: 'root, alice' });
    assert.ok(auth.isAdmin('root'));
    assert.ok(auth.isAdmin('alice'));
    assert.strictEqual(auth.isAdmin('bob'), false);
});

test('a provider that throws surfaces as not-logged-in, not a crash', async () => {
    const fixture = path.join(__dirname, 'test-fixtures', 'provider-throws.js');
    const auth = loadAuth({ AUTH_MODE: 'external', AUTH_PROVIDER_MODULE: fixture });
    assert.strictEqual(await auth.resolveUser({ headers: {} }), null);
});
