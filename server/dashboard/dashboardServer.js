// AIProf Dashboard Server — WS control plane + REST upload/query.
//
// Notable behaviours baked into this server:
//   1. taskId is validated against a strict UUID whitelist before touching FS
//   2. tar-slip preflight (`tar -tf` scan) + `--no-same-owner --no-absolute-names`
//   3. start_ai_analysis debounces on client status: rejects when !Idle,
//      then flips to Dispatching with a 30s watchdog so a lost TASK_RESULT
//      cannot pin the client forever
//   4. Heartbeat timeout does a soft `ws.close(1001)` first, hard-terminate
//      only if the peer refuses to close within 5s
//   5. TASK_RESULT bookkeeping (tasks.json + list_record surfacing) with a
//      single _persistChain promise chain and 500ms debounce so persist calls
//      are serialized and coalesced
//
// Paths are env-driven so this file is deploy-agnostic:
//   RESULT_DIR       (default: <repo>/ui/webapp/public/resource/ai_observable/result)
//   PERSISTENCE_DIR  (default: <this dir>/.profiling-data)
//   UPLOAD_TMP       (default: <this dir>/.temp-uploads)
//   PORT             (default: 7000)

const express = require('express');
const cors = require('cors');
const path = require('path');
const { spawn } = require('child_process');
const crypto = require('crypto');
const fs = require('fs').promises;
const fsSync = require('fs');
const multer = require('multer');
const zlib = require('zlib');
const { pipeline } = require('stream/promises');
const http = require('http');
const WebSocket = require('ws');

const auth = require('./auth.js');
const ownership = require('./ownership.js');
const { CollectionQueue } = require('./queue.js');

// BASE_PATH lets the whole app be reverse-proxied under a path prefix
// (e.g. nginx `location /aiprof-open/` → this server). Every route below is
// registered on a Router that gets mounted at the prefix, so route strings
// stay prefix-free. Empty (the default) mounts at "/" — historical behaviour.
const BASE_PATH = (process.env.BASE_PATH || '').replace(/\/+$/, '');

const rootApp = express();
// A Router is the mount unit; `app` keeps its name so the ~2000 lines of
// route registrations below are untouched by the prefix.
const app = express.Router();

rootApp.use(cors());
rootApp.use(express.json());
// Static-serve Perfetto launcher / other small tool pages: `public/perfetto.html` etc.
app.use('/aiprof', express.static(path.join(__dirname, 'public')));

// Static-serve capture results (pickle / trace / summary etc.) for frontend MemoryViz / Perfetto fetch.
// URLs produced by analysis.py convert_to_resource_path look like /resource/app_observable/ai_observable/result/<aid>/<file>.
app.use('/resource/app_observable/ai_observable/result',
    express.static(process.env.RESULT_DIR
        || path.join(__dirname, '..', '..', 'ui', 'webapp', 'public', 'resource', 'ai_observable', 'result')));

// SPA webapp (Vite build output). Mounted at root; assets served from /assets/...
// UI_DIR lets ops override the location; container images symlink it to /app/webapp by default.
const UI_DIR = process.env.UI_DIR
    || path.join(__dirname, 'public', 'webapp', 'dist');
if (fsSync.existsSync(path.join(UI_DIR, 'index.html'))) {
    // Asset filenames carry Vite content hashes (index-<hash>.js) so they're safe to long-cache;
    // once a new hash is built, index.html points at the new name and browsers fetch it fresh.
    app.use('/assets', express.static(path.join(UI_DIR, 'assets'), {
        immutable: true,
        maxAge: '1y',
    }));
    // Express 5 uses path-to-regexp v8 — bare `*` no longer works, must use named `/*splat`.
    app.get(['/', '/aiprof', '/ai_observable', '/ai_observable/*splat', '/aiprof/*splat'], (_req, res, next) => {
        // Don't intercept existing static assets (/aiprof/perfetto.html etc.), only catch SPA routes.
        // index.html must NEVER be cached, otherwise after a deploy the old index.html keeps pointing at
        // the old bundle name and the SPA loads stale code (the classic "just deployed and can't see the
        // new button" symptom).
        res.set('Cache-Control', 'no-store, no-cache, must-revalidate');
        res.set('Pragma', 'no-cache');
        res.set('Expires', '0');
        const indexPath = path.join(UI_DIR, 'index.html');
        res.sendFile(indexPath, err => { if (err) next(); });
    });
}

// ===== Paths =====
const PERSISTENCE_DIR = process.env.PERSISTENCE_DIR
    || path.join(__dirname, '.profiling-data');
const UPLOAD_TMP = process.env.UPLOAD_TMP
    || path.join(__dirname, '.temp-uploads');
const RESULT_DIR = process.env.RESULT_DIR
    || path.join(__dirname, '..', '..', 'ui', 'webapp', 'public', 'resource', 'ai_observable', 'result');
const CLIENTS_FILE = path.join(PERSISTENCE_DIR, 'clients.json');
const TASKS_FILE = path.join(PERSISTENCE_DIR, 'tasks.json');
const LLM_CONFIG_FILE = path.join(PERSISTENCE_DIR, 'llm-config.json');
const QUEUES_FILE = path.join(PERSISTENCE_DIR, 'queues.json');
const PORT = parseInt(process.env.PORT, 10) || 7000;
const MAX_QUEUE_PER_CLIENT = parseInt(process.env.MAX_QUEUE_PER_CLIENT, 10) || 5;
const QUEUE_ITEM_TTL_MS = parseInt(process.env.QUEUE_ITEM_TTL_MS, 10) || 30 * 60 * 1000;

for (const dir of [PERSISTENCE_DIR, UPLOAD_TMP, RESULT_DIR]) {
    fsSync.mkdirSync(dir, { recursive: true });
}

// ===== Persistence layer (JSON file, no external DB) =====
function loadFromDisk(filePath, defaultValue = {}) {
    try {
        if (fsSync.existsSync(filePath)) {
            const content = fsSync.readFileSync(filePath, 'utf-8');
            return JSON.parse(content);
        }
    } catch (err) {
        console.error(`Failed to load ${filePath}:`, err);
    }
    return defaultValue;
}

function loadClientsFromDisk() {
    const data = loadFromDisk(CLIENTS_FILE, {});
    // On boot every client is Offline until it re-registers via WS.
    Object.keys(data).forEach((key) => {
        data[key].status = 'Offline';
        data[key].wsConnected = false;
    });
    return data;
}

async function saveToDisk(filePath, data) {
    try {
        await fs.writeFile(filePath, JSON.stringify(data, null, 2), 'utf-8');
    } catch (err) {
        console.error(`Failed to save ${filePath}:`, err);
    }
}

// Fix 5: serialize + debounce persist calls behind a single promise chain,
// so N in-flight WS messages do not spawn N concurrent JSON writes and clobber
// each other under load.
let _persistChain = Promise.resolve();
let _persistTimer = null;
let _persistPending = { clients: false, tasks: false, queues: false };
function schedulePersist(kind) {
    _persistPending[kind] = true;
    if (_persistTimer) return;
    _persistTimer = setTimeout(() => {
        _persistTimer = null;
        const flush = { ..._persistPending };
        _persistPending = { clients: false, tasks: false, queues: false };
        _persistChain = _persistChain.then(async () => {
            if (flush.clients) {
                await saveToDisk(CLIENTS_FILE, Object.fromEntries(profilingClients));
            }
            if (flush.tasks) {
                await saveToDisk(TASKS_FILE, Object.fromEntries(taskResults));
            }
            if (flush.queues) {
                await saveToDisk(QUEUES_FILE, collectionQueue.toJSON());
            }
        });
    }, 500);
}

// ===== In-memory stores =====
const profilingClients = new Map(Object.entries(loadClientsFromDisk()));
const taskResults = new Map(Object.entries(loadFromDisk(TASKS_FILE, {})));
const taskArgumentsCache = new Map();

// Persist task arguments to disk so `instance` (and other params) survive
// server restarts. Without this, list_record would fall back to the hardcoded
// "offline-task-node" string for any task dispatched by a previous process.
function taskArgsPath(analysisId) {
    return path.join(RESULT_DIR, analysisId, 'task_args.json');
}
function persistTaskArgs(analysisId, args) {
    if (!analysisId || !isValidTaskId(analysisId)) return;
    try {
        const dir = path.join(RESULT_DIR, analysisId);
        fsSync.mkdirSync(dir, { recursive: true });
        fsSync.writeFileSync(taskArgsPath(analysisId), JSON.stringify(args, null, 2), 'utf-8');
    } catch (err) {
        console.warn(`[task_args] persist failed for ${analysisId}: ${err.message}`);
    }
}
function loadTaskArgsFromDisk(analysisId) {
    if (!analysisId) return null;
    try {
        const raw = fsSync.readFileSync(taskArgsPath(analysisId), 'utf-8');
        return JSON.parse(raw);
    } catch {
        return null;
    }
}
// analysisId -> { clientId, expectStatus, watchdog }
const dispatchWatchdogs = new Map();

function nowTs() {
    return Date.now();
}

// Fix 1: strict RFC-4122 hex-only UUID whitelist. Anything outside is 400.
const UUID_RE = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;
function isValidTaskId(id) {
    return typeof id === 'string' && UUID_RE.test(id);
}

// Fix 3: 30s watchdog. If we never see a TASK_RESULT (or a heartbeat that
// reflects the state change) within 30s, unstick the client so the next
// start_ai_analysis is not blocked by a phantom Dispatching flag.
function armDispatchWatchdog(analysisId, clientId) {
    if (dispatchWatchdogs.has(analysisId)) {
        clearTimeout(dispatchWatchdogs.get(analysisId).timer);
    }
    const timer = setTimeout(() => {
        const rec = profilingClients.get(clientId);
        if (rec && rec.status === 'Dispatching') {
            console.warn(`Dispatch watchdog: client ${clientId} still Dispatching after 30s, resetting to Idle`);
            rec.status = 'Idle';
            profilingClients.set(clientId, rec);
            schedulePersist('clients');
        }
        dispatchWatchdogs.delete(analysisId);
    }, 30_000);
    dispatchWatchdogs.set(analysisId, { clientId, timer });
}

function clearDispatchWatchdog(analysisId) {
    const entry = dispatchWatchdogs.get(analysisId);
    if (entry) {
        clearTimeout(entry.timer);
        dispatchWatchdogs.delete(analysisId);
    }
}

// Default heartbeat cadence assumed for poll-mode agents; used by the
// offline sweep to decide when a poll client is silent for too long.
const DEFAULT_HB_INTERVAL_MS = Number(process.env.AIPROF_HB_INTERVAL_MS) || 30_000;

// ==== poll-transport state (mirrors wsConnections for HTTP long-poll agents) ====
// clientId -> { resolve, timer }: the in-flight GET /api/agent/poll we are
// holding open for that client. At most one per client.
const pollWaiters = new Map();
// clientId -> { analysisId, profilingConfig }: a task already dispatched but
// not yet picked up, so a poll arriving after the dispatch still gets it.
const inflightItems = new Map();

function resolvePollWaiter(clientId, payload) {
    const waiter = pollWaiters.get(clientId);
    if (!waiter) return false;
    pollWaiters.delete(clientId);
    clearTimeout(waiter.timer);
    waiter.resolve(payload);
    return true;
}

function dropPollWaiter(clientId) {
    resolvePollWaiter(clientId, null);
}

// Dispatch one queued task to the agent. The ws branch throws on a dead
// socket, which releases the queue slot (see CollectionQueue.pump) so a gone
// client does not wedge its own queue. The poll branch never throws: "no
// in-flight poll right now" is the normal state between two polls, and the
// task must stay in `running` until a poll picks it up.
async function dispatchQueuedTask(clientId, item) {
    const clientRecord = profilingClients.get(clientId);
    if (!clientRecord) throw new Error(`client ${clientId} not registered`);

    const ws = wsConnections.get(clientId);
    if (clientRecord.transport !== 'poll' && (!ws || ws.readyState !== WebSocket.OPEN)) {
        throw new Error(`client ${clientId} not connected`);
    }

    taskArgumentsCache.set(item.analysisId, item.argsForCache);
    persistTaskArgs(item.analysisId, item.argsForCache);

    clientRecord.status = 'Dispatching';
    profilingClients.set(clientId, clientRecord);
    schedulePersist('clients');
    armDispatchWatchdog(item.analysisId, clientId);

    const payload = {
        analysisId: item.analysisId,
        profilingConfig: item.profilingConfig,
    };

    if (clientRecord.transport === 'poll') {
        const delivered = resolvePollWaiter(clientId, payload);
        if (!delivered) {
            // No poll in flight right now — stash the payload so the next
            // poll picks it up. Delivery through an active waiter needs no
            // stash: the payload is already in transit to the client, and
            // keeping it here would just re-serve the same task to the poll
            // the client fires the instant it dispatches.
            inflightItems.set(clientId, payload);
        }
        console.log(`向 poll client ${clientId} 下发任务 ${item.analysisId}（owner=${item.owner}, delivered=${delivered}）`);
        return;
    }

    ws.send(JSON.stringify({ type: 'NEW_TASK', ...payload }));
    console.log(`通过 WebSocket 向 client ${clientId} 下发任务 ${item.analysisId}（owner=${item.owner}）`);
}

const collectionQueue = CollectionQueue.fromJSON(loadFromDisk(QUEUES_FILE, {}), {
    maxPerClient: MAX_QUEUE_PER_CLIENT,
    itemTtlMs: QUEUE_ITEM_TTL_MS,
    now: () => Date.now(),
    onDispatch: dispatchQueuedTask,
    onPersist: () => { schedulePersist('queues'); },
});

const SWEEP_INTERVAL_MS = Number(process.env.AIPROF_SWEEP_INTERVAL_MS) || 60_000;

// Expired queue items must leave a trace: their owner is polling list_record
// and would otherwise see "queued" forever.
setInterval(() => {
    for (const { clientId, item } of collectionQueue.sweepExpired()) {
        taskResults.set(item.analysisId, {
            analysisId: item.analysisId,
            clientId,
            status: 'Failed',
            message: `排队超时（超过 ${Math.round(QUEUE_ITEM_TTL_MS / 60000)} 分钟未能下发）`,
            ts: nowTs(),
        });
        schedulePersist('tasks');
    }
    sweepOfflinePollClients(nowTs());
}, SWEEP_INTERVAL_MS).unref();

// A poll client has no persistent socket, so nothing fires the ws 'close'
// handler that flips a client Offline. Both heartbeats and polls refresh
// lastHeartbeat, so silence past three heartbeat intervals means gone.
function sweepOfflinePollClients(now) {
    const flipped = [];
    for (const [clientId, rec] of profilingClients) {
        if (rec.transport !== 'poll' || rec.status === 'Offline') continue;
        if (now - (rec.lastHeartbeat || 0) <= 3 * DEFAULT_HB_INTERVAL_MS) continue;
        rec.status = 'Offline';
        profilingClients.set(clientId, rec);
        dropPollWaiter(clientId);
        flipped.push(clientId);
    }
    if (flipped.length > 0) {
        console.log(`poll clients marked Offline (no heartbeat): ${flipped.join(', ')}`);
        schedulePersist('clients');
    }
    return flipped;
}

// ===== WebSocket control plane =====
const server = http.createServer(rootApp);
const wss = new WebSocket.Server({ server, path: `${BASE_PATH}/ws` });

// clientId -> WebSocket
const wsConnections = new Map();

wss.on('connection', (ws) => {
    let clientId = null;
    let heartbeatTimer = null;
    let terminateTimer = null;

    function scheduleHeartbeatTimeout() {
        if (heartbeatTimer) clearTimeout(heartbeatTimer);
        // Fix 4: soft close first, hard-terminate only if peer refuses to leave.
        heartbeatTimer = setTimeout(() => {
            if (!(clientId && wsConnections.get(clientId) === ws)) return;
            console.log(`Client ${clientId} 心跳超时，发送 close(1001)`);
            try { ws.close(1001, 'heartbeat timeout'); } catch {}
            terminateTimer = setTimeout(() => {
                if (wsConnections.get(clientId) === ws) {
                    console.log(`Client ${clientId} 5s 后仍未 close，terminate`);
                    ws.terminate();
                }
            }, 5000);
        }, 60_000);
    }

    scheduleHeartbeatTimeout();

    ws.on('message', async (message) => {
        scheduleHeartbeatTimeout();

        let data;
        try {
            data = JSON.parse(message.toString());
        } catch (err) {
            console.error('无法解析 WebSocket 消息:', err);
            return;
        }

        const type = data.type;

        if (type === 'REGISTER') {
            const {
                clientId: incomingClientId,
                namespace,
                podName,
                nodeName,
                labels = {},
                capabilities = [],
            } = data;

            if (!incomingClientId) {
                ws.send(JSON.stringify({
                    type: 'ERROR',
                    message: 'clientId is required for REGISTER',
                }));
                return;
            }

            clientId = incomingClientId;
            wsConnections.set(clientId, ws);

            const record = profilingClients.get(clientId) || {};
            const now = nowTs();

            const updated = {
                clientId,
                namespace: namespace || record.namespace || '',
                podName: podName || record.podName || '',
                nodeName: nodeName || record.nodeName || '',
                labels: labels || record.labels || {},
                capabilities: Array.isArray(capabilities) && capabilities.length > 0
                    ? capabilities
                    : (record.capabilities || []),
                status: 'Idle',
                lastHeartbeat: now,
                runningTaskIds: [],
                wsConnected: true,
                transport: 'ws',
            };

            profilingClients.set(clientId, updated);
            schedulePersist('clients');

            ws.send(JSON.stringify({ type: 'REGISTERED', clientId }));
            console.log(`Client ${clientId} 已通过 WebSocket 注册`);
        } else if (type === 'HEARTBEAT') {
            if (!clientId) return;
            const { status, runningTaskIds } = data;
            const record = profilingClients.get(clientId) || { clientId };

            record.lastHeartbeat = nowTs();
            // If the client heartbeats a real status (Idle/Profiling), trust it
            // over any Dispatching flag the server is holding.
            if (status) {
                record.status = status;
            } else if (!record.status) {
                record.status = 'Idle';
            }
            if (Array.isArray(runningTaskIds)) {
                record.runningTaskIds = runningTaskIds;
            } else if (!record.runningTaskIds) {
                record.runningTaskIds = [];
            }
            record.wsConnected = true;

            profilingClients.set(clientId, record);
            schedulePersist('clients');

            if (record.status === 'Idle' && collectionQueue.depth(clientId) > 0) {
                collectionQueue.pump(clientId).catch((e) => console.error('[queue] pump failed:', e.message));
            }

            ws.send(JSON.stringify({ type: 'HEARTBEAT_ACK' }));
        } else if (type === 'TASK_RESULT') {
            // Fix 5: agent reports final task status. Persist to tasks.json so
            // list_record can surface success/failure even when analysis is
            // never re-run.
            const { analysisId, status, message: taskMessage } = data;
            if (!analysisId) return;

            const record = taskResults.get(analysisId) || { analysisId };
            record.clientId = clientId || record.clientId;
            record.status = status || 'Unknown';
            record.message = taskMessage || '';
            record.ts = nowTs();
            taskResults.set(analysisId, record);
            schedulePersist('tasks');

            // clear the dispatch flag on the client so the next task can go through
            const client = profilingClients.get(clientId);
            if (client && client.status === 'Dispatching') {
                client.status = 'Idle';
                profilingClients.set(clientId, client);
                schedulePersist('clients');
            }
            clearDispatchWatchdog(analysisId);

            // Free the queue slot and hand the agent its next task.
            collectionQueue.markDone(clientId, analysisId);
            collectionQueue.pump(clientId).catch((e) => console.error('[queue] pump failed:', e.message));

            console.log(`TASK_RESULT ${analysisId} status=${status} client=${clientId}`);
        } else if (type === 'LIST_GPU_PROCS_RESULT') {
            // client response to LIST_GPU_PROCS
            const { reqId, procs, error: gpuErr } = data;
            const pending = gpuProcsPending.get(reqId);
            if (pending) {
                gpuProcsPending.delete(reqId);
                clearTimeout(pending.timer);
                if (gpuErr) pending.reject(new Error(gpuErr));
                else pending.resolve(procs || []);
            }
        }
    });

    ws.on('close', () => {
        if (heartbeatTimer) clearTimeout(heartbeatTimer);
        if (terminateTimer) clearTimeout(terminateTimer);
        if (clientId && wsConnections.get(clientId) === ws) {
            wsConnections.delete(clientId);
            const client = profilingClients.get(clientId);
            if (client) {
                client.wsConnected = false;
                client.status = 'Offline';
                profilingClients.set(clientId, client);
                schedulePersist('clients');
            }
            console.log(`Client ${clientId} WebSocket 断开`);
        }
    });

    ws.on('error', (err) => {
        console.error('WebSocket error:', err);
    });
});

// ==== Client management APIs ====
app.post('/api/clients/register', async (req, res) => {
    const {
        clientId,
        namespace,
        podName,
        nodeName,
        labels = {},
        capabilities = [],
    } = req.body || {};

    if (!clientId) {
        return res.status(400).json({ code: 'error', message: 'clientId is required' });
    }

    const record = profilingClients.get(clientId) || {};
    const updated = {
        clientId,
        namespace: namespace || record.namespace || '',
        podName: podName || record.podName || '',
        nodeName: nodeName || record.nodeName || '',
        labels: labels || record.labels || {},
        capabilities: Array.isArray(capabilities) && capabilities.length > 0
            ? capabilities
            : (record.capabilities || []),
        status: record.status || 'Idle',
        lastHeartbeat: nowTs(),
        runningTaskIds: record.runningTaskIds || [],
    };
    profilingClients.set(clientId, updated);
    schedulePersist('clients');

    return res.json({ code: 'Success', message: 'client registered', data: updated });
});

app.post('/api/clients/heartbeat', async (req, res) => {
    const { clientId, status, runningTaskIds } = req.body || {};
    if (!clientId) {
        return res.status(400).json({ code: 'error', message: 'clientId is required' });
    }
    const record = profilingClients.get(clientId) || { clientId };
    record.lastHeartbeat = nowTs();
    if (status) {
        record.status = status;
    } else if (!record.status) {
        record.status = 'Idle';
    }
    if (Array.isArray(runningTaskIds)) {
        record.runningTaskIds = runningTaskIds;
    } else if (!record.runningTaskIds) {
        record.runningTaskIds = [];
    }
    profilingClients.set(clientId, record);
    schedulePersist('clients');

    return res.json({ code: 'Success', message: 'heartbeat ok', data: record });
});

// ==== HTTP long-poll agent control plane ====
// These endpoints mirror the WebSocket control frames (REGISTER / HEARTBEAT /
// TASK_RESULT). They exist for deployments behind reverse proxies that strip
// the `Connection: Upgrade` header (WS handshake degrades to a plain GET). A
// client picks its transport via the `TRANSPORT` env var; the server offers
// both channels unconditionally so ws-mode and poll-mode clients can coexist.
// The task-fetch endpoint (`GET /api/agent/poll`) is added in the next task
// together with the poll-aware dispatch branch.

app.post('/api/agent/register', async (req, res) => {
    const {
        clientId,
        namespace,
        podName,
        nodeName,
        labels = {},
        capabilities = [],
    } = req.body || {};

    if (!clientId) {
        return res.status(400).json({ code: 'error', message: 'clientId is required' });
    }

    const record = profilingClients.get(clientId) || {};
    const updated = {
        clientId,
        namespace: namespace || record.namespace || '',
        podName: podName || record.podName || '',
        nodeName: nodeName || record.nodeName || '',
        labels: labels || record.labels || {},
        capabilities: Array.isArray(capabilities) && capabilities.length > 0
            ? capabilities
            : (record.capabilities || []),
        status: 'Idle',
        lastHeartbeat: nowTs(),
        runningTaskIds: [],
        wsConnected: false,
        transport: 'poll',
    };
    profilingClients.set(clientId, updated);
    schedulePersist('clients');

    console.log(`Client ${clientId} registered via HTTP long-poll`);
    return res.json({ code: 'Success', clientId });
});

app.post('/api/agent/heartbeat', async (req, res) => {
    const { clientId, status, runningTaskIds, gpuProcs } = req.body || {};
    if (!clientId) {
        return res.status(400).json({ code: 'error', message: 'clientId is required' });
    }
    const record = profilingClients.get(clientId) || { clientId, transport: 'poll' };
    record.lastHeartbeat = nowTs();
    // Trust the client's self-reported status (mirrors the WS handler): the
    // agent knows whether it's Idle or Profiling more accurately than any
    // server-side Dispatching flag we might be holding.
    if (status) {
        record.status = status;
    } else if (!record.status) {
        record.status = 'Idle';
    }
    if (Array.isArray(runningTaskIds)) {
        record.runningTaskIds = runningTaskIds;
    } else if (!record.runningTaskIds) {
        record.runningTaskIds = [];
    }
    if (Array.isArray(gpuProcs)) record.gpuProcs = gpuProcs;
    if (!record.transport) record.transport = 'poll';
    profilingClients.set(clientId, record);
    schedulePersist('clients');

    if (record.status === 'Idle' && collectionQueue.depth(clientId) > 0) {
        collectionQueue.pump(clientId).catch((e) => console.error('[queue] pump failed:', e.message));
    }

    return res.json({ code: 'Success' });
});

// Long-poll for the next task. Returns immediately when one is already
// in flight or gets dispatched during the wait; otherwise resolves
// `{ task: null }` once `wait` seconds elapse and the client re-polls.
app.get('/api/agent/poll', async (req, res) => {
    const clientId = req.query.client_id;
    if (!clientId) {
        return res.status(400).json({ code: 'error', message: 'client_id is required' });
    }
    // Cap the hold below a typical proxy read timeout.
    const waitSecs = Math.min(parseInt(req.query.wait, 10) || 30, 60);

    const record = profilingClients.get(clientId) || { clientId, transport: 'poll', runningTaskIds: [] };
    // A poll is itself a liveness signal, so it refreshes the heartbeat clock.
    record.lastHeartbeat = nowTs();
    if (!record.transport) record.transport = 'poll';
    if (record.status === 'Offline' || !record.status) record.status = 'Idle';
    profilingClients.set(clientId, record);
    schedulePersist('clients');

    // A task dispatched while no poll was in flight was stashed for pickup.
    // Serve it once and clear the stash — the client will resubmit its poll
    // right after receiving the task, and re-serving here would loop.
    const pending = inflightItems.get(clientId);
    if (pending) {
        inflightItems.delete(clientId);
        return res.json({ task: pending });
    }

    // Register the waiter before pumping: pump() resolves the waiter
    // synchronously via dispatchQueuedTask, so a later registration would
    // miss its own task.
    const task = await new Promise((resolve) => {
        const timer = setTimeout(() => {
            pollWaiters.delete(clientId);
            resolve(null);
        }, waitSecs * 1000);
        // A client only ever has one poll open; a new one supersedes the old.
        dropPollWaiter(clientId);
        pollWaiters.set(clientId, { resolve, timer });
        // The peer may vanish mid-hold (proxy timeout, container restart);
        // release the slot so the next poll can register its own waiter.
        res.on('close', () => {
            if (pollWaiters.get(clientId)?.resolve === resolve) {
                pollWaiters.delete(clientId);
                clearTimeout(timer);
                resolve(null);
            }
        });

        if (collectionQueue.depth(clientId) > 0 && record.status === 'Idle') {
            collectionQueue.pump(clientId).catch((e) => console.error('[queue] pump failed:', e.message));
        }
    });

    return res.json({ task });
});

app.post('/api/agent/task_result', async (req, res) => {
    const { clientId, analysisId, status, message: taskMessage } = req.body || {};
    if (!analysisId) {
        return res.status(400).json({ code: 'error', message: 'analysisId is required' });
    }

    const record = taskResults.get(analysisId) || { analysisId };
    record.clientId = clientId || record.clientId;
    record.status = status || 'Unknown';
    record.message = taskMessage || '';
    record.ts = nowTs();
    taskResults.set(analysisId, record);
    schedulePersist('tasks');

    const client = profilingClients.get(clientId);
    if (client && client.status === 'Dispatching') {
        client.status = 'Idle';
        profilingClients.set(clientId, client);
        schedulePersist('clients');
    }
    clearDispatchWatchdog(analysisId);

    if (clientId) {
        inflightItems.delete(clientId);
        collectionQueue.markDone(clientId, analysisId);
        collectionQueue.pump(clientId).catch((e) => console.error('[queue] pump failed:', e.message));
    }

    console.log(`TASK_RESULT ${analysisId} status=${status} client=${clientId} (poll)`);
    return res.json({ code: 'Success' });
});

app.get('/api/me', async (req, res) => {
    const user = await auth.resolveUser(req);
    if (!user) {
        return res.status(401).json({
            code: 'error', message: 'not logged in',
            loginUrl: auth.loginUrl(), authMode: auth.AUTH_MODE,
        });
    }
    return res.json({
        code: 'Success',
        data: {
            userId: user.userId, username: user.username, nickname: user.nickname,
            isAdmin: auth.isAdmin(user.userId),
            authMode: auth.AUTH_MODE,
            loginUrl: auth.loginUrl(), logoutUrl: auth.logoutUrl(),
        },
    });
});

async function requireOwnedTrace(req, res, next) {
    const analysisId = req.query?.analysisId || req.body?.analysisId;
    if (!analysisId || !isValidTaskId(analysisId)) {
        return res.status(400).json({ code: 'error', message: 'analysisId must be a UUID' });
    }
    // Service callers admitted by requireAuthOrInternal read on behalf of the
    // owner, so per-user ownership does not apply to them.
    if (req._skipOwnership) return next();
    const args = taskArgumentsCache.get(analysisId) || loadTaskArgsFromDisk(analysisId);
    const ownerId = ownership.ownerOf(args);
    if (!ownership.canAccess({ ownerId, userId: req.user.userId, isAdmin: auth.isAdmin(req.user.userId) })) {
        return res.status(403).json({ code: 'error', message: 'forbidden: not your analysis' });
    }
    return next();
}

app.get('/api/clients', (req, res) => {
    const { namespace, nodeName, status } = req.query || {};
    const list = Array.from(profilingClients.values()).filter((c) => {
        if (c.status === 'Offline' && status !== 'Offline') return false;
        if (namespace && c.namespace !== namespace) return false;
        if (nodeName && c.nodeName !== nodeName) return false;
        if (status && c.status !== status) return false;
        return true;
    });
    return res.json({ code: 'Success', data: list });
});

// Pending LIST_GPU_PROCS requests: reqId -> { resolve, reject, timer }
const gpuProcsPending = new Map();

function parseNvidiaSmiCsv(text) {
    const lines = String(text || '').trim().split('\n').filter(Boolean);
    // Expected format: pid,process_name,used_gpu_memory [MiB]
    const procs = [];
    for (const line of lines) {
        // Skip header row
        if (/^pid/i.test(line.trim())) continue;
        const parts = line.split(',').map(s => s.trim());
        if (!parts[0] || !/^\d+$/.test(parts[0])) continue;
        procs.push({
            pid: parts[0],
            name: parts[1] || '',
            memMiB: parts[2] ? parseInt(String(parts[2]).replace(/[^\d]/g, ''), 10) || 0 : 0,
        });
    }
    return procs;
}

async function localNvidiaSmi() {
    return new Promise((resolve, reject) => {
        const child = spawn('nvidia-smi', ['--query-compute-apps=pid,process_name,used_gpu_memory', '--format=csv,noheader']);
        let out = '';
        let err = '';
        child.stdout.on('data', d => { out += d; });
        child.stderr.on('data', d => { err += d; });
        child.on('error', reject);
        child.on('exit', code => {
            if (code === 0) resolve(parseNvidiaSmiCsv(out));
            else reject(new Error(err.trim() || `nvidia-smi exit ${code}`));
        });
    });
}

// GET /api/clients/:clientId/gpu-procs
// Lets the UI discover active CUDA processes on a capture client's host with one click.
app.get('/api/clients/:clientId/gpu-procs', async (req, res) => {
    const clientId = req.params.clientId;
    const timeoutMs = Math.min(parseInt(req.query.timeoutMs, 10) || 3000, 10000);

    let clientReason = 'client not connected';
    const ws = wsConnections.get(clientId);
    const clientRecord = profilingClients.get(clientId);

    // Poll clients push their GPU proc list on every heartbeat; serve that
    // cache directly — there is no server→client request channel for poll.
    if (clientRecord && clientRecord.transport === 'poll' && Array.isArray(clientRecord.gpuProcs)) {
        return res.json({ code: 'Success', data: clientRecord.gpuProcs, source: 'client-heartbeat' });
    }

    if (ws && ws.readyState === WebSocket.OPEN) {
        const reqId = crypto.randomUUID();
        const wait = new Promise((resolve, reject) => {
            const timer = setTimeout(() => {
                gpuProcsPending.delete(reqId);
                reject(new Error('client did not respond in time'));
            }, timeoutMs);
            gpuProcsPending.set(reqId, { resolve, reject, timer });
        });
        try {
            ws.send(JSON.stringify({ type: 'LIST_GPU_PROCS', reqId }));
        } catch (e) {
            gpuProcsPending.delete(reqId);
            return res.status(500).json({ code: 'Error', message: `ws send failed: ${e.message}` });
        }
        try {
            const procs = await wait;
            return res.json({ code: 'Success', data: procs, source: 'client' });
        } catch (e) {
            clientReason = e.message;
            console.warn(`LIST_GPU_PROCS via client ${clientId} failed: ${clientReason} — fallback to local nvidia-smi`);
        }
    }

    try {
        const procs = await localNvidiaSmi();
        return res.json({ code: 'Success', data: procs, source: 'server-local' });
    } catch (e) {
        return res.status(503).json({
            code: 'Error',
            message: `no GPU procs available: client ${clientId} — ${clientReason}; server-side nvidia-smi: ${e.message}`,
            data: [],
        });
    }
});

// === Endpoint 1: start AI analysis (dispatched to a client over WebSocket) ===
app.post('/api/v1/app_observ/aiAnalysis/start_ai_analysis', auth.requireAuth, async (req, res) => {
    const data = req.body;
    const targetClientId = data.instance;

    if (!targetClientId) {
        return res.status(400).json({ code: 'error', message: 'instance (clientId) is required' });
    }

    const clientRecord = profilingClients.get(targetClientId);
    if (!clientRecord) {
        return res.status(400).json({ code: 'error', message: `Client ${targetClientId} not registered` });
    }

    const ws = wsConnections.get(targetClientId);
    const isPoll = clientRecord.transport === 'poll';
    // ws clients must have a live socket; poll clients have no persistent
    // connection, so we treat any non-Offline poll client as reachable and
    // let the queue hold the task until its next poll.
    if (!isPoll && (!ws || ws.readyState !== WebSocket.OPEN)) {
        return res.status(400).json({ code: 'error', message: `Client ${targetClientId} not connected via WebSocket` });
    }
    if (isPoll && clientRecord.status === 'Offline') {
        return res.status(400).json({ code: 'error', message: `Client ${targetClientId} offline` });
    }

    const analysisId = crypto.randomUUID();

    const argsForCache = {
        uid: '1808078950770264',
        pids: data.pids || '',
        comms: data.comms || '',
        region: data.region || '',
        channel: 'offline',
        timeout: data.timeout || 2000,
        instance: targetClientId,
        created_by: req.user.userId,
        iteration_mod: data.iteration_mod || '',
        iteration_func: data.iteration_func || '',
        iteration_range: data.iteration_range || null,
        analysis_params: data.analysis_params || [],
        dispatchedAt: nowTs(),
    };

    const profilingConfig = {
        timeout: data.timeout,
        iteration: data.iteration_range,
        iteration_module: data.iteration_mod,
        iteration_function: data.iteration_func,
        analysis_params: data.analysis_params || [],
        pids: data.pids,
        comms: data.comms,
    };

    const { accepted, position, reason } = collectionQueue.enqueue(targetClientId, {
        analysisId, owner: req.user.userId, profilingConfig, argsForCache,
    });
    if (!accepted) {
        return res.status(429).json({ code: 'error', message: reason, queueFull: true });
    }

    taskArgumentsCache.set(analysisId, argsForCache);
    persistTaskArgs(analysisId, argsForCache);

    if (clientRecord.status === 'Idle') {
        await collectionQueue.pump(targetClientId);
    }

    const queuePosition = collectionQueue.positionOf(targetClientId, analysisId);
    return res.json({
        code: 'Success',
        message: queuePosition
            ? `已排队，当前第 ${queuePosition} 位`
            : 'Profiling task dispatched via WebSocket',
        analysisId,
        queued: queuePosition !== null,
        queuePosition,
        data: { analysisId, queued: queuePosition !== null, queuePosition },
    });
});

// ==== Upload =====
const upload = multer({
    dest: UPLOAD_TMP,
    limits: { fileSize: 3 * 1024 * 1024 * 1024 },
});

// Fix 2 helper: reject a tar whose entry names would escape the target dir.
async function tarPreflight(tarFilePath) {
    return new Promise((resolve, reject) => {
        const child = spawn('tar', ['-tf', tarFilePath]);
        let out = '';
        let err = '';
        child.stdout.on('data', (d) => { out += d.toString(); });
        child.stderr.on('data', (d) => { err += d.toString(); });
        child.on('error', reject);
        child.on('close', (code) => {
            if (code !== 0) return reject(new Error(`tar -tf failed (${code}): ${err}`));
            const bad = out.split('\n').map((s) => s.trim()).filter(Boolean)
                .find((entry) => entry.startsWith('/')
                    || entry.startsWith('../')
                    || entry.includes('/../')
                    || entry === '..');
            if (bad) return reject(new Error(`tar entry escapes target: ${bad}`));
            resolve();
        });
    });
}

app.post('/api/results/upload', upload.single('file'), async (req, res) => {
    const { taskId, clientId } = req.body;

    if (!taskId || !clientId) {
        if (req.file) { try { await fs.unlink(req.file.path); } catch {} }
        return res.status(400).json({ code: 'error', message: 'taskId and clientId are required' });
    }

    // Fix 1: validate taskId shape before it lands in any path
    if (!isValidTaskId(taskId)) {
        if (req.file) { try { await fs.unlink(req.file.path); } catch {} }
        return res.status(400).json({ code: 'error', message: `taskId must be a UUID` });
    }

    if (!req.file) {
        return res.status(400).json({ code: 'error', message: 'file is required' });
    }

    const uploadedFilePath = req.file.path;
    const analysisId = taskId;
    const outputDir = path.join(RESULT_DIR, analysisId);

    try {
        await fs.mkdir(RESULT_DIR, { recursive: true });
        await fs.mkdir(outputDir, { recursive: true });

        console.log(`开始 gz 解压: ${uploadedFilePath} → ${outputDir}`);

        const tarFilePath = path.join(outputDir, 'profiling-data.tar');
        const gunzip = zlib.createGunzip();
        const readStream = fsSync.createReadStream(uploadedFilePath);
        const writeStream = fsSync.createWriteStream(tarFilePath);

        writeStream.on('error', (err) => console.error(`Write stream error: ${err.message}`));

        await pipeline(readStream, gunzip, writeStream);
        console.log(`gz 解压完成 → ${tarFilePath}`);

        // Fix 2: preflight scan for tar-slip before extraction
        await tarPreflight(tarFilePath);

        // Fix 2: --no-same-owner hardens extract. Absolute/parent-escape
        // entries are already rejected by tarPreflight above, so we do not
        // need --no-absolute-names (which GNU tar < 1.32 does not recognize).
        await new Promise((resolve, reject) => {
            const tar = spawn('tar', [
                '-xf', tarFilePath,
                '-C', outputDir,
                '--no-same-owner',
            ]);
            tar.stdout.on('data', (d) => console.log(`[tar] ${d}`));
            tar.stderr.on('data', (d) => console.error(`[tar ERROR] ${d}`));
            tar.on('close', async (code) => {
                try { await fs.unlink(tarFilePath); } catch {}
                if (code === 0) { console.log(`tar 解压完成 → ${outputDir}`); resolve(); }
                else reject(new Error(`tar exited with code ${code}`));
            });
            tar.on('error', reject);
        });

        // Prefer PyInstaller-packaged binary; fall back to `python3 analysis.py`
        // (env AIPROF_PYTHON overrides interpreter). Both invocations pass
        // `-d <outputDir>` — analysis.py's __main__ mirrors the binary CLI.
        const summaryBin = path.join(__dirname, 'analysis_summary');
        const analysisPy = path.join(__dirname, 'analysis.py');
        let analysisCmd = null;
        let analysisArgs = null;
        let analysisLabel = '';
        if (fsSync.existsSync(summaryBin)) {
            analysisCmd = summaryBin;
            analysisArgs = ['-d', outputDir];
            analysisLabel = 'analysis_summary';
        } else if (fsSync.existsSync(analysisPy)) {
            analysisCmd = process.env.AIPROF_PYTHON || 'python3';
            analysisArgs = [analysisPy, '-d', outputDir];
            analysisLabel = 'analysis.py';
        }
        if (analysisCmd) {
            console.log(`开始执行 ${analysisLabel} (cmd=${analysisCmd} args=${analysisArgs.join(' ')})`);
            const analysis = spawn(analysisCmd, analysisArgs, { cwd: __dirname });
            let stderrTail = '';
            analysis.stdout.on('data', (d) => console.log(`[${analysisLabel}] ${d}`));
            analysis.stderr.on('data', (d) => {
                console.error(`[${analysisLabel} ERROR] ${d}`);
                // Keep the last ~2KB so list_record can surface a useful failedLog
                stderrTail = (stderrTail + d.toString()).slice(-2048);
            });
            analysis.on('close', async (code) => {
                console.log(`${analysisLabel} 退出，代码: ${code}`);
                try { await fs.unlink(uploadedFilePath); } catch {}
                const summaryPath = path.join(outputDir, 'Analysis_Summary.json');
                const summaryExists = fsSync.existsSync(summaryPath);
                if (code === 0 && summaryExists) {
                    // Analysis done → generate the rule-based HTML report.
                    const reportPath = path.join(outputDir, 'Analysis_Report.html');
                    const reportPy = path.join(__dirname, 'report.py');
                    if (fsSync.existsSync(reportPy)) {
                        const py = process.env.AIPROF_PYTHON || 'python3';
                        const rep = spawn(py, [reportPy, summaryPath, reportPath], { cwd: __dirname });
                        rep.stdout.on('data', (d) => console.log(`[report.py] ${d}`));
                        rep.stderr.on('data', (d) => console.error(`[report.py ERROR] ${d}`));
                        rep.on('close', (c) => console.log(`report.py 退出，代码: ${c} → ${reportPath}`));
                    }
                } else {
                    // Analysis crashed (code != 0), or exited 0 without writing
                    // Analysis_Summary.json (e.g. empty/unparseable trace).
                    // Drop a marker so list_record shows Failed with a real
                    // reason instead of "analyzing" forever.
                    const reason = code === 0
                        ? `${analysisLabel} 退出码 0 但未生成 Analysis_Summary.json（trace 可能为空或无可分析事件）`
                        : `${analysisLabel} 退出码 ${code}${stderrTail ? '：\n' + stderrTail.trim() : ''}`;
                    try {
                        await fs.writeFile(
                            path.join(outputDir, 'analysis_failed.json'),
                            JSON.stringify({ code, label: analysisLabel, reason, ts: Date.now() }, null, 2),
                        );
                    } catch (e) {
                        console.error('写 analysis_failed.json 失败:', e);
                    }
                }
            });
        } else {
            console.warn(`analysis_summary 与 analysis.py 均不存在，跳过后处理 (dir=${__dirname})`);
            try { await fs.unlink(uploadedFilePath); } catch {}
            try {
                await fs.writeFile(
                    path.join(outputDir, 'analysis_failed.json'),
                    JSON.stringify({
                        code: -1,
                        label: 'analysis',
                        reason: 'analysis_summary 与 analysis.py 均不存在，server 镜像未打包分析器',
                        ts: Date.now(),
                    }, null, 2),
                );
            } catch (e) {
                console.error('写 analysis_failed.json 失败:', e);
            }
        }

        return res.json({
            code: 'Success',
            message: 'File uploaded and analysis started',
            data: { taskId, analysisId },
        });
    } catch (err) {
        console.error('处理上传文件失败:', err);
        try { await fs.unlink(uploadedFilePath); } catch {}
        // If we bailed after mkdir, wipe the empty dir so list_record does not
        // surface a ghost record.
        try { await fs.rm(outputDir, { recursive: true, force: true }); } catch {}
        return res.status(err.message?.includes('escapes target') ? 400 : 500).json({
            code: 'error',
            message: 'Failed to process uploaded file',
            error: err.message,
        });
    }
});

// === Endpoint 2: query analysis result ===
app.post('/api/v1/app_observ/aiAnalysis/query_result', auth.requireAuth, requireOwnedTrace, async (req, res) => {
    const { analysisId } = req.body || {};

    if (!analysisId) {
        return res.status(400).json({ code: 'error', message: 'analysisId is required', data: {} });
    }
    if (!isValidTaskId(analysisId)) {
        return res.status(400).json({ code: 'error', message: 'analysisId must be a UUID', data: {} });
    }

    try {
        const resultDir = path.join(RESULT_DIR, analysisId);
        try { await fs.access(resultDir); } catch {
            return res.json({
                code: 'error',
                message: `Analysis with ID '${analysisId}' not found`,
                request_id: '',
                data: {},
            });
        }

        const summaryFilePath = path.join(resultDir, 'Analysis_Summary.json');
        try {
            const fileContent = await fs.readFile(summaryFilePath, 'utf-8');
            const summaryData = JSON.parse(fileContent);
            return res.json(summaryData);
        } catch (err) {
            console.error('Error reading Analysis_Summary.json:', err);
            return res.json({
                code: 'error',
                message: `Analysis_Summary.json not found or invalid for ID '${analysisId}'`,
                request_id: '',
                data: {},
            });
        }
    } catch (err) {
        console.error('Error in query_result:', err);
        return res.status(500).json({ code: 'error', message: 'Internal Server Error', error: err.message });
    }
});

// === Endpoint 2.6: stream a persisted chrome-trace JSON to Perfetto ===
// GET /api/v1/app_observ/aiAnalysis/trace?analysisId=<uuid>[&pid=<pid>]
// - analysisId is validated against a UUID whitelist
// - excludes Analysis_Summary.json / Analysis_Report.html from the dir; picks the
//   largest same-pid *.json (typically AIProf_<pid>.json)
// - supports HTTP Range (Perfetto UI does chunked reads) and gzip passthrough (.json.gz)
// The only endpoint a companion service may reach with AIPROF_INTERNAL_TOKEN:
// a read of one trace by id. Every other authenticated endpoint stays on plain
// requireAuth, so the shared secret cannot start, delete, or reconfigure work.
app.get('/api/v1/app_observ/aiAnalysis/trace', auth.requireAuthOrInternal, requireOwnedTrace, async (req, res) => {
    const analysisId = req.query.analysisId;
    const pid = req.query.pid ? String(req.query.pid) : null;
    if (!analysisId || !isValidTaskId(analysisId)) {
        return res.status(400).send('analysisId must be a UUID');
    }
    const resultDir = path.join(RESULT_DIR, analysisId);
    let entries;
    try { entries = await fs.readdir(resultDir); } catch {
        return res.status(404).send('analysis dir not found');
    }
    const candidates = [];
    for (const name of entries) {
        if (name === 'Analysis_Summary.json' || name === 'Analysis_Report.html') continue;
        if (!/\.json(\.gz)?$/i.test(name)) continue;
        if (pid) {
            const stem = name.replace(/\.json(\.gz)?$/i, '');
            const tail = stem.split('_').pop();
            if (tail !== pid) continue;
        }
        const p = path.join(resultDir, name);
        try {
            const st = await fs.stat(p);
            candidates.push({ path: p, size: st.size, name });
        } catch {}
    }
    if (!candidates.length) return res.status(404).send('trace not found');
    candidates.sort((a, b) => b.size - a.size);
    const file = candidates[0];
    const isGz = /\.gz$/i.test(file.name);
    res.setHeader('Content-Type', 'application/json');
    if (isGz) res.setHeader('Content-Encoding', 'gzip');
    res.setHeader('Accept-Ranges', 'bytes');
    res.setHeader('Access-Control-Allow-Origin', '*');
    // Range support (Perfetto UI reads large files in chunks)
    const range = req.headers.range;
    if (range && !isGz) {
        const m = /^bytes=(\d*)-(\d*)$/.exec(range);
        if (m) {
            const start = m[1] ? parseInt(m[1], 10) : 0;
            const end = m[2] ? parseInt(m[2], 10) : file.size - 1;
            if (start >= file.size || end >= file.size || start > end) {
                res.setHeader('Content-Range', `bytes */${file.size}`);
                return res.status(416).end();
            }
            res.status(206);
            res.setHeader('Content-Range', `bytes ${start}-${end}/${file.size}`);
            res.setHeader('Content-Length', end - start + 1);
            fsSync.createReadStream(file.path, { start, end }).pipe(res);
            return;
        }
    }
    res.setHeader('Content-Length', file.size);
    fsSync.createReadStream(file.path).pipe(res);
});

// === Endpoint 2.5: read (or generate on demand) the rule-based HTML report ===
// GET /api/v1/app_observ/aiAnalysis/report?analysisId=<uuid>
// Returns text/html directly; if Analysis_Report.html is missing but Analysis_Summary.json
// exists, invokes report.py on the fly to produce one.
app.get('/api/v1/app_observ/aiAnalysis/report', async (req, res) => {
    const analysisId = req.query.analysisId;
    if (!analysisId || !isValidTaskId(analysisId)) {
        return res.status(400).send('analysisId must be a UUID');
    }
    const resultDir = path.join(RESULT_DIR, analysisId);
    const reportPath = path.join(resultDir, 'Analysis_Report.html');
    const summaryPath = path.join(resultDir, 'Analysis_Summary.json');
    try {
        try {
            const html = await fs.readFile(reportPath, 'utf-8');
            res.setHeader('Content-Type', 'text/html; charset=utf-8');
            return res.send(html);
        } catch {}
        // Fallback: generate now
        try { await fs.access(summaryPath); } catch {
            return res.status(404).send('Analysis_Summary.json 未就绪');
        }
        const reportPy = path.join(__dirname, 'report.py');
        if (!fsSync.existsSync(reportPy)) return res.status(500).send('report.py 缺失');
        const py = process.env.AIPROF_PYTHON || 'python3';
        await new Promise((resolve, reject) => {
            const rep = spawn(py, [reportPy, summaryPath, reportPath], { cwd: __dirname });
            let err = '';
            rep.stderr.on('data', (d) => { err += d.toString(); });
            rep.on('close', (c) => c === 0 ? resolve() : reject(new Error(err || `report.py exit ${c}`)));
            rep.on('error', reject);
        });
        const html = await fs.readFile(reportPath, 'utf-8');
        res.setHeader('Content-Type', 'text/html; charset=utf-8');
        return res.send(html);
    } catch (err) {
        console.error('report endpoint error:', err);
        return res.status(500).send('生成报告失败: ' + err.message);
    }
});

// === Endpoint 4: diff_analysis — rule-based diff between two step ranges of the same pid ===
// POST body: { task1: {analysisId, pids:[pid], step_start, step_end}, task2: {...} }
// Returns (matches the frontend contract): {code:'Success', data:{data:"<json string>", flamegraph:"<json string>"}}
app.post('/api/v1/app_observ/aiAnalysis/diff_analysis', auth.requireAuth, async (req, res) => {
    const body = req.body || {};
    const t1 = body.task1 || {};
    const t2 = body.task2 || {};
    const norm = (t) => ({
        analysisId: t.analysisId,
        pid: Array.isArray(t.pids) ? String(t.pids[0]) : (t.pid !== undefined ? String(t.pid) : ''),
        step_start: Number(t.step_start),
        step_end: Number(t.step_end),
    });
    const n1 = norm(t1);
    const n2 = norm(t2);
    if (!isValidTaskId(n1.analysisId) || !isValidTaskId(n2.analysisId)) {
        return res.status(400).json({ code: 'error', message: 'analysisId must be UUID', data: {} });
    }
    // requireOwnedTrace only inspects a single id, so diff checks both here.
    for (const aid of [n1.analysisId, n2.analysisId]) {
        const args = taskArgumentsCache.get(aid) || loadTaskArgsFromDisk(aid);
        const ownerId = ownership.ownerOf(args);
        if (!ownership.canAccess({ ownerId, userId: req.user.userId, isAdmin: auth.isAdmin(req.user.userId) })) {
            return res.status(403).json({ code: 'error', message: `forbidden: not your analysis ${aid}` });
        }
    }
    if (!n1.pid || !n2.pid || !Number.isFinite(n1.step_start) || !Number.isFinite(n1.step_end)
        || !Number.isFinite(n2.step_start) || !Number.isFinite(n2.step_end)) {
        return res.status(400).json({ code: 'error', message: 'invalid task payload', data: {} });
    }
    const diffPy = path.join(__dirname, 'diff_analysis.py');
    if (!fsSync.existsSync(diffPy)) {
        return res.status(500).json({ code: 'error', message: 'diff_analysis.py missing', data: {} });
    }
    const py = process.env.AIPROF_PYTHON || 'python3';
    const argv = [diffPy,
        '--result-dir', RESULT_DIR,
        '--task1', JSON.stringify(n1),
        '--task2', JSON.stringify(n2),
    ];
    try {
        const out = await new Promise((resolve, reject) => {
            const child = spawn(py, argv, { cwd: __dirname });
            let stdout = '';
            let stderr = '';
            child.stdout.on('data', (d) => { stdout += d.toString(); });
            child.stderr.on('data', (d) => { stderr += d.toString(); });
            child.on('error', reject);
            child.on('close', (code) => {
                if (code !== 0) reject(new Error(stderr || `diff_analysis.py exit ${code}`));
                else resolve(stdout);
            });
        });
        let parsed;
        try { parsed = JSON.parse(out); }
        catch (e) { throw new Error('diff_analysis.py returned invalid JSON: ' + e.message); }
        // Frontend diffAnalysis.tsx does JSON.parse(JSON.parse(res.data)["data"]) — top-level `data` must be a JSON string
        return res.json({
            code: 'Success',
            message: '',
            data: JSON.stringify(parsed),
            request_id: '',
        });
    } catch (err) {
        console.error('diff_analysis error:', err);
        return res.status(500).json({ code: 'error', message: String(err.message || err), data: {} });
    }
});

// === Endpoint: delete an analysisId — cleans up both the result directory and the in-memory record
app.delete('/api/v1/app_observ/aiAnalysis/delete_record', auth.requireAuth, requireOwnedTrace, async (req, res) => {
    const analysisId = req.query.analysisId;
    if (!analysisId) {
        return res.status(400).json({ code: 'error', message: 'analysisId is required' });
    }
    if (!isValidTaskId(analysisId)) {
        return res.status(400).json({ code: 'error', message: 'analysisId must be a UUID' });
    }
    try {
        const dirPath = path.join(RESULT_DIR, analysisId);
        await fs.rm(dirPath, { recursive: true, force: true });
        taskResults.delete(analysisId);
        taskArgumentsCache.delete(analysisId);
        schedulePersist('tasks');
        return res.json({ code: 'Success', message: 'deleted', data: { analysisId } });
    } catch (err) {
        console.error('delete_record error:', err);
        return res.status(500).json({ code: 'error', message: String(err.message || err) });
    }
});

// === Endpoint 3: list_record — merge result dirs with tasks.json (Fix 5)
app.get('/api/v1/app_observ/aiAnalysis/list_record', auth.requireAuth, async (req, res) => {
    // Ownership comes from the session, not the query string. The previous
    // behaviour let any caller pass created_by=<someone else> and read their
    // records.
    const viewerId = req.user.userId;
    const viewerIsAdmin = auth.isAdmin(viewerId);
    const current = parseInt(req.query.current, 10) || 1;
    const pageSize = parseInt(req.query.pageSize, 10) || 10;

    try {
        const allRecords = [];
        let idCounter = 1;

        try {
            await fs.mkdir(RESULT_DIR, { recursive: true });
            const entries = await fs.readdir(RESULT_DIR, { withFileTypes: true });
            // Fix 1: dir name (from client-uploaded taskId) must also be a UUID
            const directories = entries.filter((e) => e.isDirectory() && isValidTaskId(e.name));

            for (const dir of directories) {
                const dirPath = path.join(RESULT_DIR, dir.name);
                const stats = await fs.stat(dirPath);
                // Cache-miss (server was restarted, or the task was dispatched
                // by a previous process): fall back to task_args.json we wrote
                // at dispatch time. Only when both are absent do we use the
                // "offline-task-node" placeholder.
                const cachedArgs = taskArgumentsCache.get(dir.name) || loadTaskArgsFromDisk(dir.name);
                const ownerId = ownership.ownerOf(cachedArgs);
                if (!ownership.canAccess({ ownerId, userId: viewerId, isAdmin: viewerIsAdmin })) continue;
                const taskResult = taskResults.get(dir.name);

                const defaults = {
                    uid: '1808078950770264',
                    channel: 'offline',
                    instance: 'offline-task-node',
                    created_by: 'system',
                    timeout: 2000,
                };
                const taskArgs = {
                    uid: cachedArgs?.uid || defaults.uid,
                    channel: cachedArgs?.channel || defaults.channel,
                    instance: cachedArgs?.instance || defaults.instance,
                    created_by: cachedArgs?.created_by || defaults.created_by,
                    timeout: cachedArgs?.timeout || defaults.timeout,
                    pids: cachedArgs?.pids || '',
                    region: cachedArgs?.region || '',
                    iteration_mod: cachedArgs?.iteration_mod || '',
                    iteration_func: cachedArgs?.iteration_func || '',
                    analysis_params: cachedArgs?.analysis_params || [],
                };

                const analysisTime = new Date(stats.birthtime).toLocaleString('zh-CN', {
                    timeZone: 'Asia/Shanghai',
                    year: 'numeric', month: '2-digit', day: '2-digit',
                    hour: '2-digit', minute: '2-digit', second: '2-digit',
                    hour12: false,
                }).replace(/\//g, '-');

                let status = '分析成功';
                let failedReason = '';
                try { await fs.access(path.join(dirPath, 'Analysis_Summary.json')); }
                catch {
                    // No summary yet. Three cases:
                    //   1) upload_result already wrote analysis_failed.json → explicit failure
                    //   2) dir mtime exceeded (timeout + 120s) → stuck, mark as failed
                    //      (avoids the frontend spinning forever when analysis_summary crashes/is silent)
                    //   3) neither → still running, keep showing "analyzing"
                    status = '分析中';
                    try {
                        const failMarker = await fs.readFile(
                            path.join(dirPath, 'analysis_failed.json'), 'utf-8',
                        );
                        const parsed = JSON.parse(failMarker);
                        status = '分析失败';
                        failedReason = parsed.reason || `analysis_summary 退出码 ${parsed.code}`;
                    } catch {
                        const timeoutMs = Number(taskArgs.timeout) || 2000;
                        const stalePivot = stats.mtimeMs + timeoutMs + 120_000;
                        if (Date.now() > stalePivot) {
                            status = '分析失败';
                            failedReason = `目录已存在 ${Math.round((Date.now() - stats.mtimeMs) / 1000)}s (超过 timeout ${timeoutMs}ms + 120s 宽限期) 仍无 Analysis_Summary.json，analysis_summary 可能已崩溃或没生成输出`;
                        }
                    }
                }
                // Fix 5: agent-reported Failed wins over the filesystem check
                if (taskResult?.status === 'Failed') status = '分析失败';

                let hasReport = false;
                try { await fs.access(path.join(dirPath, 'Analysis_Report.html')); hasReport = true; }
                catch {}

                allRecords.push({
                    uid: taskArgs.uid,
                    created_by: taskArgs.created_by,
                    analysisId: dir.name,
                    analysisTime,
                    status,
                    arguments: JSON.stringify(taskArgs),
                    analysisResult: '',
                    region: taskArgs.region,
                    failedLog: taskResult?.message || failedReason || '',
                    hasReport,
                    id: idCounter++,
                });
            }
        } catch (dirErr) {
            console.error('Error reading result directory:', dirErr);
        }

        // Fix 5: also surface tasks that failed before writing any files
        for (const [analysisId, r] of taskResults.entries()) {
            if (!isValidTaskId(analysisId)) continue;
            if (allRecords.find((rec) => rec.analysisId === analysisId)) continue;
            if (r.status !== 'Failed') continue;
            const cachedArgs = taskArgumentsCache.get(analysisId) || loadTaskArgsFromDisk(analysisId) || {};
            const failedOwnerId = ownership.ownerOf(cachedArgs);
            if (!ownership.canAccess({ ownerId: failedOwnerId, userId: viewerId, isAdmin: viewerIsAdmin })) continue;
            allRecords.push({
                uid: cachedArgs.uid || '1808078950770264',
                created_by: cachedArgs.created_by || 'system',
                analysisId,
                analysisTime: new Date(r.ts || Date.now()).toLocaleString('zh-CN', {
                    timeZone: 'Asia/Shanghai',
                    year: 'numeric', month: '2-digit', day: '2-digit',
                    hour: '2-digit', minute: '2-digit', second: '2-digit',
                    hour12: false,
                }).replace(/\//g, '-'),
                status: '分析失败',
                arguments: JSON.stringify(cachedArgs),
                analysisResult: '',
                region: cachedArgs.region || '',
                failedLog: r.message || '',
                id: idCounter++,
            });
        }

        // In-flight surfacing: any dispatched task without a result dir and
        // without a TASK_RESULT yet is still capturing/analyzing. Show it so the user
        // knows the collection is running. If the client's timeout + 60s grace
        // has elapsed and we still haven't heard back, mark it as a capture timeout
        // so the row doesn't lie for up to an hour.
        for (const [analysisId, cachedArgs] of taskArgumentsCache.entries()) {
            if (!isValidTaskId(analysisId)) continue;
            if (allRecords.find((rec) => rec.analysisId === analysisId)) continue;
            if (taskResults.has(analysisId)) continue;
            const inflightOwnerId = ownership.ownerOf(cachedArgs);
            if (!ownership.canAccess({ ownerId: inflightOwnerId, userId: viewerId, isAdmin: viewerIsAdmin })) continue;
            const dispatchedAt = cachedArgs.dispatchedAt || Date.now();
            const timeoutMs = Number(cachedArgs.timeout) || 2000;
            const elapsed = Date.now() - dispatchedAt;
            // After 1h, forget: the dispatch watchdog would have long cleared it.
            if (elapsed > 60 * 60 * 1000) continue;
            const queuePos = collectionQueue.positionOf(cachedArgs.instance, analysisId);
            const isStale = elapsed > timeoutMs + 120_000;
            const phase = queuePos !== null
                ? `排队中(第${queuePos}位)`
                : (elapsed <= timeoutMs ? '采集中' : (isStale ? '采集超时' : '分析中'));
            allRecords.push({
                uid: cachedArgs.uid || '1808078950770264',
                created_by: cachedArgs.created_by || 'system',
                analysisId,
                analysisTime: new Date(dispatchedAt).toLocaleString('zh-CN', {
                    timeZone: 'Asia/Shanghai',
                    year: 'numeric', month: '2-digit', day: '2-digit',
                    hour: '2-digit', minute: '2-digit', second: '2-digit',
                    hour12: false,
                }).replace(/\//g, '-'),
                status: phase,
                arguments: JSON.stringify(cachedArgs),
                analysisResult: '',
                region: cachedArgs.region || '',
                failedLog: isStale ? `任务已下发 ${Math.round(elapsed / 1000)}s，超过预期采集时长 ${timeoutMs}ms + 120s 宽限期，仍未收到 TASK_RESULT。可能 client 崩溃或上传/分析卡住。` : '',
                hasReport: false,
                id: idCounter++,
            });
        }

        allRecords.sort((a, b) => new Date(b.analysisTime) - new Date(a.analysisTime));

        const total = allRecords.length;
        const startIndex = (current - 1) * pageSize;
        const paginatedData = allRecords.slice(startIndex, startIndex + pageSize);

        return res.json({
            code: 'Success',
            message: '',
            data: paginatedData,
            total,
            request_id: '',
        });
    } catch (err) {
        console.error('Error in list_record:', err);
        return res.status(500).json({ code: 'error', message: 'Failed to list records', error: err.message });
    }
});

// ============================================================================
// Profiling Agent proxy — the frontend "analyze" button calls a local LLM through here.
//
// Two modes:
//   mode=openclaw → the server spawns a local openclaw agent (see runOpenclawAgent)
//   mode=local    → local agent analysis (local-profiling-agent: ingest + tools + synth)
//
// The Aliyun Agent has been moved to a frontend external link (soma.openanolis.cn) and
// no longer flows through this endpoint.
//
// The backend only aggregates context and forwards the protocol; it does not cache the API key.
// ============================================================================
const AIPROF_SYSTEM_PROMPT = `你是资深 GPU / PyTorch 性能优化专家 (Profiling Agent)。
用户会给你一份 AIProf 性能分析报告的关键指标 (JSON)。请：
1) 用 3-5 条要点指出主要瓶颈（GPU 利用率、SM 利用率、TensorCore 空转、显存分配抖动、kernel launch 延迟等）；
2) 对 top kernel 逐一给出可落地的优化建议（融合 / 换 CUDA 内核 / 用 cutlass / channels_last / fp16 / graph capture 等）；
3) 如出现 memory spikes 或 OOM，指出可能的模型侧原因和缓解方案；
4) 每条结论都要给出「代码/配置」级别的具体修复建议，避免空话。
请用中文回答，Markdown 结构清晰。`;

function buildAgentPrompt(question, sessionDigest) {
    const q = (question || '').trim() || '请深入分析并给出优化建议。';
    let digest = '';
    try { digest = JSON.stringify(sessionDigest || {}, null, 2); } catch (e) { digest = String(sessionDigest || ''); }
    if (digest.length > 20000) digest = digest.slice(0, 20000) + '\n...(truncated)';
    return `问题：${q}\n\n本次报告关键指标：\n\`\`\`json\n${digest}\n\`\`\``;
}

// Find the largest chrome-tracing JSON (AIProf_<pid>.json) in the result directory.
// The local agent analysis needs this file as its entry point.
// NOTE: some historical kernel-tracker .so builds still write `AIProfiling_<pid>.json`;
// this function accepts both prefixes and can drop the legacy one once all clients update.
function findTraceFile(resultDir) {
    let names;
    try { names = fsSync.readdirSync(resultDir); } catch { return null; }
    const perPidPattern = /^(AIProf|AIProfiling)_\d+\.json$/;
    const cands = names
        .filter((n) => perPidPattern.test(n))
        .map((n) => ({ n, size: fsSync.statSync(path.join(resultDir, n)).size }))
        .sort((a, b) => b.size - a.size);
    if (cands.length) return path.join(resultDir, cands[0].n);
    for (const legacy of ['AIProf_aggregation-kernel.json', 'AIProfiling_aggregation-kernel.json']) {
        if (names.includes(legacy)) return path.join(resultDir, legacy);
    }
    return null;
}

// AI analysis conclusions are persisted per analysisId at <RESULT_DIR>/<analysisId>/ai_conclusion.json,
// so different browsers / different teammates opening the same report can all see the latest one.
function aiConclusionPath(analysisId) {
    return path.join(RESULT_DIR, analysisId, 'ai_conclusion.json');
}

// ---------- LLM config persistence (consumed by local OpenClaw analysis) ----------
// Persisted at PERSISTENCE_DIR/llm-config.json (mode 0600); apiKey is stored server-side in plain text,
// but GET /api/settings/llm masks it to sk-****abcd before returning to the frontend.
// The value is only updated when the frontend POSTs a new apiKey explicitly; otherwise the old one is kept.
const LLM_PROVIDERS = ['dashscope', 'openai', 'deepseek', 'zhipu', 'moonshot', 'custom'];

function defaultLlmConfig() {
    return {
        provider: 'dashscope',
        baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
        apiKey: '',
        model: 'qwen3.5-plus',
        updatedAt: 0,
    };
}

async function loadLlmConfig() {
    try {
        const raw = await fs.readFile(LLM_CONFIG_FILE, 'utf-8');
        const j = JSON.parse(raw);
        return { ...defaultLlmConfig(), ...j };
    } catch (e) {
        if (e.code !== 'ENOENT') console.warn(`[llm-config] read failed: ${e.message}`);
        return defaultLlmConfig();
    }
}

async function saveLlmConfig(cfg) {
    await fs.mkdir(PERSISTENCE_DIR, { recursive: true });
    await fs.writeFile(LLM_CONFIG_FILE, JSON.stringify(cfg, null, 2), { encoding: 'utf-8', mode: 0o600 });
    try { await fs.chmod(LLM_CONFIG_FILE, 0o600); } catch {}
}

function maskApiKey(k) {
    if (!k) return '';
    if (k.length <= 10) return '*'.repeat(k.length);
    return `${k.slice(0, 4)}${'*'.repeat(Math.max(4, k.length - 8))}${k.slice(-4)}`;
}

// Optional companion service that can analyse a trace by pulling it from this
// instance. Empty means unconfigured, and the UI hides the entry point.
app.get('/api/settings/external-analyzer', (_req, res) => {
    const url = (process.env.AIPROF_EXTERNAL_ANALYZER_URL || '').trim().replace(/\/+$/, '');
    // When true, the companion can pull traces server-to-server by analysisId,
    // so the UI can skip the browser-download → manual-upload dance.
    const importEnabled = process.env.AIPROF_EXTERNAL_IMPORT === 'true';
    res.json({ code: 'ok', data: { url, importEnabled } });
});

app.get('/api/settings/llm', async (_req, res) => {
    const cfg = await loadLlmConfig();
    // env-var fallback signal so the UI can hint "administrator preconfigured a key"
    const envHasKey = !!(process.env.QWEN_API_KEY || process.env.DASHSCOPE_API_KEY);
    res.json({
        code: 'ok',
        data: {
            provider: cfg.provider,
            baseUrl: cfg.baseUrl,
            apiKeyMasked: maskApiKey(cfg.apiKey),
            hasApiKey: !!cfg.apiKey,
            model: cfg.model,
            updatedAt: cfg.updatedAt,
            envHasKey,
            storagePath: LLM_CONFIG_FILE,
        },
    });
});

app.post('/api/settings/llm', auth.requireAuth, async (req, res) => {
    try {
        const { provider, baseUrl, apiKey, model } = req.body || {};
        if (provider && !LLM_PROVIDERS.includes(provider)) {
            return res.status(400).json({ code: 'bad_request', message: `unknown provider: ${provider}` });
        }
        const cur = await loadLlmConfig();
        const next = {
            provider: provider || cur.provider,
            baseUrl: (baseUrl || cur.baseUrl || '').trim(),
            // If the frontend doesn't resend apiKey (or sends an empty string) → keep the old value; only update on an explicit string.
            apiKey: typeof apiKey === 'string' && apiKey.length > 0 ? apiKey.trim() : cur.apiKey,
            model: (model || cur.model || '').trim(),
            updatedAt: Date.now(),
        };
        if (!next.baseUrl) return res.status(400).json({ code: 'bad_request', message: 'baseUrl 必填' });
        if (!next.model)   return res.status(400).json({ code: 'bad_request', message: 'model 必填' });
        await saveLlmConfig(next);
        res.json({
            code: 'ok',
            data: {
                provider: next.provider,
                baseUrl: next.baseUrl,
                apiKeyMasked: maskApiKey(next.apiKey),
                hasApiKey: !!next.apiKey,
                model: next.model,
                updatedAt: next.updatedAt,
                envHasKey: !!(process.env.QWEN_API_KEY || process.env.DASHSCOPE_API_KEY),
                storagePath: LLM_CONFIG_FILE,
            },
        });
    } catch (e) {
        res.status(500).json({ code: 'write_failed', message: e.message });
    }
});

// Normalize whatever ai_conclusion.json contains into { conclusions: [...] }.
// Old shape (single object with {mode?, label, content, savedAt, ...}) gets
// wrapped so historical files continue to work with the multi-conclusion UI.
function normalizeConclusionsStore(raw) {
    if (!raw || typeof raw !== 'object') return { conclusions: [] };
    if (Array.isArray(raw.conclusions)) return { conclusions: raw.conclusions };
    if (raw.content || raw.text) {
        return { conclusions: [{
            mode: raw.mode || 'unknown',
            label: raw.label || 'AI 分析结果',
            content: raw.content || raw.text || '',
            model: raw.model,
            savedAt: raw.savedAt || Date.now(),
        }] };
    }
    return { conclusions: [] };
}

async function readConclusionsStore(analysisId) {
    try {
        const raw = await fs.readFile(aiConclusionPath(analysisId), 'utf-8');
        return normalizeConclusionsStore(JSON.parse(raw));
    } catch (e) {
        if (e.code !== 'ENOENT') console.warn(`[ai/analyze] read failed for ${analysisId}: ${e.message}`);
        return { conclusions: [] };
    }
}

async function saveAiConclusion(analysisId, payload) {
    if (!analysisId || !isValidTaskId(analysisId)) return;
    try {
        const dir = path.join(RESULT_DIR, analysisId);
        await fs.mkdir(dir, { recursive: true });
        const store = await readConclusionsStore(analysisId);
        const mode = payload.mode || 'unknown';
        // Overwrite the latest entry of the same mode; append across modes so the frontend can compare side by side.
        const kept = store.conclusions.filter((c) => c.mode !== mode);
        kept.push({ ...payload, mode, savedAt: Date.now() });
        await fs.writeFile(aiConclusionPath(analysisId), JSON.stringify({ conclusions: kept }, null, 2), 'utf-8');
    } catch (err) {
        console.warn(`[ai/analyze] failed to persist conclusion for ${analysisId}: ${err.message}`);
    }
}

app.get('/api/ai/analyze/result', auth.requireAuth, async (req, res) => {
    const analysisId = String(req.query.analysisId || '');
    if (!analysisId || !isValidTaskId(analysisId)) {
        return res.status(400).json({ code: 'bad_request', message: 'analysisId is required' });
    }
    try {
        const store = await readConclusionsStore(analysisId);
        if (!store.conclusions.length) return res.json({ code: 'ok', data: null });
        // Compat: surface the newest conclusion's fields at top level so any older
        // frontend build still sees a result.
        const newest = [...store.conclusions].sort((a, b) => (b.savedAt || 0) - (a.savedAt || 0))[0];
        return res.json({
            code: 'ok',
            data: {
                conclusions: store.conclusions,
                mode: newest.mode,
                label: newest.label,
                content: newest.content,
                model: newest.model,
                savedAt: newest.savedAt,
            },
        });
    } catch (err) {
        return res.status(500).json({ code: 'read_failed', message: err.message });
    }
});

app.delete('/api/ai/analyze/result', auth.requireAuth, async (req, res) => {
    const analysisId = String(req.query.analysisId || '');
    const modeFilter = req.query.mode ? String(req.query.mode) : null;
    if (!analysisId || !isValidTaskId(analysisId)) {
        return res.status(400).json({ code: 'bad_request', message: 'analysisId is required' });
    }
    try {
        if (modeFilter) {
            const store = await readConclusionsStore(analysisId);
            const kept = store.conclusions.filter((c) => c.mode !== modeFilter);
            if (kept.length === store.conclusions.length) return res.json({ code: 'ok' });
            if (kept.length === 0) {
                await fs.unlink(aiConclusionPath(analysisId)).catch(() => {});
            } else {
                await fs.writeFile(aiConclusionPath(analysisId), JSON.stringify({ conclusions: kept }, null, 2), 'utf-8');
            }
            return res.json({ code: 'ok' });
        }
        await fs.unlink(aiConclusionPath(analysisId));
    } catch (err) {
        if (err.code !== 'ENOENT') {
            return res.status(500).json({ code: 'delete_failed', message: err.message });
        }
    }
    res.json({ code: 'ok' });
});

// ===== Async analysis jobs =====
// An analysis is minutes of LLM roundtrips, but the gateway fronting this
// server closes proxied requests it has heard nothing on for 60s. Holding the
// POST open therefore hands the browser a 504 while the analysis keeps running
// and eventually succeeds — the conclusion lands on disk with no way to show
// it. So POST validates, starts the job, and returns 202; the UI polls
// /api/ai/analyze/job.
//
// Keyed by (analysisId, mode) rather than a generated job id so a page reload
// can resume polling, and so a second click cannot spawn a rival openclaw
// child that races the first one's write to ai_conclusion.json.
const aiJobs = new Map();
const AI_JOB_TTL_MS = 60 * 60 * 1000;

function aiJobKey(analysisId, mode) { return `${analysisId}:${mode}`; }

function pruneAiJobs(now) {
    for (const [key, job] of aiJobs) {
        if (job.status !== 'running' && now - job.endedAt > AI_JOB_TTL_MS) aiJobs.delete(key);
    }
}

function startAiJob({ analysisId, mode, run }) {
    const key = aiJobKey(analysisId, mode);
    const running = aiJobs.get(key);
    if (running && running.status === 'running') return running;
    const job = {
        analysisId, mode, status: 'running', startedAt: Date.now(), endedAt: 0,
        model: '', content: '', message: '', errorCode: '',
    };
    aiJobs.set(key, job);
    run()
        .then(async ({ content, model, label }) => {
            await saveAiConclusion(analysisId, { mode, model, label, content });
            job.content = content;
            job.model = model;
            job.status = 'succeeded';
        })
        .catch((err) => {
            console.error(`[ai/analyze][${mode}] job failed for ${analysisId}: ${err.message}`);
            job.status = 'failed';
            job.message = err.message;
            job.errorCode = err.code || 'upstream_error';
        })
        .finally(() => {
            job.endedAt = Date.now();
            pruneAiJobs(job.endedAt);
        });
    return job;
}

function acceptedJobBody(job) {
    return { code: 'accepted', data: { mode: job.mode, status: job.status, startedAt: job.startedAt } };
}

app.get('/api/ai/analyze/job', auth.requireAuth, (req, res) => {
    const job = aiJobs.get(aiJobKey(req.query.analysisId, String(req.query.mode || '')));
    if (!job) return res.json({ code: 'ok', data: null });
    return res.json({
        code: 'ok',
        data: {
            mode: job.mode, status: job.status, startedAt: job.startedAt, endedAt: job.endedAt,
            model: job.model, content: job.content, message: job.message, errorCode: job.errorCode,
        },
    });
});

app.post('/api/ai/analyze', auth.requireAuth, async (req, res) => {
    const { mode, question, sessionDigest, llm, analysisId } = req.body || {};
    // Both modes key their job and their on-disk conclusion by analysisId, so
    // it must be a real task id before anything is started.
    if (!analysisId || !isValidTaskId(analysisId)) {
        return res.status(400).json({ code: 'bad_request', message: '分析需要有效的 analysisId' });
    }
    try {
        if (mode === 'openclaw') {
            // Precedence: req.body.llm (one-off override from the modal, legacy behavior)
            //   > server-persisted llm-config.json (global config saved from the settings page)
            //   > environment variables (QWEN_API_KEY / AIPROF_OPENCLAW_MODEL)
            const persisted = await loadLlmConfig();
            const modelRef = llm?.model || persisted.model || process.env.AIPROF_OPENCLAW_MODEL || 'qwen3.5-plus';
            const apiKey   = llm?.apiKey || persisted.apiKey || process.env.QWEN_API_KEY || '';
            if (!apiKey) {
                // Fail fast in a Chinese error the UI can render, before spawning
                // the openclaw child — its ProviderAuthError trace is 1KB+ of noise.
                return res.status(400).json({
                    code: 'missing_api_key',
                    message: '未提供 API Key：请到「设置 · LLM 配置」页面保存 API Key，或在 aiprof-server 上设置环境变量 QWEN_API_KEY / DASHSCOPE_API_KEY 后重启容器。',
                });
            }
            const job = startAiJob({
                analysisId,
                mode: 'openclaw',
                run: async () => {
                    const result = await runOpenclawAgent({
                        systemPrompt: AIPROF_SYSTEM_PROMPT,
                        userPrompt: buildAgentPrompt(question, sessionDigest),
                        model: modelRef,
                        apiKey,
                    });
                    return {
                        content: result.text,
                        model: modelRef,
                        label: `本地 OpenClaw 分析结果 · ${modelRef}`,
                    };
                },
            });
            return res.status(202).json(acceptedJobBody(job));
        }

        if (mode === 'local') {
            // Local agent analysis: feed the chrome-tracing file from <RESULT_DIR>/<analysisId>/
            // directly to local-profiling-agent (ingest → tools → planner → synthesizer).
            const persisted = await loadLlmConfig();
            const modelRef = llm?.model || persisted.model || process.env.AIPROF_OPENCLAW_MODEL || 'qwen3.5-plus';
            const apiKey   = llm?.apiKey || persisted.apiKey || process.env.QWEN_API_KEY || '';
            const baseUrl  = (llm?.baseUrl || persisted.baseUrl || 'https://dashscope.aliyuncs.com/compatible-mode/v1').trim();
            if (!apiKey) {
                return res.status(400).json({
                    code: 'missing_api_key',
                    message: '未提供 API Key：请到「LLM 全局配置」页面保存 API Key，或设置 QWEN_API_KEY 后重启容器。',
                });
            }
            const resultDir = path.join(RESULT_DIR, analysisId);
            const traceFile = findTraceFile(resultDir);
            if (!traceFile) {
                return res.status(404).json({
                    code: 'trace_not_found',
                    message: `未在报告目录找到 chrome-tracing 文件（AIProf_*.json）：${resultDir}。请确认采集是否成功产出 trace。`,
                });
            }
            const job = startAiJob({
                analysisId,
                mode: 'local',
                run: async () => {
                    const { runLocalProfilingAgent, createOpenAICompatibleBackend } = require('../../local-profiling-agent');
                    const backend = createOpenAICompatibleBackend({ baseUrl, apiKey, model: modelRef });
                    const result = await runLocalProfilingAgent({
                        traceFilePath: traceFile,
                        question,
                        llm: backend,
                        options: { onProgress: (p) => { if (p.msg) console.log(`[ai/analyze][local][${p.phase}] ${p.msg}`); } },
                    });
                    return {
                        content: result.markdown,
                        model: modelRef,
                        label: `本地 Agent 分析结果 · ${modelRef}`,
                    };
                },
            });
            return res.status(202).json(acceptedJobBody(job));
        }

        return res.status(400).json({ code: 'bad_mode', message: `不支持的分析模式：${mode || '(空)'}。可用 openclaw / local。阿里云 Agent 请前往 ${process.env.ALIYUN_AGENT_URL || 'https://soma.openanolis.cn'} 体验。` });
    } catch (err) {
        console.error('[ai/analyze] error:', err.message);
        const status = err.status || 502;
        const body = { code: err.code || 'upstream_error', message: err.message };
        res.status(status).json(body);
    }
});


// ============================================================================
// OpenClaw local agent — a third LLM path: build the report into a message file, then
// spawn a one-shot `openclaw agent --local` child. The key only lives in the child env; nothing is persisted.
// ============================================================================
const OPENCLAW_PREFIX = path.join(PERSISTENCE_DIR, 'openclaw');
const OPENCLAW_BIN_DIR = path.join(OPENCLAW_PREFIX, 'bin');
const OPENCLAW_SENTINEL = path.join(OPENCLAW_PREFIX, '.installed');
const OPENCLAW_INSTALL_LOG = path.join(OPENCLAW_PREFIX, 'install.log');

// Resolve `openclaw` binary: prefer our private prefix (installed via
// /api/ai/openclaw/install), then fall back to PATH (baked-in via Dockerfile
// WITH_OPENCLAW=1 or a host-side `npm i -g openclaw`).
function openclawPath() {
    const local = path.join(OPENCLAW_BIN_DIR, 'openclaw');
    if (fsSync.existsSync(local)) return local;
    return 'openclaw';
}

function runOpenclawCli(args, extraEnv = {}, opts = {}) {
    return new Promise((resolve, reject) => {
        const bin = openclawPath();
        const env = { ...process.env, ...extraEnv };
        env.PATH = `${OPENCLAW_BIN_DIR}:${env.PATH || ''}`;
        env.NPM_CONFIG_PREFIX = OPENCLAW_PREFIX;
        const child = spawn(bin, args, { env, stdio: ['ignore', 'pipe', 'pipe'] });
        let stdout = '', stderr = '';
        child.stdout.on('data', (d) => { stdout += d.toString(); if (opts.onTail) opts.onTail(d.toString()); });
        child.stderr.on('data', (d) => { stderr += d.toString(); if (opts.onTail) opts.onTail(d.toString()); });
        child.on('error', reject);
        child.on('close', (code) => {
            if (code === 0) resolve({ stdout, stderr });
            else reject(Object.assign(new Error(stderr.slice(-2000) || `openclaw exited ${code}`), { code: 'openclaw_failed', exitCode: code, stdout, stderr }));
        });
    });
}

async function detectOpenclaw() {
    const bin = openclawPath();
    let version = '';
    try {
        const { stdout } = await runOpenclawCli(['--version']);
        version = stdout.trim().split(/\s+/).pop() || stdout.trim();
    } catch {
        return { installed: false, qwenReady: false, binary: bin };
    }
    let qwenReady = false;
    try {
        const { stdout } = await runOpenclawCli(['plugins', 'list']);
        qwenReady = /@openclaw\/qwen-provider/.test(stdout);
    } catch {
        // `plugins list` refuses to run when openclaw.json is invalid (and may
        // not exist in every release) — fall back to a file-layout probe.
        // openclaw installs plugins into its own npm home, NOT under our
        // --prefix, so probe there; the old prefix-relative path never matched
        // and silently reported "installed but missing Qwen provider".
        const home = process.env.HOME || '/root';
        qwenReady = fsSync.existsSync(path.join(home, '.openclaw', 'npm', 'projects'))
            && fsSync.readdirSync(path.join(home, '.openclaw', 'npm', 'projects'))
                .some((d) => d.startsWith('openclaw-qwen-provider'));
    }
    return { installed: true, qwenReady, binary: bin, version };
}

// detectOpenclaw() spawns `openclaw` twice; each cold spawn takes ~3s (Node CLI
// loading 300 npm modules). The UI polls /status every time the modal opens —
// cache the detect result so subsequent opens return instantly. Cache is
// invalidated on install completion.
let _openclawDetectCache = { at: 0, promise: null, value: null };
const OPENCLAW_DETECT_TTL_MS = Number(process.env.AIPROF_OPENCLAW_DETECT_TTL_MS) || 60_000;
function invalidateOpenclawDetectCache() { _openclawDetectCache = { at: 0, promise: null, value: null }; }
async function detectOpenclawCached() {
    const now = Date.now();
    if (_openclawDetectCache.value && (now - _openclawDetectCache.at) < OPENCLAW_DETECT_TTL_MS) {
        return _openclawDetectCache.value;
    }
    if (_openclawDetectCache.promise) return _openclawDetectCache.promise;
    _openclawDetectCache.promise = (async () => {
        try {
            const v = await detectOpenclaw();
            _openclawDetectCache = { at: Date.now(), promise: null, value: v };
            return v;
        } catch (e) {
            _openclawDetectCache = { at: 0, promise: null, value: null };
            throw e;
        }
    })();
    return _openclawDetectCache.promise;
}

async function runOpenclawAgent({ systemPrompt, userPrompt, model, apiKey }) {
    const detect = await detectOpenclawCached();
    if (!detect.installed) {
        const err = new Error('OpenClaw 未安装。请先在页面上点「安装 OpenClaw」或在宿主机执行 npm i -g openclaw && openclaw plugins install @openclaw/qwen-provider。');
        err.code = 'openclaw_missing'; err.status = 503;
        throw err;
    }
    // Only enforce the qwen-plugin check for qwen model refs; users may pass
    // e.g. openai/gpt-4o-mini once they've configured that provider elsewhere.
    if (/(^|\/)qwen[^/]*$/.test(model) && !detect.qwenReady) {
        const err = new Error('OpenClaw 已装但缺 Qwen provider。请点「安装 Qwen 插件」或执行 openclaw plugins install @openclaw/qwen-provider。');
        err.code = 'openclaw_qwen_missing'; err.status = 503;
        throw err;
    }

    const tmpPath = path.join(UPLOAD_TMP, `openclaw-${crypto.randomUUID()}.txt`);
    const body = `${systemPrompt}\n\n---\n\n${userPrompt}`;
    // openclaw --message-file caps at 4 MiB; buildAgentPrompt already truncates
    // the digest to 20k chars so this is defence-in-depth.
    const capped = body.length > 3 * 1024 * 1024 ? body.slice(0, 3 * 1024 * 1024) + '\n...(truncated)' : body;
    await fs.writeFile(tmpPath, capped, 'utf-8');
    // openclaw model refs are <provider>/<model>. Without a slash the CLI defaults
    // to `openai/<model>` and returns "Unknown model: openai/qwen3.5-plus". Auto-
    // prefix `qwen/` for bare Qwen model IDs so the DashScope + qwen-provider path
    // works without users needing to know the ref format.
    const resolvedModel = /\//.test(model) ? model : (/^qwen/i.test(model) ? `qwen/${model}` : model);
    try {
        const timeout = Number(process.env.AIPROF_OPENCLAW_TIMEOUT_S) || 300;
        // --agent main: pin to the default agent; without it openclaw errors
        //   "Pass --to <E.164>, --session-key, --session-id, or --agent".
        const args = ['agent', '--local', '--agent', 'main', '--model', resolvedModel, '--message-file', tmpPath, '--json', '--timeout', String(timeout)];
        const extraEnv = {};
        if (apiKey) {
            // Cover the three env names the Qwen provider accepts.
            extraEnv.QWEN_API_KEY = apiKey;
            extraEnv.DASHSCOPE_API_KEY = apiKey;
            extraEnv.MODELSTUDIO_API_KEY = apiKey;
        }
        const { stdout } = await runOpenclawCli(args, extraEnv);
        let parsed;
        try { parsed = JSON.parse(stdout); } catch {
            return { text: stdout.trim() };
        }
        // openclaw v2026.7 --json output shape: { payloads: [{text}], meta: { finalAssistantVisibleText, ... } }
        // Older versions attached payloads under result.*; check both locations.
        const payloads = parsed?.payloads || parsed?.result?.payloads || [];
        const text = payloads.map((p) => p?.text || '').filter(Boolean).join('\n\n')
            || parsed?.meta?.finalAssistantVisibleText
            || parsed?.meta?.finalAssistantRawText
            || parsed?.summary
            || JSON.stringify(parsed, null, 2);
        return { text };
    } finally {
        fs.unlink(tmpPath).catch(() => {});
    }
}

const openclawInstall = { status: 'idle', startedAt: 0, endedAt: 0, tail: [], error: '' };
function pushInstallTail(chunk) {
    for (const line of chunk.split(/\r?\n/)) {
        if (line) openclawInstall.tail.push(line);
    }
    if (openclawInstall.tail.length > 200) openclawInstall.tail = openclawInstall.tail.slice(-200);
}

app.get('/api/ai/openclaw/status', async (_req, res) => {
    try {
        const d = await detectOpenclawCached();
        res.json({
            code: 'ok',
            install: {
                status: openclawInstall.status,
                startedAt: openclawInstall.startedAt || null,
                endedAt: openclawInstall.endedAt || null,
                tail: openclawInstall.tail.slice(-40),
                error: openclawInstall.error || '',
            },
            openclaw: d,
            defaults: {
                model: process.env.AIPROF_OPENCLAW_MODEL || 'qwen3.5-plus',
                hasServerQwenKey: Boolean(process.env.QWEN_API_KEY),
            },
        });
    } catch (err) {
        res.status(500).json({ code: 'error', message: err.message });
    }
});

app.post('/api/ai/openclaw/install', async (_req, res) => {
    if (openclawInstall.status === 'running') {
        return res.status(409).json({ code: 'busy', message: '安装正在进行中，请稍候。' });
    }
    openclawInstall.status = 'running';
    openclawInstall.startedAt = Date.now();
    openclawInstall.endedAt = 0;
    openclawInstall.tail = [];
    openclawInstall.error = '';
    res.json({ code: 'ok', message: '开始安装，轮询 /api/ai/openclaw/status 查看进度。' });

    (async () => {
        try {
            fsSync.mkdirSync(OPENCLAW_PREFIX, { recursive: true });
            pushInstallTail(`[install] prefix=${OPENCLAW_PREFIX}`);
            // openclaw is only published to registry.npmjs.org; on mirrors commonly used in China (npmmirror),
            // the same name is a placeholder shell that resolves to a 0.0.1 no-bin build,
            // which makes subsequent `openclaw plugins ...` calls fail with ENOENT.
            const OPENCLAW_REGISTRY = process.env.AIPROF_OPENCLAW_REGISTRY || 'https://registry.npmjs.org';
            pushInstallTail(`[install] registry=${OPENCLAW_REGISTRY}`);
            await new Promise((resolve, reject) => {
                const child = spawn('npm', ['install', '-g', '--prefix', OPENCLAW_PREFIX, `--registry=${OPENCLAW_REGISTRY}`, 'openclaw'], {
                    env: { ...process.env, NPM_CONFIG_PREFIX: OPENCLAW_PREFIX },
                    stdio: ['ignore', 'pipe', 'pipe'],
                });
                child.stdout.on('data', (d) => pushInstallTail(d.toString()));
                child.stderr.on('data', (d) => pushInstallTail(d.toString()));
                child.on('error', reject);
                child.on('close', (code) => code === 0 ? resolve() : reject(new Error(`npm install openclaw exit ${code}`)));
            });
            // Heal the config before touching plugins. Every non-allowlisted
            // subcommand (`plugins install` included) refuses to run against an
            // invalid openclaw.json, and openclaw drops config keys across
            // releases — 2026.9 removed plugins.bundledDiscovery, which an
            // older AIProf wrote here. Without this, a second install attempt
            // fails with "config is invalid" and the plugin never lands.
            pushInstallTail('[install] openclaw doctor --fix ...');
            try {
                await runOpenclawCli(['doctor', '--fix'], {}, { onTail: (chunk) => pushInstallTail(chunk) });
            } catch (e) {
                pushInstallTail(`[install] warn: doctor --fix failed: ${e.message}`);
            }
            pushInstallTail('[install] installing @openclaw/qwen-provider ...');
            await runOpenclawCli(['plugins', 'install', '@openclaw/qwen-provider'], {}, {
                onTail: (chunk) => pushInstallTail(chunk),
            });
            // openclaw's plugins.allow being empty means it won't auto-load discovered providers,
            // so after installing the plugin we still have to add qwen to allow + entries explicitly.
            // Edit ~/.openclaw/openclaw.json directly rather than relying on interactive `openclaw configure`.
            try {
                const home = process.env.HOME || '/root';
                const cfgPath = path.join(home, '.openclaw', 'openclaw.json');
                let cfg = {};
                try { cfg = JSON.parse(fsSync.readFileSync(cfgPath, 'utf-8')); } catch {}
                cfg.plugins = cfg.plugins || {};
                const allow = new Set(cfg.plugins.allow || []);
                allow.add('qwen');
                cfg.plugins.allow = Array.from(allow);
                cfg.plugins.entries = cfg.plugins.entries || {};
                cfg.plugins.entries.qwen = { ...(cfg.plugins.entries.qwen || {}), enabled: true };
                cfg.models = cfg.models || {};
                cfg.models.providers = cfg.models.providers || {};
                if (!cfg.models.providers.qwen) {
                    cfg.models.providers.qwen = {
                        baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
                        api: 'openai-completions',
                        apiKey: { source: 'env', provider: 'default', id: 'QWEN_API_KEY' },
                    };
                }
                fsSync.mkdirSync(path.dirname(cfgPath), { recursive: true });
                fsSync.writeFileSync(cfgPath, JSON.stringify(cfg, null, 2));
                pushInstallTail('[install] wrote plugins.allow=[qwen], plugins.entries.qwen.enabled, models.providers.qwen');
            } catch (e) {
                pushInstallTail(`[install] warn: openclaw.json patch failed: ${e.message}`);
            }
            fsSync.writeFileSync(OPENCLAW_SENTINEL, new Date().toISOString());
            openclawInstall.status = 'ok';
            openclawInstall.endedAt = Date.now();
            invalidateOpenclawDetectCache();
            pushInstallTail('[install] done.');
            try { fsSync.writeFileSync(OPENCLAW_INSTALL_LOG, openclawInstall.tail.join('\n')); } catch {}
        } catch (err) {
            openclawInstall.status = 'error';
            openclawInstall.endedAt = Date.now();
            openclawInstall.error = err.message || String(err);
            invalidateOpenclawDetectCache();
            pushInstallTail(`[install] FAILED: ${openclawInstall.error}`);
            try { fsSync.writeFileSync(OPENCLAW_INSTALL_LOG, openclawInstall.tail.join('\n')); } catch {}
        }
    })();
});

// Fail fast on a misconfigured auth mode: a missing provider must crash at
// boot, not turn every request into a 401.
try {
    auth.assertConfigured();
} catch (err) {
    console.error(`[auth] configuration error: ${err.message}`);
    process.exit(1);
}

// Mount the app router at BASE_PATH ("" → "/"). Doing this after all routes
// are registered on the router is fine; Express walks the mount table on
// each request.
rootApp.use(BASE_PATH || '/', app);

server.listen(PORT, '0.0.0.0', () => {
    console.log(`AIProf dashboard server on :${PORT}`);
    console.log(`  AUTH_MODE       = ${auth.AUTH_MODE}`);
    console.log(`  BASE_PATH       = ${BASE_PATH || '(root)'}`);
    console.log(`  RESULT_DIR      = ${RESULT_DIR}`);
    console.log(`  PERSISTENCE_DIR = ${PERSISTENCE_DIR}`);
    console.log(`  UPLOAD_TMP      = ${UPLOAD_TMP}`);
    console.log(`  ext analyzer    = ${(process.env.AIPROF_EXTERNAL_ANALYZER_URL || '').trim() || '(未配置) 「云端 Trace 分析」按钮不显示'}`);
    console.log(`  ext import mode = ${process.env.AIPROF_EXTERNAL_IMPORT === 'true' ? 'pull (同机 server-to-server 拷贝)' : 'download (浏览器下载再上传)'}`);
    console.log(`  internal token  = ${(process.env.AIPROF_INTERNAL_TOKEN || '').trim() ? 'configured (仅放行 GET aiAnalysis/trace)' : '(未配置) 拉取接口需正常用户登录'}`);
    // Prewarm openclaw detection so the first modal-open doesn't pay the
    // ~3.4 s cold `openclaw --version` cost.
    detectOpenclawCached()
        .then((d) => console.log(`  openclaw        = ${d.installed ? `${d.version} (qwen=${d.qwenReady})` : 'not installed'}`))
        .catch((e) => console.log(`  openclaw        = detect failed: ${e.message}`));
});
