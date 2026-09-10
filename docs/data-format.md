# AIProf Session Data Format

A "session" is one profiling run. On disk it is a single directory. On the
wire it is a `tar` (optionally `zstd`-compressed) of that directory.

## Directory layout

```
<session_id>/
├── meta.json               # required
├── folded.txt              # optional — Brendan Gregg folded stacks
├── timeline.json           # optional — Chrome Trace Format
├── flamegraph.svg          # optional — pre-rendered SVG
├── cupti/                  # optional — CUPTI dumps (kind == py-cuda / cupti)
│   └── kernels.jsonl
└── extra/                  # optional — plugin-specific extras
```

## `meta.json`

```json
{
    "session_id":  "20260716-153012-abc123",
    "schema_version": 1,
    "host":        "gpu-worker-07",
    "workload":    "resnet50-train",
    "kind":        "cpu-oncpu",         // cpu-oncpu | cpu-offcpu | cupti | py-cuda | mock
    "pid":         12345,
    "start_ts":    1731764412,
    "end_ts":      1731764442,
    "duration_s":  30,
    "languages":   ["python", "native"],
    "agent_ver":   "0.1.0",
    "notes":       ""                    // free text
}
```

Fields that MUST be present: `session_id`, `schema_version`, `host`, `kind`,
`start_ts`, `end_ts`.

## Wire format

```
POST /api/v1/collector/upload
Content-Type: multipart/form-data

meta   -> meta.json         (application/json)
blob   -> session.tar.zst   (application/octet-stream)
```

The collector verifies `meta.session_id` matches the directory name inside the
tarball, then stores the tarball as `blobs/<session_id>.tar.zst` and inserts a
row into `sessions`.

## `folded.txt`

```
python;train.py:main;train.py:step;torch/_ops.py:__call__ 42
python;train.py:main;train.py:step;torch/_ops.py:__call__;CUDA:sm90_gemm 17
```

Semicolon-separated stacks with a trailing count. Compatible with
`inferno-flamegraph`, `flamegraph.pl`, and Pyroscope.

## `timeline.json`

Chrome Trace Format ("`traceEvents`" array). Compatible with `chrome://tracing`
and Perfetto.
