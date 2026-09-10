__version__ = "0.2.37"

import threading as _threading
if not hasattr(_threading, "get_native_id"):
    import ctypes as _ctypes
    try:
        _libc = _ctypes.CDLL("libc.so.6", use_errno=True)

        # Prefer glibc's gettid() symbol (available in glibc >= 2.30).
        if hasattr(_libc, "gettid"):
            _libc.gettid.restype = _ctypes.c_int
            def _get_native_id():
                return _libc.gettid()
        else:
            # Fall back to syscall(SYS_gettid). The syscall number is
            # architecture-specific, so pick it based on the current machine.
            import platform as _platform
            _SYS_gettid_by_arch = {
                "x86_64":  186,
                "aarch64": 178,
            }
            _sys_gettid = _SYS_gettid_by_arch.get(_platform.machine())
            if _sys_gettid is not None:
                def _get_native_id(_nr=_sys_gettid):
                    try:
                        return _libc.syscall(_nr)
                    except Exception:
                        return _libc.pthread_self() & 0xffffffff
            else:
                def _get_native_id():
                    return _libc.pthread_self() & 0xffffffff

        _threading.get_native_id = _get_native_id
    except Exception:
        # If libc.so.6 fails to load, fall back to the Python thread ident.
        def _get_native_id():
            return _threading.current_thread().ident or 0
        _threading.get_native_id = _get_native_id


def _load_extension():
    from .native.pyki_extension import do_init
    do_init()


_load_extension()


from .profiling import *

from .logging import configure_logging
