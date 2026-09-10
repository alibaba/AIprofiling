# Vendored NVIDIA CUPTI runtime libraries

These are the `libcupti.so.<version>` shared objects that
`agent/collection_framework/src/plugins/cupti_plugin_wrapper.rs` stages next
to `libcuprof.so` at ptrace-injection time. The wrapper reads the target
process's `libcuprof.so` `DT_NEEDED` list, finds the requested
`libcupti.so.<major>` soname, and copies the matching file from this
directory into the target's `/tmp` so `dlopen` succeeds inside the target
process.

Multiple versions are shipped because CUPTI's ABI is tied to the CUDA
Toolkit release used to build the target application. A PyTorch process
built against CUDA 12.1 links a different CUPTI soname than one built
against CUDA 11.4; we cannot know which the operator will inject into
until runtime.

Copyright and licence: see `NOTICE` in this directory. These binaries are
NVIDIA's, are **not** covered by AIProf's Apache-2.0 licence, and are
redistributed unmodified under the NVIDIA CUDA Toolkit EULA.

## Why CUPTI is bundled at all

`libcuprof.so` (the CUDA kernel tracer, see `../../plugins/cuprof/`) is not
launched together with the profiled program. CollectionFramework
`ptrace`-injects it into an already-running target process, and the
*target's* dynamic linker `dlopen`s it there, so dependency resolution
happens in the target's mount namespace using the target's search paths;
the injector's own `LD_LIBRARY_PATH` has no effect. `libcuprof.so` declares
`DT_NEEDED libcupti.so.<soname>` and carries an `$ORIGIN`-first RPATH (see
`../../plugins/cuprof/patches/0001-rpath-origin-for-ptrace-injection.patch`),
so injection succeeds only if a file named exactly that SONAME is reachable
from the directory the injected copy lands in. That directory is the
target's `/tmp`: `custom_init()` copies `libcuprof.so` to
`/proc/<pid>/root/tmp/cf_loader_cupti_<pid>.so` (`CUPTI_DST_PATH` in
`../../const.rs`), so `$ORIGIN` resolves to `/tmp` inside the target
namespace.

No `libcupti.so.<soname>` symlink is shipped here, so `/opt/aiprof/cupti`
on the image's `LD_LIBRARY_PATH` is inert for loader resolution — the
loader matches on file name. The vendored files become usable only because
staging copies one of them to exactly `/tmp/<soname>` next to the injected
`libcuprof.so`.

Most environments already provide libcupti: host toolkit installs ship it
under `/usr/local/cuda-*/extras/CUPTI/lib64/`, and the `nvidia/cuda` images
that include the `cuda-cupti` package (`*-runtime` and `*-devel` tiers)
ship it under `/usr/local/cuda/targets/<arch>-linux/lib/` — the layout
difference is why `deploy/docker/Dockerfile.client` points `CUPTI_HOME` at
`targets/x86_64-linux` when building `libcuprof.so`. It is absent from
minimal images (for example `nvidia/cuda:*-base`, or images trimmed down to
run plain CUDA applications) and from namespaces where only an unrelated
CUDA generation is installed. For those targets,
`CUPTIPluginWrapper::stage_cupti_runtime()` copies the matching vendored
binary through `/proc/<pid>/root/` to `/tmp/<soname>`, which makes
injection self-contained. If staging fails, the wrapper logs a warning and
injection relies on whatever CUPTI the target can resolve on its own. If
the target cannot resolve the SONAME either, `libcuprof.so`'s `dlopen`
fails **silently** — the collector still reports `InitSuccess` and the run
yields an empty kernel timeline with no error (see the RPATH comment in
`../../plugins/cuprof/Makefile`). Deleting this directory therefore does
not degrade GPU kernel collection on such targets; it removes it.

Upstream cuprof deliberately does the opposite:
`../../plugins/cuprof/docs/design.md` lists "System CUPTI, linked at build
time" as a design decision precisely to keep that repository "free of
redistributable-binary questions". AIProf reverses that locally for the
ptrace-injection flow; this directory and its `NOTICE` are the cost of
doing so.

## Contents and provenance

Each file was extracted from the corresponding NVIDIA CUDA Toolkit release
(the `cuda-cupti-<version>` package or the `nvidia/cuda:<tag>-devel-*`
image at `/usr/local/cuda/extras/CUPTI/lib64/`). CUPTI is a
redistributable component under the NVIDIA CUDA Toolkit EULA
(<https://docs.nvidia.com/cuda/eula/>); see `NOTICE` in this directory for
the retained copyright notice and the redistribution declaration, and the
top-level `NOTICE` for the repo-wide inventory.

The SONAME column is what `stage_cupti_runtime()` actually matches on, so
it is listed here instead of left to a local `readelf` run:

| File                   | Bytes   | SHA-256                                                            | ELF SONAME         | CUDA Toolkit line |
|------------------------|---------|--------------------------------------------------------------------|--------------------|-------------------|
| `libcupti.so.9.0.176`  | 5701080 | `0edfb9687babaa4e919de25a30d48dc21367ec2821dc85666ceb23a087dc8e63` | `libcupti.so.9.0`  | CUDA 9.0          |
| `libcupti.so.10.2.75`  | 5761360 | `cfb2777d5b0ff80a4bab2f2366f702490251f1afdc3290507c1641115b0c482d` | `libcupti.so.10.2` | CUDA 10.2         |
| `libcupti.so.2020.1.0` | 6489904 | `800c590127bce2f8d6a4a7d7694c5731669cc0ee4dfc2d8ccd15d646792ea5b1` | `libcupti.so.11.0` | CUDA 11.0         |
| `libcupti.so.2021.2.2` | 7306096 | `1cf95388ff8a012fb8f45fe2b10e8215a9244b204cd4ec29ab37c2c52fbaa24c` | `libcupti.so.11.4` | CUDA 11.4         |
| `libcupti.so.2022.2.0` | 7091568 | `6ac96871a53c07291c7cfe3d5541cd3bbed628fd1299129ca7c568fee333d2c8` | `libcupti.so.11.7` | CUDA 11.7         |
| `libcupti.so.2023.2.1` | 7526416 | `ade8c347db2fe510530854a924965176e25903c2cb31c4fc79d4f4bf9ac4ae1c` | `libcupti.so.12`   | CUDA 12.1         |
| `libcupti.so.2024.1.1` | 7748112 | `fb2a7c5b15c84df9505dd47e553fe46f3121a57d30391fca24179d202f73f3f7` | `libcupti.so.12`   | CUDA 12.3         |
| `libcupti.so.2024.3.2` | 8067920 | `76dcad94420820ea9aee9094c346cc34c6cb1ff630cef9b14e04db298a86305d` | `libcupti.so.12`   | CUDA 12.5         |
| `libcupti.so.2025.2.1` | 7604800 | `2fdab19dc3fccdd4b2f5eba137aabefc68e685f6497bb38eea649866c5672a2a` | `libcupti.so.12`   | CUDA 12.8         |
| `libcupti.so.2025.3.1` | 4152208 | `e2f9ed861fe27c492b8bb52b5e3220ef5120f3edcda36312e96b7fd8a186be3e` | `libcupti.so.13`   | CUDA 13.0         |

Ten files, 67,449,464 bytes in total (~67 MB, ~64 MiB). Four of them —
`2023.2.1`, `2024.1.1`, `2024.3.2`, `2025.2.1` — share the SONAME
`libcupti.so.12`; the SONAME alone does not identify the Toolkit release a
file came from, and only one of the four can be staged for a given
`libcuprof.so` (see below).

Nothing here is patched, stripped or relinked, which is also what the
EULA's Linux redistribution grant (§2.3) requires. Re-verify a file with
`sha256sum -c` against the table above, and its SONAME with:

    readelf -d libcupti.so.<version> | grep SONAME

## CUDA-major alignment: shipping a vendored `.so` is NOT enough

`stage_cupti_runtime` picks a file from this directory by matching its
`SONAME` (e.g. `libcupti.so.13`) against the `DT_NEEDED` entry in the
`libcuprof.so` that CollectionFramework was built with. If the two majors
disagree, the picker returns nothing — or worse, the target-side glibc
loader rejects the mismatch and the injected process SIGSEGVs inside
`InitializeInjection()` before any diagnostic can be printed.

Practical consequences:

- **Adding a newer `libcupti.so.<version>` here does not itself enable
  that CUDA line.** `libcuprof.so` must also be rebuilt against a CUDA
  Toolkit of the same major, so its `DT_NEEDED` names the matching
  soname. Verify with `readelf -d libcuprof.so | grep NEEDED`.
- **Symptom of a mismatch:** the target process SIGSEGVs the moment
  CollectionFramework injects the CUPTI collector into it; CF logs
  `PTRACE_GETREGSET: No such process` (that message is the *detach*
  path finding the target already dead — not the root cause).
- **Fix:** in the client image build, set the CUDA base image to match
  the target host's driver major. `deploy/docker/Dockerfile.client`
  exposes `--build-arg CUDA_VERSION=…` for this. Then keep this
  directory populated with at least one `libcupti.so.<version>` whose
  soname matches every CUDA major the client image is expected to run
  under.

SONAME `libcupti.so.13` is covered by `libcupti.so.2025.3.1` above, so a
`libcuprof.so` built against CUDA 13 can be satisfied from this directory.
The CUDA major the client image is built against is a build argument, not a
constant: `deploy/docker/Dockerfile.client` uses
`nvidia/cuda:${CUDA_VERSION}-devel-${BUILDER_UBUNTU}` to build
`libcuprof.so` and `nvidia/cuda:${CUDA_VERSION}-runtime-${RUNTIME_UBUNTU}`
as its final stage, with `CUDA_VERSION=12.2.0` and
`BUILDER_UBUNTU=ubuntu20.04` as defaults only. `CUDA_VERSION` must match
the target host's CUDA major, or injection SIGSEGVs as described above.

## Selecting a file at runtime

`stage_cupti_runtime()` reads the required SONAME from `libcuprof.so`'s
`DT_NEEDED`, then scans, in order: `<libcuprof.so dir>/cupti/`,
`<libcuprof.so dir>/`, `<agent executable dir>/cupti/`, `<agent executable
dir>/`, de-duplicated — when `libcuprof.so` sits next to the agent
executable those four collapse to two distinct paths. In the shipped image
the first of these is `/opt/aiprof/cupti/`, which
`deploy/docker/Dockerfile.client` populates with this whole directory. The
first directory holding any match wins; if nothing matches anywhere,
staging fails with a warning and injection falls back to whatever CUPTI the
target can resolve on its own.

Four files here share SONAME `libcupti.so.12`, so the match *within* a
directory is ranked rather than taken in `read_dir()` order, which is
neither sorted nor stable across filesystems. Ranking compares the release
number embedded in the file name, component by component, so `2024.3.2`
outranks `2023.2.1` and `10.2.75` outranks `9.0.176`. A file whose name
does not carry at least three numeric components — NVIDIA's
`libcupti.so.<a>.<b>.<c>` convention — is ranked last rather than compared
as a version, so a hand-made `libcupti.so.12` can never outrank a real
release.

The default rule is **highest CUPTI release wins**. The environment
variable `AIPROF_CUPTI_PREFER`, read from the agent process, overrides it:

- unset, empty, or `highest` — the default: the highest release among the
  SONAME matches wins.
- `lowest` — the lowest release among the SONAME matches wins.
- an exact vendored file name, e.g. `libcupti.so.2023.2.1` — that file is
  staged, provided it is present among the SONAME matches in the directory
  being scanned. If it is not, the override is ignored with a warning and
  the default rule applies.

An unrecognised value warns and falls back to `highest`. Whenever more than
one candidate shares the SONAME, the chosen file, the preference in force
and the full candidate set are logged at `info`, so a surprising pick is
diagnosable after the fact.

Neither rule is simply correct, and that is why the override exists.
CUPTI's minimum driver requirement rises with each patch level inside one
SONAME family, so either end of the range trades driver compatibility
against GPU-architecture coverage:

- `highest` on a host still running a CUDA 12.1-era driver hands it
  `libcupti.so.2025.2.1` (CUPTI 2025.2.1, CUDA 12.8), which can ask for more
  than that driver supports and yield zero kernels. Do not expect to be told
  so: CollectionFramework builds its `CuprofConfig` with `verbose: false`, and
  cuprof's `Logf` returns immediately unless `CUPROF_VERBOSE` is set, so no
  CUPTI result code is ever printed. The observable symptom is a run that
  reports success with an empty kernel timeline.
- `lowest`, or pinning an older file by name, keeps the driver requirement
  down but risks CUPTI not recognising a GPU newer than the toolkit release
  the library came from.

With the set as shipped and the default in force, any `libcuprof.so` asking
for `libcupti.so.12` is given `libcupti.so.2025.2.1` on every host. An
operator who needs a different patch level sets `AIPROF_CUPTI_PREFER` on
the agent process — `deploy/docker/run-client.sh` forwards it as
`CUPTI_PREFER` (`--cupti-prefer`), and `deploy/docker/docker-compose.yml`
carries it commented out — while pruning the directory (**Pruning the
set** below) pins the same choice, but only by rebuilding the client
image.

The winner is copied to a unique temporary name in the target's `/tmp` —
`.cupti-staging-<agent pid>-<nanos hex>-<target pid>` — and then `rename(2)`d
onto `/tmp/<soname>`. The rename matters: `libcuprof.so` is linked
`-z nodelete`, so a libcupti staged for an earlier collection window
is still mapped in the target, and copying straight onto the final name
would truncate that live inode and kill the target with SIGBUS on its next
page-in. The temporary name deliberately does not start with
`libcupti.so.`, so a leftover can never be mistaken for a staging
candidate, and both of the staging error paths unlink it.

## Adding a new version

When a target application is built against a newer CUDA release than
anything above, drop the matching `libcupti.so.<version>` from that
toolkit's `extras/CUPTI/lib64/` (or `targets/<arch>-linux/lib/`) into this
directory, append its size, `sha256sum`, SONAME and CUDA Toolkit line to
the table above, and add its row to `NOTICE`. Extend the top-level `NOTICE`
if the SONAME family is new.

The file must ship the exact soname the injector will look up. Confirm
with:

    readelf -d libcupti.so.<version> | grep SONAME

The `SONAME` string is what `libcuprof.so`'s `DT_NEEDED` entry names.

Keep NVIDIA's file name unchanged: `libcupti.so.<a>.<b>.<c>`, with at least
three numeric components. The runtime ranking parses those components as a
version, and a name that does not follow the convention is ranked last
among the candidates for its SONAME rather than compared as a release (see
**Selecting a file at runtime** above).

## Pruning the set

Pruning is the build-time lever; `AIPROF_CUPTI_PREFER` (**Selecting a file
at runtime** above) is the runtime one. Image builds may drop any file
whose SONAME no shipped `libcuprof.so` needs;
`deploy/docker/Dockerfile.client` copies this whole directory to
`/opt/aiprof/cupti/`, so pruning shrinks the image directly — the ten files
add ~67 MB to a fresh clone. Keep at least one file per SONAME family
declared in `DT_NEEDED`.

If you keep several patch levels of one family, the ranking described above
decides which is staged — the highest release by default — and a newer CUPTI
patch level can require a newer driver. Check the minimum-driver column of
the CUDA Toolkit release notes for the release you intend to ship: drop the
newer files, or plan to run with `AIPROF_CUPTI_PREFER=lowest` or an exact
file name, if your targets run old drivers. The CUDA 9.0–11.7 copies exist
so a maintainer can rebuild `libcuprof.so` against an older `CUDA_HOME`
without a second download, and are prunable by this rule.

## Never modify a binary in place

The EULA's Linux redistribution grant covers unmodified object code only
(§2.3). Adding or removing whole files is fine; editing, stripping,
relinking or renaming one of NVIDIA's binaries is not, and would void the
basis on which this directory ships.

## License

Copyright NVIDIA Corporation. All rights reserved. Redistributed unmodified
under the NVIDIA CUDA Toolkit EULA — not under AIProf's Apache-2.0 licence.
See `NOTICE` in this directory for the retained notice and the section-level
redistribution basis, the top-level `NOTICE` for the repo-wide inventory,
and <https://docs.nvidia.com/cuda/eula/> for the full EULA text.
