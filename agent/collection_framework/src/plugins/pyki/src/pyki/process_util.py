import logging
import os
import subprocess
import sys

from pyki.error import raise_error
from pyki.native.pyki_extension import setns_wrapper
from pyki.utils import require_os

_logger = logging.getLogger(__name__)


@require_os(["Linux"])
def get_nspid(pid: int) -> int:
    nspid = pid
    path = f"/proc/{pid}/status"
    if not os.path.exists(path):
        return pid
    with open(path, "r") as f:
        for line in f:
            if line.startswith("NStgid:"):
                nspid = int(line.strip().split("\t")[-1])
    return nspid


@require_os(["Linux"])
def enter_ns(pid: int, nspid: int, nstype: str) -> None:
    path = f"/proc/{pid}/ns/{nstype}"
    # os.setns() was added in cpython 3.12. We wrap this api by ourself for compatibility.
    result = setns_wrapper(path)
    if result != 0:
        raise_error(f"Failed to enter namespace {nstype} of pid {pid}, errocode = {result}")


@require_os(["Linux"])
def is_process_exist(pid) -> bool:
    return os.path.exists(f"/proc/{pid}")


@require_os(["Linux"])
def get_process_commandline(pid) -> str:
    with open(f"/proc/{pid}/cmdline", "r") as f:
        cmdline = f.read().rstrip('\0')
    args = cmdline.split('\0')
    formatted_args = [f'"{arg}"' if ' ' in arg else arg for arg in args]
    return ' '.join(formatted_args)


@require_os(["Linux"])
def is_root():
    return os.getuid() == 0


@require_os(["Linux"])
def check_python_process(pid):
    if not is_process_exist(pid):
        raise_error(f"process {pid} does not exist", True)
    if not is_python_process(pid):
        raise_error(f"process {pid} is not a python process", True)


@require_os(["Linux"])
def is_python_process(pid):
    # FIXME: check cmdline is sometimes not correct
    # upstream orchestrator will check python process for us
    return True

    # cmdline = get_process_commandline(pid)
    # return "python" in cmdline


@require_os(["Linux"])
def get_rss() -> int:
    """Get the resident set size (RSS) of the current process."""
    pid = os.getpid()
    path = f"/proc/{pid}/status"
    if not os.path.exists(path):
        return -1
    try:
        with open(path, "r") as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    rss_kb = int(line.split()[1])
                    return rss_kb * 1024
    except:
        ...
    return -1


@require_os(["Linux"])
def find_lib(lib_name: str, pid: int = None):
    if pid is None:
        pid = os.getpid()
    maps_path = f"/proc/{pid}/maps"

    if not os.path.exists(maps_path):
        return None

    try:
        with open(maps_path, "r") as f:
            for line in f:
                # 每行格式类似于: 7f8b9c000000-7f8b9c021000 r--p 00000000 08:02 1234567 /path/to/library.so
                parts = line.strip().split()
                if len(parts) >= 6 and lib_name in parts[5]:
                    lib_path = parts[5]
                    # 确保返回的是一个存在的文件路径
                    if os.path.exists(lib_path):
                        return lib_path
    except Exception as e:
        return None
    return None


def run_python_script(script_name, *args):
    path = os.path.join(os.path.dirname(__file__), "scripts", script_name + ".py")
    path = os.path.abspath(path)
    assert os.path.isfile(path)
    command = [sys.executable, path]
    if command[0] is None or command[0] == "":
        raise_error("python executable not found")
    for arg in args:
        command.append(str(arg))
    _logger.debug(f'Running python script: {" ".join(command)}')
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, universal_newlines=True)
    stdout, stderr = process.communicate()
    if stdout is not None and stdout != "":
        _logger.info(f"stdout: \n{stdout}")
    if stderr is not None and stderr != "":
        _logger.info(f"stderr: \n{stderr}")
    if process.returncode != 0:
        raise_error(f'Failed to run python script: {" ".join(command)}')
