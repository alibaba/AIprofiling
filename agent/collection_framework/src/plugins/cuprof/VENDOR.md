# Vendored cuprof — provenance and local patches

This directory is a **vendored copy** of cuprof, not a git submodule and not a
subtree. Do not edit files here ad hoc: either change upstream and re-sync, or
add a patch under `patches/` and record it below.

| | |
|---|---|
| Upstream | `cuprof` — clone URL is maintained internally and deliberately **not** published here; ask an AIProf maintainer for it |
| Base branch | `main` |
| Base commit | `84cb9d8` — "fix: keep libcuprof.so loaded; rewrite the unload contract" (2026-08-12) |
| Last synced | 2026-09-10 — re-verified *internally* that upstream `main` is still `84cb9d8`. Not independently checkable, because the clone URL above is deliberately unpublished. |
| License | Apache-2.0. `LICENSE` and `NOTICE` here are upstream's, unmodified. |

Everything in this tree is byte-identical to `cuprof@84cb9d8` **except** the seven
files touched by the four patches below. Each of those files carries an
Apache-2.0 §4(b) "modified" notice in its first lines — a code comment in the
six source files, an HTML comment in `README.md` so the rendered page is
unchanged.

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
(it does *not* clamp to 0). The event **duration** is *not* derived from the two
shifted endpoints — see `patches/0004-duration-from-raw-endpoint-pair.patch`.

Note this implements "framework/Python stack fusion", which upstream lists as a
**non-goal** in `docs/design.md`. Keep it local unless upstream adopts it.

### `patches/0003-readme-aiprof-cupti-alignment-note.patch`

`README.md`. Puts a note at the top of the vendored README that sends a builder
to `../../third_party/cupti/README.md`, and records that upstream's "no CUDA
libraries are vendored" claims describe upstream cuprof as a stand-alone
repository rather than this tree.

Upstream links against the system CUPTI and vendors nothing, so its README says
nothing about which CUPTI a build ends up needing. AIProf ptrace-injects the
compiled `libcuprof.so` into arbitrary target processes, where the CUPTI ABI is
tied to the build machine's CUDA major and a mismatch with the target host's
driver silently SIGSEGVs the injected process. That constraint is documented one
level up in `src/third_party/cupti/`, which a reader of this README has no reason
to open.

Upstream's `NOTICE` ends with the same "No NVIDIA libraries or headers are
distributed with this repository" claim, which is likewise true of upstream and
false of AIProf. It is deliberately **left unmodified**: it is upstream's
attribution file, and the correction belongs in the AIProf-local README note, not
in an edit to an upstream licence document.

Consumer: `src/third_party/cupti/README.md`.

### `patches/0004-duration-from-raw-endpoint-pair.patch`

`src/trace_writer.cc`. Takes each event's `"dur"` from the raw, unshifted
endpoint pair instead of subtracting the two values patch 0002 shifted.

Patch 0002 shifts `start_ns` and `end_ns` separately, which is only sound while
both endpoints land on the same side of the offset. The offset is computed once
in `LoadConfig()` and is roughly the wall-clock time at boot, so it is only
valid while `CLOCK_REALTIME` keeps advancing. A step **backwards** by more than
the uptime — a VM resumed from a snapshot with a stale RTC, an explicit
`date -s`, or a large `chronyc makestep` on a host that booted with a badly
wrong clock — puts later CUPTI timestamps below an offset derived from the
pre-step clock. (Ordinary NTP slewing cannot: it would have to move the clock
back past boot time.) An event that straddles the offset then gets one endpoint
shifted and the other left alone, so
`adj_end - adj_start` underflows `uint64` to roughly 1.8e19 ns. The emitted
duration becomes ~1.8e16 us, which stretches Perfetto's time bounds by six
orders of magnitude and flattens every real kernel into a single pixel, while
`WriteChromeTrace()` still returns `true` and CollectionFramework still reports
a successful collection. Nothing downstream can tell the trace is corrupt.

An `end_ns` of `0` underflows the same way but cannot reach the writer through
cuprof's own path: `IngestRecord` in `src/cupti_sink.cc` already drops records
with `end == 0 || end < start`, for exactly this anti-wrap reason. It is fixed
here anyway because `WriteChromeTrace()` is exported in `src/trace_writer.h` and
cannot assume that every embedder filtered its records first.

The trigger is narrow, so treat this as hardening an exported function rather
than as a field bug: `WriteChromeTrace()` is declared in `src/trace_writer.h` and
has no business assuming its caller kept the two endpoints on one side of a shift
that the caller never sees. A duration is invariant under a uniform shift, so the
raw pair is the correct source; a reversed or unset pair clamps to zero. Timestamps keep patch 0002's
semantics exactly.

Unlike 0002 this is not an AIProf-specific policy but a latent bug in the
conversion 0002 introduces, so if 0002 is ever offered upstream this fix should
travel with it.

Consumer: `test/native/test_cuprof_trace_writer.cc`, which covers both shapes (a
straddling event and an unset `end_ns`) and is the only automated coverage of
either. Run it with `make test-native`; against the unfixed writer those two
cases report durations of 1.67e16 us and 1.84e16 us.

## Re-syncing to a newer upstream

```bash
git clone <internal-cuprof-url> /tmp/cuprof-upstream   # URL not published; see the row above
cd /tmp/cuprof-upstream && git checkout <new-base-commit>

# from the AIProf repo root
D=agent/collection_framework/src/plugins/cuprof
rsync -a --delete --exclude='.git' --exclude='patches' --exclude='VENDOR.md' \
      /tmp/cuprof-upstream/ "$D"/
cd "$D" && git apply patches/000*.patch
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
     `CUPROF_OUTPUT` / `DURATION` / `VERBOSE` / `SOCKET`, the four keys
     `write_cuprof_config()` in `cupti_plugin_wrapper.rs` emits, plus
     `CUPROF_CONFIG`, which that function does **not** write: it is the
     environment key `LoadCfgFile()` reads to locate the `.cfg` file, and it is
     part of the same contract.
   - `drift_guard_vendored_cuprof_keeps_the_cfg_filename_convention` — the
     `/tmp/cuprof_<pid>.cfg` path, which `write_cuprof_config()` writes into the
     target's namespace and `LoadCfgFile` must look for by exactly that name.
3. The guards are substring checks and none of them covers
   `src/trace_writer.cc`, so after a re-sync also run `make test-native` — it is
   the only automated check on patches 0002 and 0004.
4. Rebuild the client image — `deploy/docker/Dockerfile.client` stage 1b builds
   this tree with `nvidia/cuda:${CUDA_VERSION}-devel-${BUILDER_UBUNTU}`, which
   defaults to `12.2.0-devel-ubuntu20.04` (20.04 deliberately, so the injected
   library's glibc symbols stay old enough for Anolis 8 / CentOS 8 targets).
   `CUDA_VERSION` has to match the CUDA major of the hosts the client will inject
   into, or injection SIGSEGVs — see `src/third_party/cupti/README.md`.
