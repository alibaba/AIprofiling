# SPDX-License-Identifier: Apache-2.0
"""SQLite-backed metadata store for AIProf sessions.

Deliberately uses stdlib `sqlite3` instead of SQLAlchemy: keeps the
open-source dependency graph small and avoids ORM tax for a schema this
narrow. Swap in SQLAlchemy if you need MySQL/Postgres — the query
surface is intentionally tiny (5 methods).
"""

from __future__ import annotations

import json
import os
import sqlite3
import threading
import time
from contextlib import contextmanager
from typing import Any, Dict, Iterable, List, Optional


_SCHEMA = """
CREATE TABLE IF NOT EXISTS sessions (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id     TEXT UNIQUE NOT NULL,
    host           TEXT NOT NULL,
    workload       TEXT,
    kind           TEXT NOT NULL,
    pid            INTEGER,
    start_ts       INTEGER NOT NULL,
    end_ts         INTEGER NOT NULL,
    duration_s     INTEGER NOT NULL,
    languages      TEXT NOT NULL,        -- JSON array
    agent_ver      TEXT NOT NULL,
    blob_key       TEXT NOT NULL,        -- relative path in blob store
    created_ts     INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sessions_host    ON sessions(host);
CREATE INDEX IF NOT EXISTS idx_sessions_workload ON sessions(workload);
CREATE INDEX IF NOT EXISTS idx_sessions_kind    ON sessions(kind);
CREATE INDEX IF NOT EXISTS idx_sessions_start   ON sessions(start_ts);

CREATE TABLE IF NOT EXISTS reports (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id   TEXT NOT NULL,
    mode         TEXT NOT NULL,          -- "ai" | "manual"
    content      TEXT NOT NULL,
    created_ts   INTEGER NOT NULL,
    UNIQUE(session_id, mode)
);
"""


class Store:
    def __init__(self, db_url: str) -> None:
        if not db_url.startswith("sqlite:///"):
            raise ValueError("this reference store only handles sqlite:/// URLs")
        path = db_url[len("sqlite:///"):]
        os.makedirs(os.path.dirname(os.path.abspath(path)) or ".", exist_ok=True)
        self._path = path
        self._lock = threading.Lock()
        # `check_same_thread=False` is safe under `self._lock` (one writer at a time).
        self._conn = sqlite3.connect(path, check_same_thread=False)
        self._conn.row_factory = sqlite3.Row
        self._conn.executescript(_SCHEMA)
        self._conn.commit()

    @contextmanager
    def _cursor(self) -> Iterable[sqlite3.Cursor]:
        with self._lock:
            cur = self._conn.cursor()
            try:
                yield cur
                self._conn.commit()
            finally:
                cur.close()

    def insert_session(self, meta: Dict[str, Any], blob_key: str) -> int:
        with self._cursor() as cur:
            cur.execute(
                """INSERT OR REPLACE INTO sessions
                   (session_id, host, workload, kind, pid, start_ts, end_ts,
                    duration_s, languages, agent_ver, blob_key, created_ts)
                   VALUES (?,?,?,?,?,?,?,?,?,?,?,?)""",
                (
                    meta["session_id"],
                    meta["host"],
                    meta.get("workload"),
                    meta["kind"],
                    meta.get("pid"),
                    int(meta["start_ts"]),
                    int(meta["end_ts"]),
                    int(meta.get("duration_s", 0)),
                    json.dumps(meta.get("languages", [])),
                    meta.get("agent_ver", ""),
                    blob_key,
                    int(time.time()),
                ),
            )
            return cur.lastrowid or 0

    def list_sessions(
        self,
        host: Optional[str] = None,
        workload: Optional[str] = None,
        kind: Optional[str] = None,
        limit: int = 50,
    ) -> List[Dict[str, Any]]:
        clauses: List[str] = []
        params: List[Any] = []
        if host:
            clauses.append("host = ?"); params.append(host)
        if workload:
            clauses.append("workload = ?"); params.append(workload)
        if kind:
            clauses.append("kind = ?"); params.append(kind)
        where = ("WHERE " + " AND ".join(clauses)) if clauses else ""
        params.append(limit)
        with self._cursor() as cur:
            cur.execute(
                f"SELECT * FROM sessions {where} ORDER BY start_ts DESC LIMIT ?",
                params,
            )
            return [self._row_to_session(r) for r in cur.fetchall()]

    def get_session(self, session_id: str) -> Optional[Dict[str, Any]]:
        with self._cursor() as cur:
            cur.execute("SELECT * FROM sessions WHERE session_id = ?", (session_id,))
            row = cur.fetchone()
            return self._row_to_session(row) if row else None

    def upsert_report(self, session_id: str, mode: str, content: str) -> None:
        with self._cursor() as cur:
            cur.execute(
                """INSERT OR REPLACE INTO reports
                   (session_id, mode, content, created_ts)
                   VALUES (?, ?, ?, ?)""",
                (session_id, mode, content, int(time.time())),
            )

    def get_report(self, session_id: str, mode: str) -> Optional[Dict[str, Any]]:
        with self._cursor() as cur:
            cur.execute(
                "SELECT * FROM reports WHERE session_id = ? AND mode = ?",
                (session_id, mode),
            )
            r = cur.fetchone()
            return dict(r) if r else None

    @staticmethod
    def _row_to_session(r: sqlite3.Row) -> Dict[str, Any]:
        d = dict(r)
        try:
            d["languages"] = json.loads(d.get("languages") or "[]")
        except json.JSONDecodeError:
            d["languages"] = []
        return d
