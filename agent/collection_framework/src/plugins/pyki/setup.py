from setuptools import Extension, setup, find_packages
from setuptools.command.build_py import build_py
from setuptools.command.build_ext import build_ext
try:
    # Only present in setuptools >= 64 (Python >= 3.7). Older setuptools
    # (e.g. the 59.x line that still supports Python 3.6) doesn't ship it,
    # in which case we simply skip the editable-install customization —
    # non-editable installs still work.
    from setuptools.command.editable_wheel import editable_wheel, _TopLevelFinder
    _HAVE_EDITABLE_WHEEL = True
except ImportError:
    editable_wheel = None
    _TopLevelFinder = object
    _HAVE_EDITABLE_WHEEL = False
from collections.abc import Iterator
import glob
import os
import platform
import shutil
import atexit

_version = None
_version_location = "src/pyki/__init__.py"
with open(_version_location, "r") as file:
    line = file.readline().strip()
    if not line.startswith("__version__"):
        raise Exception("Cannot read version")
    _version = line.split("=")[1].strip()[1:-1]

system = platform.system()
assert system in ["Linux"], "not supported platform"

do_shadow = os.environ.get("PYKI_SHADOW", "1") == "1"
do_coverage = os.environ.get("PYKI_COVERAGE", "0") == "1"
debug = os.environ.get("PYKI_DEBUG", "0") == "1"
# If the logic here is modified, please synchronize to shadow_test.py
version_replaced = "v" + _version.replace(".", "_").replace("+", "_").replace("-", "_")
shadowed_package_name = f'pyki.{version_replaced}' if do_shadow else "pyki"


class TempFileChange:
    def __init__(self):
        self.changes = []

    def add_change(self, file, original):
        self.changes.append((file, original))

    def recover_changes(self):
        for file, original in self.changes:
            with open(file, "w") as f:
                f.write(original)


tempFileChange = TempFileChange()
atexit.register(tempFileChange.recover_changes)

class build_py_ext(build_py):

    def run(self):
        super().run()

        if do_shadow:
            self.shadow_pyki()

    def shadow_pyki(self):
        for root, dirs, files in os.walk(self.build_lib):
            for file in files:
                if file.endswith(".py") or file.endswith(".pyi"):
                    filepath = os.path.join(root, file)
                    with open(filepath, "r", encoding="utf-8") as f:
                        content = f.read()
                    new_content = content.replace("pyki.", f"{shadowed_package_name}.")
                    if new_content != content:
                        print(f"shadowing {filepath}")
                        with open(filepath, "w", encoding="utf-8") as f:
                            f.write(new_content)

        for root, dirs, files in os.walk("src/pyki"):
            for file in files:
                basename = os.path.basename(file)
                if basename.endswith(".cpp") or file.endswith(".h") or file.endswith(".hpp"):
                    filepath = os.path.join(root, file)
                    with open(filepath, "r", encoding="utf-8") as f:
                        content = f.read()
                    new_content = content.replace("namespace pyki", f"namespace pyki_{version_replaced}")
                    new_content = new_content.replace("pyki::", f"pyki_{version_replaced}::")
                    if basename == "shadow.hpp":
                        new_content = new_content.replace("pyki.", f"{shadowed_package_name}.")
                    if new_content != content:
                        print(f"shadowing {filepath}")
                        tempFileChange.add_change(filepath, content)
                        with open(filepath, "w", encoding="utf-8") as f:
                            f.write(new_content)

        build_lib_version_replaced = version_replaced.replace("pyki.", f"{shadowed_package_name}.")
        if os.path.exists(build_lib_version_replaced):
            shutil.rmtree(build_lib_version_replaced)
        shutil.move(os.path.join(self.build_lib, "pyki"),
                   build_lib_version_replaced)
        if not os.path.exists(os.path.join(self.build_lib, "native")):
            os.mkdir(os.path.join(self.build_lib, "native"))
        shutil.move(build_lib_version_replaced,
                    os.path.join(self.build_lib, "pyki", version_replaced))
        with open(os.path.join(self.build_lib, "pyki", "__init__.py"), "w") as f:
            f.write(
                f'from {shadowed_package_name}.profiling.profiler import *\n')
            f.write(
                f'from {shadowed_package_name}.logging import configure_logging\n')
            pass


class CustomBuildExt(build_ext):
    def initialize_options(self):
        super().initialize_options()
        # 强制使用本地 build 目录而不是临时目录
        if not self.build_temp:
            self.build_temp = "build"


if _HAVE_EDITABLE_WHEEL:
    from typing import Iterator, Tuple

    class CustomizedTopLevelFinder(_TopLevelFinder):
        def get_implementation(self) -> Iterator[Tuple[str, bytes]]:
            for file, content in super().get_implementation():
                yield (file, content)

    editable_wheel._TopLevelFinder = CustomizedTopLevelFinder

    class editable_wheel_with_redirector(editable_wheel):
        def _select_strategy(self, name, tag, build_lib):
            # The default mode is "lenient" - others are "strict" and "compat".
            # "compat" is deprecated. "strict" creates a tree of links to files in
            # the repo. It could be implemented, but we only handle the default
            # case for now.
            if self.mode is not None and self.mode != "lenient":
                raise RuntimeError(
                    "Only lenient mode is supported for editable "
                    f"install. Current mode is {self.mode}"
                )

            return CustomizedTopLevelFinder(self.distribution, name)
else:
    CustomizedTopLevelFinder = None
    editable_wheel_with_redirector = None

extra_compile_args = ["-std=c++17", "-fvisibility=hidden", "-fno-omit-frame-pointer", "-ffunction-sections", "-fdata-sections"]
extra_link_args = ["-Wl,-T../../third_party/profiler/link.ld", "-Wl,--version-script=./version.map", "-Wl,--gc-sections"]

if system == "Linux":
    extra_compile_args.append("-frecord-gcc-switches")

if do_coverage:
    extra_compile_args.extend(["--coverage", "-O0", "-fprofile-arcs", "-ftest-coverage"])
    extra_link_args.append("--coverage")
elif debug:
    extra_compile_args.extend(["-g", "-O0"])
    extra_link_args.extend(["-O0"]) # https://linux.die.net/man/1/ld
else:
    extra_compile_args.extend(["-g0", "-O3", "-DNDEBUG"])
    extra_link_args.extend(["-O3"])


setup(
    name='pyki',
    version=_version,
    description="Python Kit",
    python_requires=">=3.6, <3.15",

    classifiers=[
        "Development Status :: 1 - Planning",
        "License :: OSI Approved :: Apache Software License",
        "Operating System :: POSIX :: Linux",
        "Programming Language :: Python :: 3.6",
        "Programming Language :: Python :: 3.9",
        "Programming Language :: Python :: 3.10",
        "Programming Language :: Python :: 3.11",
        "Programming Language :: Python :: 3.12",
        "Programming Language :: Python :: Implementation :: CPython",
    ],

    package_dir={"": "src"},
    packages=find_packages(where="src", exclude=["sketch"]),

    ext_modules=[
        Extension(
            name=f"{shadowed_package_name}.native.pyki_extension",
            sources=glob.glob("src/pyki/native/*.cpp") + glob.glob("src/pyki/native/**/*.cpp"),
            extra_compile_args=extra_compile_args,
            extra_link_args=extra_link_args,
            extra_objects=["../../third_party/profiler/src/libprofiler.a",
                           f"src/pyki/native/rust/target/{'debug' if debug else 'release'}/libpyki_rust_lib.a"],
            include_dirs=["third-party/pybind11/include"]
        ),
    ],

    install_requires=[
        'dataclasses; python_version < "3.7"',
        'importlib_metadata; python_version < "3.8"',
    ],

    entry_points={
        "console_scripts": [f"pyki = {shadowed_package_name}.cli.cmd:main"]
    },

    cmdclass=dict(
        {
            "build_py": build_py_ext,
            "build_ext": CustomBuildExt,
        },
        # Python3.6: either way is ok
        #   rm -rf build/ dist/ && python setup.py bdist_wheel && pip install --force-reinstall --no-deps dist/pyki-*.whl
        #   PYKI_SHADOW=0 python setup.py develop
        # Python3.9+: pip install -e . --force-reinstall
        **({"editable_wheel": editable_wheel_with_redirector}
           if _HAVE_EDITABLE_WHEEL else {})
    ),
)
