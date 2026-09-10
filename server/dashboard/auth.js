// SPDX-License-Identifier: Apache-2.0
//
// Pluggable identity layer. Ships two built-in providers and can load a
// third-party one at runtime, so deployment-specific SSO integrations stay
// out of this repository.
//
//   AUTH_MODE=none      (default) every caller is the shared `anonymous` user.
//                       Preserves the pre-auth behaviour for local/OSS use.
//   AUTH_MODE=dev       every caller is `dev`.
//   AUTH_MODE=external  require(AUTH_PROVIDER_MODULE) and delegate to it.
//
// An external provider must export `async resolveUser(req)` returning
// `{userId, username, nickname}` or null. It may also export
// `assertConfigured()`, `loginBase` and `logoutBase`.

const crypto = require('crypto');

const AUTH_MODE = (process.env.AUTH_MODE || 'none').toLowerCase();
const PROVIDER_MODULE = (process.env.AUTH_PROVIDER_MODULE || '').trim();
const ADMIN_USERS = new Set(
    (process.env.AIPROF_ADMIN_USERS || '')
        .split(',').map((s) => s.trim()).filter(Boolean),
);

// Optional service-to-service shared secret. When set, a companion service
// can present this token via the X-AIProf-Internal-Token header to reach the
// few endpoints that opt in via `requireAuthOrInternal` — nothing else.
// Empty (the default) disables the bypass entirely.
const INTERNAL_TOKEN = (process.env.AIPROF_INTERNAL_TOKEN || '').trim();
const INTERNAL_TOKEN_BUF = INTERNAL_TOKEN ? Buffer.from(INTERNAL_TOKEN) : null;
const INTERNAL_USER = {
    userId: '__internal', username: '__internal', nickname: 'Internal Service',
};

function isInternalTokenValid(header) {
    if (!INTERNAL_TOKEN_BUF) return false;
    if (typeof header !== 'string' || !header) return false;
    const got = Buffer.from(header);
    if (got.length !== INTERNAL_TOKEN_BUF.length) return false;
    return crypto.timingSafeEqual(got, INTERNAL_TOKEN_BUF);
}

const ANONYMOUS = { userId: 'anonymous', username: 'anonymous', nickname: '匿名用户' };
const DEV_USER = { userId: 'dev', username: 'dev', nickname: '开发者' };

let externalProvider = null;
let externalLoadError = null;
if (AUTH_MODE === 'external' && PROVIDER_MODULE) {
    try {
        externalProvider = require(PROVIDER_MODULE);
    } catch (err) {
        externalLoadError = err;
    }
}

function assertConfigured() {
    if (AUTH_MODE === 'none' || AUTH_MODE === 'dev') return;
    if (AUTH_MODE !== 'external') {
        throw new Error(`unknown AUTH_MODE="${AUTH_MODE}" (expected none|dev|external)`);
    }
    if (!PROVIDER_MODULE) {
        throw new Error('AUTH_PROVIDER_MODULE is required when AUTH_MODE=external');
    }
    if (externalLoadError) {
        throw new Error(`failed to load AUTH_PROVIDER_MODULE=${PROVIDER_MODULE}: ${externalLoadError.message}`);
    }
    if (typeof externalProvider?.resolveUser !== 'function') {
        throw new Error(`AUTH_PROVIDER_MODULE=${PROVIDER_MODULE} must export async resolveUser(req)`);
    }
    if (typeof externalProvider.assertConfigured === 'function') {
        externalProvider.assertConfigured();
    }
}

// A provider fault must read as "not logged in", never as a 500: an unreachable
// SSO upstream should show the login page, not an error page.
async function resolveUser(req) {
    if (AUTH_MODE === 'none') return { ...ANONYMOUS };
    if (AUTH_MODE === 'dev') return { ...DEV_USER };
    if (typeof externalProvider?.resolveUser !== 'function') return null;
    try {
        const user = await externalProvider.resolveUser(req);
        if (!user || typeof user.userId !== 'string' || !user.userId) return null;
        return {
            userId: user.userId,
            username: user.username || user.userId,
            nickname: user.nickname || '',
        };
    } catch (err) {
        console.error('[auth] provider resolveUser failed:', err.message);
        return null;
    }
}

function loginUrl() { return externalProvider?.loginBase || null; }
function logoutUrl() { return externalProvider?.logoutBase || null; }

function isAdmin(userId) { return ADMIN_USERS.has(userId); }

async function requireAuth(req, res, next) {
    const user = await resolveUser(req);
    if (!user) {
        return res.status(401).json({
            code: 'error',
            message: 'authentication required',
            loginUrl: loginUrl(),
        });
    }
    req.user = user;
    return next();
}

// Opt-in variant of `requireAuth` for the narrow set of endpoints a companion
// service may reach with AIPROF_INTERNAL_TOKEN. Mount it explicitly — the
// default `requireAuth` never honours the token, so a leaked secret cannot
// reach anything an endpoint did not deliberately expose.
async function requireAuthOrInternal(req, res, next) {
    if (isInternalTokenValid(req.headers['x-aiprof-internal-token'])) {
        req.user = { ...INTERNAL_USER };
        req._skipOwnership = true;
        return next();
    }
    return requireAuth(req, res, next);
}

module.exports = {
    AUTH_MODE, resolveUser, requireAuth, requireAuthOrInternal, assertConfigured,
    loginUrl, logoutUrl, isAdmin,
};
