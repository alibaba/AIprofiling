# AIProf server (FastAPI micro-services)

Three small FastAPI apps sharing common code from `common/`:

| Service | Port | Job |
|---|---|---|
| `collector` | 7101 | receives session tarballs, writes blobs, indexes meta |
| `query`     | 7102 | REST for the UI: list / detail / folded / diff |
| `report`    | 7103 | proxies to `agent-ai` gateway, caches AI reports |

Storage:

- Metadata: **SQLite** by default (single file, no dep), overridable to MySQL
  via `AIPROF_DB_URL`.
- Blobs: local filesystem under `AIPROF_BLOB_DIR` (default `./data/blobs`).
  MinIO/S3 hooks are behind an interface in `common/storage.py`.

## Run locally

```bash
pip install -r requirements.txt

uvicorn services.collector.app.main:app --port 7101 &
uvicorn services.query.app.main:app     --port 7102 &
uvicorn services.report.app.main:app    --port 7103 &
```

## Run via Docker

See `deploy/docker/Dockerfile.server` for the multi-service image and
`deploy/docker/docker-compose.yml` for wiring.
