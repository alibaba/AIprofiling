import argparse
import os
import sys

current_path = os.getcwd()
temp_dir = "temp_dir"


class Struct:
    def __init__(self, name, fields):
        self.name = name
        self.fields = fields


class Manifest:
    def __init__(self, version, max_micro, headers, structs):
        self.version = version
        self.max_micro = max_micro
        self.headers = headers
        self.structs = structs


def handle(cpython_path, manifest: Manifest):
    version = manifest.version
    print(f"Handle {version} at {cpython_path}")

    final_output = os.path.join(
        current_path, f"src/pyki/native/cpython_structs/{version}.hpp")

    for micro in range(manifest.max_micro + 1):
        full_version = f"{version}.{micro}"

        cpp_file = os.path.join(
            cpython_path, temp_dir, f"{full_version}_main.c")
        output = os.path.join(cpython_path, temp_dir, f"{full_version}.hpp")

        ret = os.system(
            f"""
            set -e
            cd {cpython_path}
            git checkout {full_version}

            # need to run configure on the current branch to generate pyconfig.h sometimes
            {"./configure prefix=" + os.path.abspath(os.path.join(cpython_path, version)) if micro == 0 else ""}
            """
        )

        if ret:
            return ret

        extra_includes = ""

        for header in manifest.headers:
            extra_includes += f'#include "{header}"\n'

        statements = '  printf("// Auto-generated file\\n");\n'

        statements += '  printf("#include <stddef.h>\\n\\n");\n'

        statements += f'  printf("namespace pyki::cpython_{version.replace(".", "_")} {{\\n\\n");\n'

        for struct in manifest.structs:
            sn = struct.name[1:] if struct.name.startswith(
                "_") else struct.name
            statements += f'  printf("size_t const {sn}_size = %zu;\\n\\n", sizeof({struct.name}));\n'
            for field in struct.fields:
                fn = field[1:] if field.startswith("_") else field
                fn = fn.replace(".", "_")
                statements += f'  printf("size_t const {sn}_{fn}_offset = %zu;\\n\\n", offsetof({struct.name}, {field}));\n'

        statements += '  printf("}");\n'
        statements += '  return 0;'

        program = f"""// Auto-generated file
#include <stddef.h>
#include <stdio.h>
#define Py_BUILD_CORE 1
#include "Python.h"
#undef HAVE_STD_ATOMIC
{extra_includes}
int main(void) {{
{statements}
}}"""

        with open(cpp_file, "w") as f:
            f.write(program)

        ret = os.system(
            f"""
            set -e
            cd {cpython_path}

            gcc -x c -o {temp_dir}/{full_version}_main {cpp_file} -I . -I ./Include -I ./Include/internal
        """
        )
        if ret:
            return ret

        ret = os.system(
            f"""
            set -e
            cd {cpython_path}

            ./{temp_dir}/{full_version}_main > {output}
        """
        )

        if micro == 0:
            with open(output, "r", encoding="utf-8") as f:
                content_base = f.read()

        if micro > 0:
            with open(output, "r", encoding="utf-8") as f:
                content = f.read()
                if content != content_base:
                    print(
                        f"Structure definitions for {full_version} differ from {version}.0"
                    )
                    return 1

    ret = os.system(
        f"""
        set -e
        cp {os.path.join(cpython_path, temp_dir, f"{version}.0.hpp")} {final_output}
        """
    )

    if ret:
        return ret


if __name__ == "__main__":
    default_cpython_path = os.path.join(
        os.getenv("HOME", "/"), "code", "cpython")

    parser = argparse.ArgumentParser(
        description="runs bindgen on cpython version",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    parser.add_argument(
        "--cpython",
        type=str,
        default=default_cpython_path,
        dest="cpython",
        help="path to cpython repo",
    )
    parser.add_argument(
        "--version",
        type=str,
        action="append",
        default=None,
        dest="versions",
        help="only process this Python version (e.g. '3.6' or 'v3.6'); "
             "may be repeated. Default: process all known versions.",
    )

    args = parser.parse_args()

    if not os.path.isdir(args.cpython):
        print(f"Directory '{args.cpython}' doesn't exist!")
        print("Pass a valid cpython path in with --cpython <pathname>")
        sys.exit(1)

    manifests = [
        Manifest(
            "v3.6",
            15,
            [
                "frameobject.h",
            ],
            [
                Struct(
                    "PyThreadState",
                    [
                        "frame",
                    ],
                ),
                Struct(
                    "PyFrameObject",
                    [
                        "f_code",
                        "f_back",
                        "f_lasti",
                    ],
                ),
                Struct(
                    "PyCodeObject",
                    [
                        "co_filename",
                        "co_name",
                        "co_lnotab",
                        "co_firstlineno",
                    ],
                ),
                Struct(
                    "PyUnicodeObject",
                    [
                        "_base",
                        "data.any"
                    ]
                ),
                Struct(
                    "PyCompactUnicodeObject",
                    [
                        "_base"
                    ]
                ),
                Struct(
                    "PyASCIIObject",
                    [
                        "length",
                        "state",
                    ]
                ),
                Struct(
                    "PyBytesObject",
                    [
                        "ob_base.ob_size",
                        "ob_sval",
                    ]
                ),
            ],
        ),

        Manifest(
            "v3.9",
            25,
            [
                "frameobject.h",
                "pycore_interp.h",
            ],
            [
                Struct(
                    "PyThreadState",
                    [
                        "frame",
                    ],
                ),
                Struct(
                    "PyFrameObject",
                    [
                        "f_code",
                        "f_back",
                        "f_lasti",
                    ],
                ),
                Struct(
                    "PyCodeObject",
                    [
                        "co_filename",
                        "co_name",
                        "co_lnotab",
                        "co_firstlineno",
                    ],
                ),
                Struct(
                    "PyUnicodeObject",
                    [
                        "_base",
                        "data.any"
                    ]
                ),
                Struct(
                    "PyCompactUnicodeObject",
                    [
                        "_base"
                    ]
                ),
                Struct(
                    "PyASCIIObject",
                    [
                        "length",
                        "state",
                    ]
                ),
                Struct(
                    "PyBytesObject",
                    [
                        "ob_base.ob_size",
                        "ob_sval",
                    ]
                ),
            ],
        ),

        Manifest(
            "v3.10",
            19,
            [
                "frameobject.h",
                "pycore_interp.h",
            ],
            [
                Struct(
                    "PyThreadState",
                    [
                        "frame",
                    ],
                ),
                Struct(
                    "PyFrameObject",
                    [
                        "f_code",
                        "f_back",
                        "f_lasti",
                    ],
                ),
                Struct(
                    "PyCodeObject",
                    [
                        "co_filename",
                        "co_name",
                        "co_linetable",
                        "co_firstlineno",
                    ],
                ),
                Struct(
                    "PyUnicodeObject",
                    [
                        "_base",
                        "data.any"
                    ]
                ),
                Struct(
                    "PyCompactUnicodeObject",
                    [
                        "_base"
                    ]
                ),
                Struct(
                    "PyASCIIObject",
                    [
                        "length",
                        "state",
                    ]
                ),
                Struct(
                    "PyBytesObject",
                    [
                        "ob_base.ob_size",
                        "ob_sval",
                    ]
                ),
            ],
        ),

        Manifest(
            "v3.11",
            14,
            [
                "frameobject.h",
                "pycore_interp.h",
                "pycore_frame.h",
            ],
            [
                Struct(
                    "PyThreadState",
                    [
                        "cframe",
                    ],
                ),
                Struct(
                    "_PyCFrame",
                    [
                        "current_frame",
                    ],
                ),
                Struct(
                    "_PyInterpreterFrame",
                    [
                        "f_code",
                        "previous",
                        "prev_instr",
                        "is_entry",
                    ],
                ),
                Struct(
                    "PyCodeObject",
                    [
                        "co_filename",
                        "co_name",
                        "co_code_adaptive",
                        "co_linetable",
                        "co_firstlineno",
                    ],
                ),
                Struct(
                    "PyUnicodeObject",
                    [
                        "_base",
                        "data.any"
                    ]
                ),
                Struct(
                    "PyCompactUnicodeObject",
                    [
                        "_base"
                    ]
                ),
                Struct(
                    "PyASCIIObject",
                    [
                        "length",
                        "state",
                    ]
                ),
                Struct(
                    "PyBytesObject",
                    [
                        "ob_base.ob_size",
                        "ob_sval",
                    ]
                ),
            ],
        ),

        Manifest(
            "v3.12",
            12,
            [
                "frameobject.h",
                "pycore_interp.h",
            ],
            [
                Struct(
                    "PyThreadState",
                    [
                        "cframe",
                    ],
                ),
                Struct(
                    "_PyCFrame",
                    [
                        "current_frame",
                    ],
                ),
                Struct(
                    "_PyInterpreterFrame",
                    [
                        "f_code",
                        "previous",
                        "prev_instr",
                        "owner",
                    ],
                ),
                Struct(
                    "PyCodeObject",
                    [
                        "co_filename",
                        "co_name",
                        "co_code_adaptive",
                        "co_linetable",
                        "co_firstlineno",
                    ],
                ),
                Struct(
                    "PyUnicodeObject",
                    [
                        "_base",
                        "data.any"
                    ]
                ),
                Struct(
                    "PyCompactUnicodeObject",
                    [
                        "_base",
                    ]
                ),
                Struct(
                    "PyASCIIObject",
                    [
                        "length",
                        "state",
                    ]
                ),
                Struct(
                    "PyBytesObject",
                    [
                        "ob_base.ob_size",
                        "ob_sval",
                    ]
                ),
            ],
        ),

    ]

    temp = os.path.join(args.cpython, temp_dir)
    os.makedirs(temp, exist_ok=True)

    if args.versions:
        wanted = {(v if v.startswith("v") else f"v{v}") for v in args.versions}
        known = {m.version for m in manifests}
        unknown = wanted - known
        if unknown:
            print(f"Unknown version(s): {sorted(unknown)}")
            print(f"Known versions: {sorted(known)}")
            sys.exit(1)
        selected = [m for m in manifests if m.version in wanted]
    else:
        selected = manifests

    for manifest in selected:
        if handle(args.cpython, manifest):
            print(f"Failed to handle {manifest.version}")
            sys.exit(1)

    # Rewrite the aggregator header to reflect all headers currently on disk,
    # not just the subset we generated this run — otherwise a targeted
    # `--version v3.6` run would drop the includes for the other versions.
    hpp = os.path.join(current_path, "src/pyki/native/cpython_structs.hpp")
    structs_dir = os.path.join(current_path, "src/pyki/native/cpython_structs")

    def _version_sort_key(name: str):
        # "v3.6" -> (3, 6)
        try:
            parts = name.lstrip("v").split(".")
            return tuple(int(p) for p in parts)
        except ValueError:
            print(f"Warning: Skipping invalid version file: {name}.hpp")
            return (float('inf'),)  # 将无效文件排在最后

    if os.path.exists(structs_dir):
        existing = sorted(
            (f[:-len(".hpp")] for f in os.listdir(structs_dir)
             if f.startswith("v") and f.endswith(".hpp")),
            key=_version_sort_key,
        )
    else:
        existing = []

    with open(hpp, "w") as f:
        f.write("// Auto-generated file\n\n")
        f.write("#pragma once\n\n")
        for version in existing:
            f.write(f'#include "cpython_structs/{version}.hpp"\n')
