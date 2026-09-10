# Frontend source layout (reserved)

The current AIProf UI ships as hand-written vanilla JS in `../dist/` to
avoid forcing a node/npm toolchain on OSS users. This directory is a
placeholder for a future React/Vite implementation.

Suggested structure:

```
src/
├── main.tsx
├── router.tsx
├── pages/
│   ├── SessionList.tsx
│   ├── SessionDetail.tsx
│   └── Report.tsx
└── components/
    ├── Flamegraph.tsx
    └── Filters.tsx
```

Point `vite.config.ts`'s `build.outDir` at `../dist/`.
