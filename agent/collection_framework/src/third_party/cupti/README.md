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

## Contents and provenance

Each file was extracted from the corresponding NVIDIA CUDA Toolkit release
(the `cuda-cupti-<version>` package or the `nvidia/cuda:<tag>-devel-*`
image at `/usr/local/cuda/extras/CUPTI/lib64/`). CUPTI is a
redistributable component under the NVIDIA CUDA Toolkit EULA
(<https://docs.nvidia.com/cuda/eula/>); see the top-level `NOTICE` for the
redistribution declaration.

| File                     | Bytes    | SHA-256                                                          | CUDA Toolkit line |
|--------------------------|----------|------------------------------------------------------------------|--------------------|
| `libcupti.so.9.0.176`    |  5701080 | `0edfb9687babaa4e919de25a30d48dc21367ec2821dc85666ceb23a087dc8e63` | CUDA 9.0           |
| `libcupti.so.10.2.75`    |  5761360 | `cfb2777d5b0ff80a4bab2f2366f702490251f1afdc3290507c1641115b0c482d` | CUDA 10.2          |
| `libcupti.so.2020.1.0`   |  6489904 | `800c590127bce2f8d6a4a7d7694c5731669cc0ee4dfc2d8ccd15d646792ea5b1` | CUDA 11.0          |
| `libcupti.so.2021.2.2`   |  7306096 | `1cf95388ff8a012fb8f45fe2b10e8215a9244b204cd4ec29ab37c2c52fbaa24c` | CUDA 11.4          |
| `libcupti.so.2022.2.0`   |  7091568 | `6ac96871a53c07291c7cfe3d5541cd3bbed628fd1299129ca7c568fee333d2c8` | CUDA 11.7          |
| `libcupti.so.2023.2.1`   |  7526416 | `ade8c347db2fe510530854a924965176e25903c2cb31c4fc79d4f4bf9ac4ae1c` | CUDA 12.1          |
| `libcupti.so.2024.1.1`   |  7748112 | `fb2a7c5b15c84df9505dd47e553fe46f3121a57d30391fca24179d202f73f3f7` | CUDA 12.3          |
| `libcupti.so.2024.3.2`   |  8067920 | `76dcad94420820ea9aee9094c346cc34c6cb1ff630cef9b14e04db298a86305d` | CUDA 12.5          |
| `libcupti.so.2025.2.1`   |  7604800 | `2fdab19dc3fccdd4b2f5eba137aabefc68e685f6497bb38eea649866c5672a2a` | CUDA 12.8          |
| `libcupti.so.2025.3.1`   |  4152208 | `e2f9ed861fe27c492b8bb52b5e3220ef5120f3edcda36312e96b7fd8a186be3e` | CUDA 13.0          |

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

## Adding a new version

When a target application is built against a newer CUDA release than
anything above, drop the matching `libcupti.so.<version>` from that
toolkit's `extras/CUPTI/lib64/` into this directory, append its size and
`sha256sum` to the table, and note the CUDA Toolkit line.

The file must ship the exact soname the injector will look up. Confirm
with:

    readelf -d libcupti.so.<version> | grep SONAME

The `SONAME` string is what `libcuprof.so`'s `DT_NEEDED` entry names.

## License

Redistributed under the NVIDIA CUDA Toolkit EULA. See the top-level
`NOTICE` for the declaration and `<https://docs.nvidia.com/cuda/eula/>`
for the full text.
