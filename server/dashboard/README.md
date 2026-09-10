# AIProf Dashboard Server

WS control plane + REST upload/query.

## Layout

- `dashboardServer.js` — express + ws, listens on `:7000`.
- `analysis.py` + `gmem.py` — post-upload analysis pipeline.
- `build.sh` — `pyinstaller` bundles them into a single `analysis_summary` binary.

## Env

| var                | default                                                      | notes                                |
| ------------------ | ------------------------------------------------------------ | ------------------------------------ |
| `PORT`             | `7000`                                                       | HTTP + WS port                       |
| `RESULT_DIR`       | `<repo>/ui/webapp/public/resource/ai_observable/result`      | where finalized results land         |
| `PERSISTENCE_DIR`  | `<this dir>/.profiling-data`                                 | task metadata + client registry      |
| `UPLOAD_TMP`       | `<this dir>/.temp-uploads`                                   | multipart tar.gz staging area        |

## Run

```bash
cd AIProf/server/dashboard
npm ci
node dashboardServer.js
```

## Build the analysis binary (optional)

```bash
cd AIProf/server/dashboard
bash build.sh   # requires pyinstaller in PATH
```

If `analysis_summary` is absent, uploads still succeed; only post-processing is
skipped (logged as a warning).
