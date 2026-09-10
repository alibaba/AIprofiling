# injector_probe — libprofiler.a stress harness

Purpose: exercise the injector code path (attach / inject / detach) directly
against a synthetic Python target, so the failures observed in the full stack
(tokio starve, `__tls_get_addr` blacklist, double-free on retry, wait4 hang
without `profiler_set_timeout`) can be isolated to a single layer. Driving
libprofiler.a on its own removes the Rust `CollectionFramework` harness from
the picture, so any difference points at that harness rather than the C
injector itself.

Layout:

- `target.py` — controllable Python workload. Modes:
    - `tls`   : busy `dlopen`/`dlclose` cycle to keep glibc's TLS DTV hot
    - `heap`  : multi-thread malloc/free churn
    - `mixed` : both simultaneously
    - `torch` : minimal torch training with `record_memory_history` on, so
                you can exercise the snapshot path alongside injection
- `probe.c` — direct libprofiler.a driver. Calls
  `profiler_init → profiler_attach → profiler_detach` in a loop, with
  optional switches to reproduce specific injector bugs:
    - `--timeout <sec>`   : call `profiler_set_timeout` before attach.
                            Set to 0 to reproduce the "wait4 hangs
                            forever" behaviour seen prior to the fix.
    - `--detach-on-fail`  : reproduce the double-free that occurs when the
                            caller manually invoked `profiler_detach`
                            after attach returned -2.
    - `--iterations N`    : how many attach cycles.
    - `--sleep-ms N`      : delay between cycles.
- `Makefile` — builds `probe` against `libprofiler.a`. Point it at your
  `libprofiler.a` via `LIBPROFILER_DIR=`.
- `run.sh` — convenience: spawn target, spawn probe, collect a histogram
  (ok / -2 / -1 / hung>timeout / crash).

## Build

```
make LIBPROFILER_DIR=/path/to/libprofiler
```

You need `profiler.h` and `libprofiler.a` in `LIBPROFILER_DIR`, plus the
usual glibc / libdl / libpthread on the link line. Static libprofiler.a
also pulls in binutils (`bfd`) and dwarf — the Makefile below links them
dynamically; adjust for your build layout.

## Reproduce each bug in isolation

**A — `__tls_get_addr` blacklist (-2 spam)**
```
./target.py --mode tls &                    # keeps TLS DTV hot
./probe --pid $! --iterations 200 --sleep-ms 50 --timeout 30
```
Expect a non-trivial fraction of `-2` results. This is the classic
"blacklist" trip.

**B — heap allocator on stack (-2 spam)**
```
./target.py --mode heap --threads 8 &
./probe --pid $! --iterations 200 --sleep-ms 50 --timeout 30
```
Same signature but different cause: `_int_malloc` / `_int_free` on the stack.

**C — no `profiler_set_timeout` → wait4 hangs**
```
./target.py --mode tls &
./probe --pid $! --iterations 5 --timeout 0
```
Set `--timeout 0` to explicitly *skip* `profiler_set_timeout`. Without it,
a blacklist-inducing target thread will pin the probe in wait4 forever
(reproduces the 63s tokio idle timeout root-cause). With `--timeout 30`,
each cycle bounds at 30s max.

**D — double-free on retry**
```
./target.py --mode tls &
./probe --pid $! --iterations 200 --detach-on-fail --timeout 30
```
Reproduces `free(): double free detected in tcache 2` — libprofiler.a
already frees its ptrace state when `profiler_attach` returns -2. Any extra
`profiler_detach` on that handle crashes.

**E — tokio starvation** *(not reproducible here — this is a
CollectionFramework-level bug, not libprofiler.a. To verify the full
CollectionFramework, drive it end-to-end with `run.sh --with-cf`.)*

**F — injected loader hangs on unix socket connect** *(reproduce with the
pyki loader itself; see `--mode torch` and skip probing — this needs the
loader .so, not just the injector.)*

## Metrics probe emits

```
probe: pid=... iters=200 mode=... histogram:
  ok                : NN
  err_check_stack -2: NN  (blacklist)
  err_other       -1: NN
  hung (>timeout)   : NN
  crashed           : 0/1
  avg_attach_ms     : XX.X
  p95_attach_ms     : XX.X
```

Send the histogram + kernel + libprofiler.a build hash and the two runs can
be compared.

## Prereqs on target host

- `CAP_SYS_PTRACE` (or run as root)
- kernel with `process_vm_readv` (>= 3.2, universally true)
- if in docker: `--cap-add=SYS_PTRACE`, `--pid=host` optional but useful
