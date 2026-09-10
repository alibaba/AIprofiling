// SPDX-License-Identifier: Apache-2.0
//
// Trace ownership rules. Kept separate from dashboardServer.js so the access
// decision is unit-testable without booting an HTTP listener.

const SYSTEM_PLACEHOLDER = 'system';

const LEGACY_OWNER = (process.env.AIPROF_LEGACY_OWNER || '').trim() || null;

function ownerOf(taskArgs) {
    const raw = taskArgs?.created_by;
    if (typeof raw === 'string' && raw && raw !== SYSTEM_PLACEHOLDER) return raw;
    return LEGACY_OWNER;
}

function canAccess({ ownerId, userId, isAdmin }) {
    if (isAdmin) return true;
    if (!ownerId) return false;
    return ownerId === userId;
}

module.exports = { ownerOf, canAccess, LEGACY_OWNER, SYSTEM_PLACEHOLDER };
