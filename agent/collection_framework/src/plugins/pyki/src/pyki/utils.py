import functools
import logging
import math
import os
import platform
import threading

from importlib import import_module
from pyki.wrapt import ObjectProxy # type: ignore

_logger = logging.getLogger(__name__)


def run_once(f):
    called = False
    lock = threading.Lock()
    def wrapper(*args, **kwargs):
        nonlocal called
        if called:
            return None
        with lock:
            if called:
                return None
            called = True
        return f(*args, **kwargs)
    return wrapper


def require_os(os_list):
    if isinstance(os_list, str):
        os_list = [os_list]

    def decorator(f):
        @functools.wraps(f)
        def wrapper(*args, **kwargs):
            current_os = platform.system()
            if current_os not in os_list:
                raise NotImplementedError(f"This function is only implemented for {os_list} systems.")
            return f(*args, **kwargs)

        return wrapper

    return decorator


def unwrap(module_str, function_str):
    class_str = None
    if "." in function_str:
        try:
            class_str, function_str = function_str.split(".", 1)
        except ValueError as exc:
            raise ImportError(
                f"Cannot parse '{function_str}' as dotted class name and method"
            ) from exc
    module_or_class = import_module(module_str)
    if class_str is not None:
        try:
            module_or_class = getattr(module_or_class, class_str)
        except AttributeError as exc:
            raise ImportError(
                f"Cannot import '{class_str}' from '{module_str}'"
            ) from exc
    func = getattr(module_or_class, function_str, None)
    if func and isinstance(func, ObjectProxy) and hasattr(func, "__wrapped__"):
        setattr(module_or_class, function_str, func.__wrapped__)


class EnvironmentSetter:
    def __init__(self, key, value, force_override=False, enable=True):
        self.key = key
        self.value = value
        self.force_override = force_override
        self.original_value = None
        self.enable = enable

    def __enter__(self):
        if not self.enable:
            return
        self.original_value = os.getenv(self.key)
        if self.force_override or self.original_value is None:
            os.environ[self.key] = self.value

    def __exit__(self, exc_type, exc_value, traceback):
        if not self.enable:
            return
        if os.getenv(self.key) == self.value:
            if self.original_value is not None:
                os.environ[self.key] = self.original_value
            else:
                if self.key in os.environ:
                    del os.environ[self.key]


def parse_argument(args, kwargs, nth_arg, arg_name):
    """
    Parses a named argument from either kwargs or args.

    Args:
        args (tuple): Positional arguments passed to a function.
        kwargs (dict): Keyword arguments passed to a function.
        nth_arg (int): Index of the argument in args if not found in kwargs.
        arg_name (str): Name of the argument to look for.

    Returns:
        any: The value of the argument if found in either kwargs or args.
             Returns None if the argument is not found and no exception is raised.
    """

    if arg_name in kwargs:
        return kwargs[arg_name]
    elif nth_arg < len(args):
        return args[nth_arg]
    else:
        return None


def try_except(default_return=None):
    def decorator(wrapped):
        def wrapper(*args, **kwargs):
            try:
                return wrapped(*args, **kwargs)
            except Exception as e:
                _logger.warning(f"Failed to execute {wrapped.__name__}: {e}")
                return default_return

        return wrapper

    return decorator


_byte_units = ["Bytes", "KB", "MB", "GB", "TB", "PB"]


def format_bytes(size):
    size = max(size, 0)
    if size <= 1024:
        return f"{size} Bytes"

    i = int(math.log(size, 1024))
    return f"{size / (1024 ** i):.2f} {_byte_units[i]}"


class SingletonMeta(type):
    _instances = {}
    _lock = threading.Lock()

    def __call__(cls, *args, **kwargs):
        if cls not in cls._instances:
            with cls._lock:
                if cls not in cls._instances:
                    instance = super().__call__(*args, **kwargs)
                    cls._instances[cls] = instance
        return cls._instances[cls]
