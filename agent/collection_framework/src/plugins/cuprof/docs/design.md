# Design notes

## Goal

Answer one question — *what ran on the GPU, in what order, for how long* — with
the smallest amount of machinery that can do it correctly.

## Shape

Two artifacts:

- `libcuprof.so` — loaded into the target process by the CUDA driver, subscribes
  to CUPTI activity records, writes JSON on the way out.
- `cuprof` — a launcher that sets `CUDA_INJECTION64_PATH` and `CUPROF_*`, then
  `execvp`s the target. It holds no state and exits into the target process.

## Decisions

**Injection via `CUDA_INJECTION64_PATH`.** The CUDA driver calls
`InitializeInjection()` in the named library during CUDA initialization, before
any user kernel runs. That symbol name is fixed by the driver, so it stays as a
one-line shim over `cuprof_start()`, which is the real entry point and the one
embedders should call. This is a documented driver feature, so there is no
`ptrace`, no `LD_PRELOAD` constructor ordering, and no separate injector
binary. The variable must be set before the process starts — hence the CLI is
launch-only. For already-running processes, cuprof deliberately stays on the
passive side of the line: an external orchestrator performs the remote
`dlopen` and calls `cuprof_start()` itself, following the contract in
`embedding.md`. Owning an injector would mean owning ptrace, ELF layout and
libc-version quirks — the single largest complexity source in tools of this
kind.

**Environment variables as the primary configuration channel, a `KEY=VALUE`
file as the embedded fallback.** Config is read once inside `cuprof_start()`.
An injected library cannot receive environment variables from its
orchestrator, so a plain-text file replaces them — same keys, same semantics,
no reconfiguration after start.

**Buffer-and-write-at-exit.** Activity records accumulate in memory during a
window and are serialized when the window stops (duration expiry, or
`atexit`). This trades memory for simplicity: no streaming writer, no
partial-file recovery, no writer thread. For a long job, use `--duration` to
bound both the collection window and the memory; the event buffer is cleared
after each window.

**Explicit `cuptiActivityFlushAll` on stop.** Records sit in CUPTI-internal
buffers until CUPTI decides to hand them over. Without a forced flush at
shutdown, the tail of the timeline is silently missing.

**System CUPTI, linked at build time.** The library links `-lcupti` from the
local CUDA Toolkit rather than vendoring or `dlopen`-ing versioned copies. This
keeps the repository free of redistributable-binary questions, and makes the
supported CUDA range a property of the build environment rather than of a
lookup table. The trade-off is that a build is tied to the toolkit it was built
against.

**Aggressive symbol hiding.** `export.map` exports only `cuprof_start` and
the `InitializeInjection` shim. Everything else is `local:`. The library is
loaded into someone else's process, which may already have its own copies of
the C++ runtime and third-party libraries; leaking symbols invites ODR
violations that surface as crashes far from the cause.

**Three activity kinds.** Kernel, memcpy, memset. Each additional kind adds
record-struct versioning to track and event volume to survive. Unified-memory
counters and per-instruction sampling in particular produce event rates that
overwhelm both the buffer path and any downstream viewer.

**Stream as the timeline lane.** Events are emitted with `tid = streamId` and a
`thread_name` metadata event per stream, so concurrency is legible at a glance
in the standard viewers. Correlation IDs are carried in `args` for anyone who
wants to join against other data.

## Non-goals

| Not doing | Why |
|---|---|
| Shipping an injector | remote dlopen belongs to the orchestrator; see embedding.md |
| Multi-process / cluster orchestration | belongs in whatever already schedules the jobs |
| Remote upload, storage backends | the output is a file; pipe it wherever |
| Python / framework stack fusion | needs an interpreter-side collector and a join model |
| Hardware counters, occupancy analysis | serialized kernel replay, a different tool shape |
| Non-NVIDIA backends | CUPTI is the only interface used |

## Correctness notes

- Activity callbacks run on CUPTI-owned threads; the event vector is
  mutex-guarded.
- `Stop()` is idempotent — both `atexit` and the `--duration` timer can reach
  it, and either may win.
- Timestamps are CUPTI nanoseconds converted to microseconds for the trace
  format. They share one clock domain, so no cross-domain correction is applied.
- Dropped-record counts are reported under `-v`. A nonzero count means the
  buffer path could not keep up, and the timeline has holes.
- Kernel names are demangled with `abi::__cxa_demangle`; templated names can be
  long, and are emitted verbatim after JSON escaping.
- The library is linked `-z nodelete` and is never unloaded: CUPTI holds the
  buffer-callback pointers for the process lifetime and there is no
  per-callback unregister API, so an orchestrator's `dlclose` is a refcount
  decrement only. Truncating or overwriting the injected file is forbidden by
  the embedding contract; that variant faults with SIGBUS even though the
  mapping survives.
- A loaded instance serves consecutive collection windows: Start() re-arms
  the sink, Stop() swaps the event buffer out and clears it, so windows do
  not accumulate into each other. Start()/Stop() state transitions are
  mutex-guarded; `atexit` and the duration timer may still race for Stop(),
  but only the first one performs the stop sequence.
- The forced flush can deliver records for still-running work (no end
  timestamp); IngestRecord drops them rather than emitting a wrapped
  duration into the trace.
- Write failure ends a window with `CUPTIProfilingFailed` instead of
  `CUPTIProfilingWriterOver`, so socket consumers never block on a file that
  will not appear.
