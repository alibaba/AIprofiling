#!/usr/bin/env bash
# prepare-libprofiler.sh
#
# Post-build hardening for the vendored libprofiler.a static archive.
# Run once after receiving a fresh libprofiler.a build from upstream,
# then commit the resulting archive. Intended output:
#
#   * nm --defined-only reports only libprofiler's public API
#     (functions declared in profiler.h) plus the
#     libprofiler_version symbol.
#   * .comment / .note.* sections are stripped so the archive
#     no longer carries GCC version / build-id / build-host fingerprints.
#
# Implementation notes:
#
#   libprofiler.a's public functions (e.g. profiler_attach) call into
#   private helpers (e.g. profiler__seize) that live in *sibling* object
#   files inside the archive. If we localize private symbols object-by-
#   object, those cross-object references break at link time
#   ("undefined symbol: profiler__seize").
#
#   To avoid that, we first partial-link the archive with
#     ld -r --whole-archive
#   which folds every object into one relocatable object file. Cross-
#   object calls become intra-object calls, and every private symbol is
#   safe to localize because its callers now live in the same object.
#   We then rewrap the single object into an ar archive so build.rs's
#   `cargo:rustc-link-lib=static=profiler` continues to work unchanged.
#
# Usage:
#   tools/prepare-libprofiler.sh
#
# Environment:
#   LD         override ld binary (default: ld)
#   OBJCOPY    override objcopy binary (default: objcopy)
#   STRIP      override strip binary (default: strip)

set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$here/../../.." && pwd)"

LD="${LD:-ld}"
OBJCOPY="${OBJCOPY:-objcopy}"
STRIP="${STRIP:-strip}"

for tool in "$LD" "$OBJCOPY" "$STRIP"; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "error: $tool not found in PATH" >&2
        exit 1
    fi
done

# libprofiler's public API. Kept in sync with:
#   agent/collection_framework/src/third_party/profiler/include/profiler.h
# Any function or variable listed here stays STB_GLOBAL. Everything else
# in the archive is localized (STB_LOCAL) so it does not appear in the
# dynamic symbol table of the final CollectionFramework binary and is
# not visible to nm on the archive itself.
PUBLIC_SYMBOLS=(
    profiler_init
    profiler_attach
    profiler_detach
    profiler_inject
    profiler_uninject
    profiler_call
    profiler_remote_func_addr
    profiler_remote_call
    profiler_remote_vcall
    profiler_inject_in_cloned_thread
    profiler_continue
    profiler_stop
    profiler_wait
    profiler_loose
    profiler_retach
    profiler_notify
    profiler_set_timeout
    libprofiler_version
)

# The single vendored copy of libprofiler.a. pyki's setup.py links
# through the same archive via a relative path (../../third_party/...)
# so there is exactly one binary to keep hardened.
ARCHIVES=(
    "$repo_root/agent/collection_framework/src/third_party/profiler/src/libprofiler.a"
)

for archive in "${ARCHIVES[@]}"; do
    if [ ! -f "$archive" ]; then
        echo "error: archive not found: $archive" >&2
        exit 1
    fi
done

keep_args=()
for sym in "${PUBLIC_SYMBOLS[@]}"; do
    keep_args+=(--keep-global-symbol="$sym")
done

for archive in "${ARCHIVES[@]}"; do
    echo "[prepare-libprofiler] processing $archive"

    workdir="$(mktemp -d)"
    trap 'rm -rf "$workdir"' EXIT

    cp "$archive" "$workdir/libprofiler.a"

    # Partial-link every object in the archive into a single relocatable
    # object. --whole-archive forces inclusion of every member (without it
    # ld would only pull in objects reachable from undefined symbols).
    # After this step, every cross-object private call has been resolved
    # into an intra-object relocation, so we can safely localize private
    # symbols without breaking link-time symbol resolution.
    ( cd "$workdir" \
        && "$LD" -r --whole-archive libprofiler.a -o libprofiler.merged.o )
    rm "$workdir/libprofiler.a"

    "$OBJCOPY" \
        "${keep_args[@]}" \
        --strip-unneeded \
        --remove-section=.comment \
        --remove-section=.note \
        --remove-section=.note.GNU-stack \
        --remove-section=.note.gnu.property \
        "$workdir/libprofiler.merged.o"
    "$STRIP" --strip-debug "$workdir/libprofiler.merged.o"

    # Rebuild the archive with the merged, rewritten object. `ar rcs`
    # recreates the archive index; without it the linker would fail to
    # resolve profiler_init at link time.
    ( cd "$workdir" && ar rcs libprofiler.a libprofiler.merged.o )

    cp "$workdir/libprofiler.a" "$archive"

    rm -rf "$workdir"
    trap - EXIT

    echo "[prepare-libprofiler] rewrote $archive"
done

echo
echo "[prepare-libprofiler] done. Verify with:"
echo "    nm --defined-only ${ARCHIVES[0]} | grep ' T \\| D \\| R \\| B '"
