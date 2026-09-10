# Embedding cuprof in an external orchestrator

Besides `cuprof run`, the injected library supports being loaded into an
**already-running** CUDA process by any external tool that can perform a
remote `dlopen` (a ptrace-based injector, a debugger, or a framework with its
own injection machinery). cuprof itself ships no injector — this document
defines the contract such a tool must follow.

## Contract

1. **Load** `libcuprof.so` into the target process. The library has no
   constructor; loading it has no effect on its own.
2. **Configure** it *before* calling the entry point. The target's environment usually
   cannot be changed, so the library also reads a plain-text config file,
   trying in order:
   - `$CUPROF_CONFIG` (if the environment happens to be controllable)
   - `/tmp/cuprof_<pid>.cfg` — `<pid>` as seen *inside* the target's pid
     namespace
   - `/tmp/cuprof.cfg` — pid-less fallback for one-process-per-container
     setups

   Format: `KEY=VALUE` per line, `#` comments. Keys are the same as the
   environment variables:

   ```
   CUPROF_OUTPUT=/tmp/myjob_cupti.json
   CUPROF_DURATION=30
   CUPROF_SOCKET=/tmp/.orchestrator.sock
   CUPROF_VERBOSE=1
   ```

   Paths are interpreted inside the target's mount namespace.

3. **Call** `cuprof_start()`. It returns 1 on success, 0 on failure.
   Collection starts immediately; there is no warmup phase.

   The library also exports `InitializeInjection()`, a shim that just calls
   `cuprof_start()`. It exists because the CUDA driver looks up that exact name
   for `CUDA_INJECTION64_PATH`; prefer `cuprof_start()` in your own code.
4. **Wait** for completion. Set `CUPROF_DURATION` so the library stops and
   writes the file on its own; with duration 0 it only flushes at process
   exit. To collect another window from the same process, write a fresh
   config (new `CUPROF_OUTPUT`) and call `cuprof_start()` again — the same
   loaded instance starts the next window.

5. **Never unload the library.** Do not `dlclose` `libcuprof.so`, and do not
   truncate or overwrite the file it was loaded from while the target runs
   (deleting the file is harmless; truncating or overwriting it is not —
   accessing file-backed pages beyond the file's end raises SIGBUS).
   Unloading is unsafe because CUPTI keeps holding pointers into the
   library's code for the life of the process: the activity buffer callbacks
   registered by `cuprof_start()` have no per-callback unregister API.
   (`cuptiFinalize()` can detach CUPTI wholesale, but cuprof does not
   implement that teardown, and it would affect every other CUPTI user in
   the process.) If the mapping goes away, the next CUPTI buffer delivery
   executes unmapped memory and kills the target; if instead the backing
   file is truncated or overwritten while the mapping stays, the target dies
   with SIGBUS. The library is linked with `-z nodelete`, so a `dlclose`
   returns success **without unmapping** — do not rely on unload semantics,
   and note that a repeated `dlopen` of the same path returns the same
   handle. One loaded instance serves the target's whole lifetime;
   consecutive collection windows reuse it (step 4).

## Lifecycle notifications

When `CUPROF_SOCKET` is set, cuprof connects to that unix stream socket and
sends one message per connection (plain text, no framing, no reply expected):

| Message | Meaning |
|---|---|
| `CUPTIProfilingStart` | collection is active |
| `CUPTIProfilingStop` | collection stopped, records flushed from CUPTI |
| `CUPTIProfilingWriterOver` | the output file is complete and consumable |
| `CUPTIProfilingFailed` | the output file could not be written; this window produces no file |

Consumers must key off `CUPTIProfilingWriterOver` — after `Stop` the file may
not exist yet. `CUPTIProfilingFailed` is the terminal failure signal for a
window: treat it like `WriterOver` for unblocking, and do not wait for a
file. Notifications are best-effort: a missing or dead socket never affects
collection.

## What cuprof does not do in embedded mode

- No reload between rounds: consecutive collection windows reuse the same
  loaded instance (`cuprof_start()` again after the previous window
  finished), one output file per window. Injecting a second *copy* of the
  library into the same process is unsupported: buffer delivery would depend
  on version-specific CUPTI re-registration behavior, and every copy leaks
  permanently (the library is `-z nodelete`).
- No reconfiguration of a *running* window; the config is re-read by each
  `cuprof_start()`.
- No coexistence handling: if the target already uses CUPTI (for example a
  framework-integrated profiler is active in the same process), whichever
  subscriber came first wins and the other gets errors. Avoid double
  profiling.
- No cleanup of the config file; the orchestrator owns it.

## Minimal manual example with gdb

```bash
PID=<target>
cat > /tmp/cuprof_$PID.cfg <<EOF
CUPROF_OUTPUT=/tmp/embed_cupti.json
CUPROF_DURATION=10
CUPROF_VERBOSE=1
EOF

gdb -p $PID -batch \
  -ex 'call (void*) dlopen("/usr/local/lib/libcuprof.so", 2)' \
  -ex 'call (int) cuprof_start()'

sleep 11 && cat /tmp/embed_cupti.json | head
```
