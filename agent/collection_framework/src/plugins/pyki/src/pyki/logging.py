from typing import List, Optional, Union
import logging
import os.path
import sys
import tempfile


_pyki_root_logger = logging.getLogger("pyki")
_native_logger = logging.getLogger("pyki.native")


_NATIVE_LOG_LEVEL_DEBUG = 0
_NATIVE_LOG_LEVEL_INFO = 1
_NATIVE_LOG_LEVEL_WARNING = 2
_NATIVE_LOG_LEVEL_ERROR = 3
_NATIVE_LOG_LEVEL_OFF = 4


_python_level_to_native_level = {
    logging.DEBUG: _NATIVE_LOG_LEVEL_DEBUG,
    logging.INFO: _NATIVE_LOG_LEVEL_INFO,
    logging.WARNING: _NATIVE_LOG_LEVEL_WARNING,
    logging.ERROR: _NATIVE_LOG_LEVEL_ERROR,
}


_native_level_to_python_level = {
    _NATIVE_LOG_LEVEL_DEBUG: logging.DEBUG,
    _NATIVE_LOG_LEVEL_INFO: logging.INFO,
    _NATIVE_LOG_LEVEL_WARNING: logging.WARNING,
    _NATIVE_LOG_LEVEL_ERROR: logging.ERROR,
}


def _disable_native_logging():
    from pyki.native import pyki_extension
    pyki_extension.set_native_log_level(_NATIVE_LOG_LEVEL_OFF)


def _set_native_log_level(level):
    from pyki.native import pyki_extension
    pyki_extension.set_native_log_level(
        _python_level_to_native_level.get(level, _NATIVE_LOG_LEVEL_OFF))


def _native_log(level, timestamp, filename, function, lineno, message):
    level = _native_level_to_python_level.get(level, None)
    if level is None:
        return

    record = _native_logger.makeRecord(
        name=_native_logger.name,
        level=level,
        fn=filename,
        func=function,
        lno=lineno,
        msg=message,
        args=(),
        exc_info=None,
    )
    record.created = timestamp / 1000.0
    _native_logger.handle(record)


def _flush_native_logs():
    from pyki.native import pyki_extension
    pyki_extension.flush_native_logs()


def get_log_path(pid=None):
    if pid is None:
        pid = os.getpid()
    return os.path.join(tempfile.gettempdir(), f"pyki_{pid}.log")


def _init_pyki_logger():
    _pyki_root_logger.propagate = False
    _pyki_root_logger.setLevel(logging.DEBUG)
    _set_native_log_level(logging.INFO)

    format = '%(asctime)s [PID:%(process)d] [%(filename)s:%(funcName)s:%(lineno)s] [%(levelname)s] %(message)s'
    handler: logging.Handler = logging.FileHandler(get_log_path(), delay=True)
    handler.setFormatter(logging.Formatter(format))
    handler.setLevel(logging.DEBUG)
    _pyki_root_logger.addHandler(handler)

    handler = logging.StreamHandler(sys.stdout)
    handler.setFormatter(logging.Formatter(format))
    handler.setLevel(logging.INFO)

    def filter(_: logging.LogRecord) -> bool:
        return not sys.stdout.closed

    handler.addFilter(filter)

    original_handle_error = handler.handleError

    def handler_error(record: logging.LogRecord):
        if sys.stdout.closed:
            return
        original_handle_error(record)

    handler.handleError = handler_error # type: ignore[method-assign]

    _pyki_root_logger.addHandler(handler)

    # import threading
    # threading.Thread(target=_flush_native_logs, daemon=True).start()


_init_pyki_logger()


def configure_logging(level: Optional[int] = None, handlers: Optional[Union[logging.Handler, List[logging.Handler]]] = None):
    if _pyki_root_logger.handlers:
        for old_handler in list(_pyki_root_logger.handlers):
            _pyki_root_logger.removeHandler(old_handler)

    try:
        if os.path.exists(get_log_path()):
            os.remove(get_log_path())
    except:
        ...

    _pyki_root_logger.propagate = True

    if level is not None:
        _pyki_root_logger.setLevel(level)
        _set_native_log_level(level)

    if handlers is not None:
        if isinstance(handlers, list):
            for handler in handlers:
                _pyki_root_logger.addHandler(handler)
        elif isinstance(handlers, logging.Handler):
            _pyki_root_logger.addHandler(handlers)
    else:
        _pyki_root_logger.addHandler(logging.NullHandler())
