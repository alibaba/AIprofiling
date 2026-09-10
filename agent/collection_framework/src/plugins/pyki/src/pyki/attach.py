import os
import pyki.native.pyki_extension as pyki_extension
import socket

from pyki.error import raise_error
from pyki.process_util import check_python_process, get_nspid, is_root


def get_self_address():
    # 注意：这里直接硬编码/tmp而不是使用{tempfile.gettempdir()}，因为这个临时目录的位置会受到TMPDIR,TMP或者TEMP环境变量的影响.
    # 我们需要保证两边找到的临时目录一致
    pid = os.getpid()
    return f"/tmp/.pyki_pid_{pid}"

def get_target_address(pid):
    nspid = get_nspid(pid)
    if nspid == pid:
        return f"/tmp/.pyki_pid_{pid}"
    return f"/proc/{pid}/root/tmp/.pyki_pid_{nspid}"


def attach(pid: int):
    check_python_process(pid)

    address = get_target_address(pid)

    need_attach = False
    if not os.path.exists(address):
        need_attach = True
    else:
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.connect(address)
        except:
            need_attach = True
        finally:
            s.close()

    if need_attach:
        if not is_root():
            raise_error("pyki must be run as root user to attach target process", True)
        if not pyki_extension.attach(pid):
            raise_error(f"Cannot attach Process {pid}", True)

    return address
