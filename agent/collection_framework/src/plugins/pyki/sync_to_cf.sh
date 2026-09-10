#!/usr/bin/env bash
# Build pyki wheels from this vendored source tree and copy them into
# pyki_dev_dir/pyki/ so `utils::install_pyki` picks them up at runtime.
#
# Two modes:
#
#   ./sync_to_cf.sh                  # single interpreter (host python3)
#   PYTHON=python3.11 ./sync_to_cf.sh
#
#   ./sync_to_cf.sh --manylinux      # cp39/310/311/312 manylinux2014 wheels
#                                    #   built in a container (recommended:
#                                    #   this is what ships in the image)
#
# The container mode needs podman/docker plus a host rust toolchain at
# ~/.cargo + ~/.rustup, which it mounts in rather than downloading.
#
# Keep the version in src/pyki/__init__.py in sync with PYKI_VERSION and
# PYKI_VERSION_IN in agent/collection_framework/src/const.rs — CF reinstalls
# pyki whenever the installed version differs from PYKI_VERSION_IN, and the
# injected snippet imports pyki.<PYKI_VERSION>.profiling.torch_profile.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CF_WHEEL_DIR="$SCRIPT_DIR/pyki_dev_dir/pyki"
MANYLINUX_IMAGE="${MANYLINUX_IMAGE:-quay.io/pypa/manylinux2014_x86_64:latest}"
PY_TAGS="${PY_TAGS:-cp39-cp39 cp310-cp310 cp311-cp311 cp312-cp312}"

cd "$SCRIPT_DIR"

# setup.py's shadow_pyki rewrites `pyki.` -> `pyki.vX_Y_Z.` in place across
# build_lib. Running it over output that was already shadowed produces
# pyki.vX_Y_Z.vX_Y_Z., so build/ must be gone before every wheel build.
# `make clean` is not enough here: it also runs `cargo clean`, and it is
# tolerant of failures in environments that restrict `rm -rf`.
wipe_build_dirs() {
    rm -rf "$SCRIPT_DIR/build" "$SCRIPT_DIR/dist" "$SCRIPT_DIR/src/pyki.egg-info"
    for d in build dist src/pyki.egg-info; do
        if [ -e "$SCRIPT_DIR/$d" ]; then
            echo "error: could not remove $SCRIPT_DIR/$d — stale shadowed" \
                 "output would corrupt the next build" >&2
            exit 1
        fi
    done
}

write_index() {
    local index="$CF_WHEEL_DIR/index.html"
    {
        echo '<!DOCTYPE html>'
        echo '<html>'
        echo '<body>'
        echo '    <h1>Links for pyki</h1>'
        for whl in "$CF_WHEEL_DIR"/*.whl; do
            local name
            name="$(basename "$whl")"
            echo "    <a href=\"./${name}\">${name}</a><br/>"
        done
        echo '</body>'
        echo '</html>'
    } > "$index"
    echo "==> wrote $index"
}

build_manylinux() {
    local runtime
    runtime="$(command -v podman || command -v docker || true)"
    if [ -z "$runtime" ]; then
        echo "error: --manylinux needs podman or docker on PATH" >&2
        exit 1
    fi
    if [ ! -d "$HOME/.cargo" ] || [ ! -d "$HOME/.rustup" ]; then
        echo "error: --manylinux mounts the host rust toolchain; install it" \
             "with rustup first" >&2
        exit 1
    fi

    local work out
    work="$(mktemp -d)"
    out="$(mktemp -d)"
    trap 'rm -rf "$work" "$out"' RETURN

    echo "==> staging source into $work"
    rsync -a \
        --exclude 'pyki_dev_dir' --exclude 'dist' --exclude 'build' \
        --exclude 'src/pyki/native/rust/target' --exclude '*.egg-info' \
        --exclude 'htmlcov' --exclude 'tests_shadowed' \
        "$SCRIPT_DIR/" "$work/"

    cat > "$work/.build_in_container.sh" <<'INNER'
set -euo pipefail
export CARGO_HOME=/hostcargo RUSTUP_HOME=/hostrustup
export PATH="$CARGO_HOME/bin:$PATH"
export CARGO_TARGET_DIR=/work/src/pyki/native/rust/target
cd /work

# Build the rust staticlib inside the container so its glibc references
# match manylinux2014 rather than the (newer) host glibc.
(cd src/pyki/native/rust && cargo build --release)

for TAG in $PY_TAGS; do
    PY="/opt/python/${TAG}/bin/python"
    echo "==== ${TAG} ===="
    # The mirror reachable from build hosts is often slow enough to trip
    # pip's 15s default read timeout.
    "$PY" -m pip install --quiet --upgrade --retries 10 --timeout 120 \
        pip setuptools wheel build auditwheel
    rm -rf build/ dist/ src/pyki.egg-info
    "$PY" -m build --wheel --no-isolation
    "$PY" -m auditwheel repair --plat manylinux2014_x86_64 -w /out dist/*.whl
done
INNER

    echo "==> building $PY_TAGS in $MANYLINUX_IMAGE"
    "$runtime" run --rm \
        -v "$work":/work:Z \
        -v "$out":/out:Z \
        -v "$HOME/.cargo":/hostcargo:Z \
        -v "$HOME/.rustup":/hostrustup:Z \
        -e PY_TAGS="$PY_TAGS" \
        "$MANYLINUX_IMAGE" \
        bash /work/.build_in_container.sh

    mkdir -p "$CF_WHEEL_DIR"
    rm -f "$CF_WHEEL_DIR"/*.whl
    cp -v "$out"/*.whl "$CF_WHEEL_DIR/"
}

build_host() {
    local python="${PYTHON:-python3}"
    if ! command -v "$python" >/dev/null 2>&1; then
        echo "error: interpreter '$python' not found on PATH" >&2
        exit 1
    fi
    echo "==> building pyki wheel with $python"
    "$python" -m pip install --user --quiet build
    wipe_build_dirs
    (cd src/pyki/native/rust && cargo build --release)
    "$python" -m build --wheel --no-isolation

    mkdir -p "$CF_WHEEL_DIR"
    cp -v dist/*.whl "$CF_WHEEL_DIR/"
}

if [ "${1:-}" = "--manylinux" ]; then
    build_manylinux
else
    build_host
fi

write_index

echo "==> done. CF now sees:"
ls "$CF_WHEEL_DIR"
