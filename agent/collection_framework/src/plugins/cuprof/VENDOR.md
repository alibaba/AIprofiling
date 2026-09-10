# Vendored cuprof — provenance and local patches

This directory is a **vendored copy** of cuprof, not a git submodule and not a
subtree. Do not edit files here ad hoc: either change upstream and re-sync, or
add a patch under `patches/` and record it below.

| | |
|---|---|
| Upstream | `cuprof` — clone URL is maintained internally and deliberately **not** published here; ask an AIProf maintainer for it |
| Base branch | `main` |
| Base commit | `84cb9d8` — "fix: keep libcuprof.so loaded; rewrite the unload contract" (2026-08-12) |
| Last synced | 2026-09-08 (verified: upstream `main` == `84cb9d8`, no newer commit) |
| License | Apache-2.0. `LICENSE` and `NOTICE` here are upstream's, unmodified. |

Everything in this tree is byte-identical to `cuprof@84cb9d8` **except** the six
files touched by the two patches below. Each of those files carries an
Apache-2.0 §4(b) "modified" notice in its first two lines.

Reproducibility is checked by applying `patches/*` to a pristine
`cuprof@84cb9d8` checkout and diffing against this tree — the result must be
empty apart from `patches/` and this file.

## Local patches

### `patches/0001-rpath-origin-for-ptrace-injection.patch`

`Makefile`. Puts `$ORIGIN` ahead of `$(CUPTI_HOME)/lib64` in `libcuprof.so`'s
RPATH.

Upstream only supports the `CUDA_INJECTION64_PATH` launch flow, where the
library is loaded in the build host's own namespace. CollectionFramework instead
ptrace-injects it into an **already-running** target process, so `dlopen` and
symbol resolution happen in the target's address space using the target's search
paths — the build-time CUPTI path may not exist there (different CUDA major
version, or a container build against a bare-metal target). `$ORIGIN` resolves
to the directory the injected copy was staged into (`/tmp` inside the target's
mount namespace), where `CUPTIPluginWrapper::stage_cupti_runtime()` drops a
vendored `libcupti.so.<major>` matching the library's `DT_NEEDED`.

Consumer: `src/plugins/cupti_plugin_wrapper.rs`.

### `patches/0002-align-cupti-epoch-to-clock-monotonic.patch`

`src/config.{h,cc}`, `src/trace_writer.{h,cc}`, `src/cupti_sink.cc`. Converts
CUPTI's `CLOCK_REALTIME` (epoch) nanosecond timestamps to `CLOCK_MONOTONIC`.

pyki / `torch.profiler` timestamps are `CLOCK_MONOTONIC`. AIProf renders both
sources on a single Perfetto timeline, so an unadjusted CUPTI trace lands
decades away from the Python-stack lanes and the fused view is unusable. The
offset `(CLOCK_REALTIME - CLOCK_MONOTONIC)` is computed once in `LoadConfig()`
and subtracted per event in `WriteChromeTrace()`. A timestamp at or below the
offset is left unchanged rather than subtracted, so it can never wrap `uint64`
(it does *not* clamp to 0).

Note this implements "framework/Python stack fusion", which upstream lists as a
**non-goal** in `docs/design.md`. Keep it local unless upstream adopts it.

## Re-syncing to a newer upstream

```bash
git clone <internal-cuprof-url> /tmp/cuprof-upstream   # URL not published; see the row above
cd /tmp/cuprof-upstream && git checkout <new-base-commit>

# from the AIProf repo root
D=agent/collection_framework/src/plugins/cuprof
rsync -a --delete --exclude='.git' --exclude='patches' --exclude='VENDOR.md' \
      /tmp/cuprof-upstream/ "$D"/
cd "$D" && git apply patches/0001-*.patch patches/0002-*.patch
```

A patch that no longer applies means upstream touched the same lines — resolve
by hand, then re-generate it with `git format-patch` and update this file.

After syncing, always:

1. Update the **Base commit** and **Last synced** rows above.
2. **Run `cargo test` in `agent/collection_framework/`.** Three drift guards in
   `src/unix_handler.rs` fail the build if upstream renamed anything we depend
   on, so this step replaces the by-hand checks that used to be listed here:
   - `drift_guard_vendored_cuprof_emits_every_message_we_match_on` — the four
     `CUPTIProfiling*` socket messages, matched **by string equality** against
     `src/const.rs`. This is the coupling most likely to break silently.
   - `drift_guard_vendored_cuprof_reads_every_config_key_we_write` —
     `CUPROF_OUTPUT` / `DURATION` / `VERBOSE` / `SOCKET` / `CONFIG`, the keys
     `write_cuprof_config()` in `cupti_plugin_wrapper.rs` emits.
   - `drift_guard_vendored_cuprof_keeps_the_cfg_filename_convention` — the
     `/tmp/cuprof_<pid>.cfg` path, which `write_cuprof_config()` writes into the
     target's namespace and `LoadCfgFile` must look for by exactly that name.
3. The guards are substring checks, so also eyeball `src/cupti_sink.cc` and
   `src/config.cc` for a rename that left the old string behind in a comment.
4. Rebuild the client image — `deploy/docker/Dockerfile.client` stage 1b builds
   this tree with `nvidia/cuda:12.2.0-devel-ubuntu20.04` (20.04 deliberately, so
   the injected library's glibc symbols stay old enough for Anolis 8 / CentOS 8
   targets).
