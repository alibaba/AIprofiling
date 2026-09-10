# pyki (vendored)

This is the full pyki source tree, vendored into AIProf so we can modify,
debug, and rebuild it locally. It lives alongside the other Collection
Framework plugins (`src/plugins/`).

## Runtime layout

`pyki_dev_dir/pyki/` holds the prebuilt wheels
(`pyki-*-cp39/310/311/312-*.whl`) that `utils::install_pyki` installs into
the target process via `pip install pyki --find-links=file:///pyki_dir/pyki`.

`build.rs` stages that directory next to the built binary as `pyki_dir/`,
and `package.sh` copies it into the release tarball under the same name. At
deploy time the agent must bind-mount or symlink `pyki_dev_dir/pyki/` to
`/pyki_dir/pyki` inside the target namespace.

## Rebuild flow

To iterate on the source here and refresh the wheels:

```
./sync_to_cf.sh                              # current python3
PYTHON=python3.11 ./sync_to_cf.sh            # a specific interpreter
```

The script wraps this tree's `Makefile`, builds a wheel, and copies it into
`pyki_dev_dir/pyki/`, overwriting the shipped wheel for that CPython
version. Run it once per interpreter for a full cp39..cp312 matrix.

Requirements: `python3.{9,10,11,12}` + `pip`, Rust toolchain (for
`src/pyki/native/rust/`), a C++17 compiler.

## Sub-trees to know about

- `src/pyki/native/` — C++ + Rust native extension (18k lines)
- `src/pyki/profiling/` — torch profiler, chrome trace, nvtx, python
  tracer, vllm, memory timeline (8k lines Python)
- `src/pyki/wrapt/` — vendored wrapt for shadowing
- `third-party/` — pybind11 (libprofiler.a is shared with the injector at
  `agent/collection_framework/src/third_party/profiler/` and linked from
  `setup.py` via a relative path)
