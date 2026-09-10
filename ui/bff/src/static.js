// SPDX-License-Identifier: Apache-2.0
//
// Small static-file server for the built webapp (Vite output).
// SPA fallback: any GET that doesn't match a real file falls back to
// index.html so client-side routing keeps working on reload.

'use strict';

const fs = require('fs');
const path = require('path');

const CT = {
    '.html': 'text/html; charset=utf-8',
    '.js':   'text/javascript; charset=utf-8',
    '.mjs':  'text/javascript; charset=utf-8',
    '.css':  'text/css; charset=utf-8',
    '.json': 'application/json; charset=utf-8',
    '.map':  'application/json; charset=utf-8',
    '.svg':  'image/svg+xml',
    '.png':  'image/png',
    '.jpg':  'image/jpeg',
    '.jpeg': 'image/jpeg',
    '.ico':  'image/x-icon',
    '.woff': 'font/woff',
    '.woff2':'font/woff2',
    '.ttf':  'font/ttf',
    '.txt':  'text/plain; charset=utf-8',
};

function serve(root, req, res) {
    const p = decodeURIComponent(req.url.split('?')[0]);
    let target = path.join(root, p === '/' ? '/index.html' : p);
    if (!target.startsWith(path.resolve(root))) {
        res.writeHead(400); return res.end('bad path');
    }
    fs.stat(target, (err, stat) => {
        if (err || !stat.isFile()) {
            target = path.join(root, 'index.html');
            fs.readFile(target, (e, data) => {
                if (e) { res.writeHead(404); return res.end('not found'); }
                res.writeHead(200, { 'content-type': CT['.html'] });
                res.end(data);
            });
            return;
        }
        fs.readFile(target, (e, data) => {
            if (e) { res.writeHead(500); return res.end('read error'); }
            const ct = CT[path.extname(target).toLowerCase()] || 'application/octet-stream';
            res.writeHead(200, { 'content-type': ct });
            res.end(data);
        });
    });
}

module.exports = { serve };
