# cuprof

A minimal CUDA kernel timeline profiler built on
[CUPTI](https://docs.nvidia.com/cupti/). Run a CUDA program under `cuprof` and
get a Chrome-trace JSON you can open in `chrome://tracing` or
[Perfetto](https://ui.perfetto.dev).

> **Building cuprof for use inside AIProf's CollectionFramework?** Read
> `../../third_party/cupti/README.md` first. AIProf ptrace-injects the
> compiled `libcuprof.so` into arbitrary target processes; the CUPTI ABI
> is tied to the build machine's CUDA major, and a mismatch with the
> target host's CUDA driver silently SIGSEGVs the injected process. The
> vendored CUPTI directory documents the alignment contract and how to
> rebuild for a different CUDA major.

```bash
cuprof run -o trace.json python train.py
```

No daemon, no server, no background state — one CLI, one injected library, one
JSON file out.

## Two ways to use it

**Launch mode** (the CLI). `cuprof run` sets `CUDA_INJECTION64_PATH` and execs
your program; the CUDA driver loads the profiler during CUDA initialization.

```
cuprof run <program>
   |  sets CUDA_INJECTION64_PATH=libcuprof.so and CUPROF_* variables, then execs
   v
<program>  -- the CUDA driver loads libcuprof.so during CUDA initialization
   |          and calls the entry point, which enables CUPTI activities
   v
process exit -- atexit flush --> trace.json
```

**Embedded mode** (no CLI). An external orchestrator `dlopen`s
`libcuprof.so` into an already-running process and calls `cuprof_start()`
itself. Since an injected library cannot be handed
environment variables, config comes from a `KEY=VALUE` file, and progress is
reported over an optional unix socket. cuprof ships no injector of its own —
see [docs/embedding.md](docs/embedding.md) for the contract.

## Collected activities

| Activity | Fields |
|---|---|
| Kernel launches | name (demangled), duration, stream, grid/block, registers, shared mem |
| Memcpy | direction, bytes, duration, stream |
| Memset | bytes, duration, stream |

That is the complete list — cuprof is deliberately minimal. Events are emitted
one lane per CUDA stream, so concurrency is visible at a glance.

## Build

Requires Linux x86_64, a CUDA Toolkit providing CUPTI, and a C++11 compiler.

```bash
make CUDA_HOME=/usr/local/cuda      # -> build/libcuprof.so, build/cuprof
sudo make install                   # -> /usr/local/{bin,lib}
```

CUPTI is linked from `$CUDA_HOME/extras/CUPTI`; no CUDA libraries are vendored
into this repository, and the supported CUDA range is whatever the build
machine's toolkit provides.

Tested with CUDA 12.8 and 13.0 on NVIDIA A10 (driver 580.126.09), g++ 13.

## Usage

```bash
# profile a binary
cuprof run ./my_cuda_app arg1 arg2

# pick the output path, stop collecting after 30 seconds
cuprof run -o /tmp/t.json --duration 30 python train.py

# see what the profiler is doing
cuprof run -v ./app

# '--' is only needed when the target itself starts with '-'
cuprof run -o t.json -- -weird-binary-name
```

Then open the JSON at `chrome://tracing` or https://ui.perfetto.dev.

### Verify the build

```bash
make examples
./build/cuprof run -v -o /tmp/t.json ./build/vector_add
```

Expect `[cuprof] wrote N events` with N > 0, and the example's two streams on
separate timeline lanes.

## Options

The CLI translates its flags into these variables; in embedded mode the same
keys go into the config file. The environment takes precedence over the file.

| CLI flag | Key | Default |
|---|---|---|
| `-o, --output PATH` | `CUPROF_OUTPUT` | `./cuprof_<pid>.json` |
| `-d, --duration SEC` | `CUPROF_DURATION` | `0` (collect until process exit) |
| `-v` / `-q` | `CUPROF_VERBOSE` | `0` |
| — | `CUPROF_SOCKET` | unset (no lifecycle notifications) |
| — | `CUPROF_CONFIG` | `/tmp/cuprof_<pid>.cfg`, then `/tmp/cuprof.cfg` |
| — | `CUPROF_LIB` | auto-detected next to the CLI |

## Layout

```
src/               injected library + CLI (C++11)
examples/          CUDA workload for manual verification
docs/design.md     design notes and non-goals
docs/embedding.md  contract for external orchestrators
export.map         symbol visibility for the injected library
```

## Non-goals

Shipping an injector, multi-process orchestration, remote upload,
framework/Python stack fusion, hardware counter metrics, non-NVIDIA backends.
The reasoning behind each omission is in [docs/design.md](docs/design.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
