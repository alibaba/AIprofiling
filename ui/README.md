# AIProf UI

Two parts:

- `frontend/dist/` — a single-page static app. **No build step required.**
  Uses `d3-flame-graph` for the flame chart, loaded from a CDN. Suitable
  for both `python -m http.server` and the containerised BFF.
- `bff/` — a small Node.js proxy (~90 LoC of stdlib) that fronts the
  three FastAPI services under a single origin.

## Local dev

```bash
# Terminal 1: the three FastAPI services
cd server && ./run-all.sh

# Terminal 2: the BFF (also serves the static UI)
cd ui/bff && npm start
```

Then browse http://localhost:9100/.

## Re-building the frontend (optional)

The current `dist/` is hand-written vanilla JS to keep the OSS setup
approachable. Teams wanting React/Vite can drop a modern project into
`frontend/src/` and point Vite's `build.outDir` at `frontend/dist/` —
the BFF only cares about static files.
