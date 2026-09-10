import logging
import os
import time
from types import FunctionType
from enum import Enum

import pyki
from pyki.module_util import is_package_installed
from pyki.wrapt import wrap_function_wrapper # type: ignore
from pyki.native.pyki_extension import get_chrome_trace_base_time
from pyki.profiling.chrome_trace import ChromeTraceEvent
from pyki.error import raise_error
from pyki.utils import unwrap, parse_argument, try_except
from pyki.packaging import version
from pyki.module_util import get_package_version
from abc import ABC, abstractmethod
from importlib import import_module
from typing import Optional, TypedDict, Callable, List
import threading

_logger = logging.getLogger(__name__)


class _TracerState(Enum):
    NOT_STARTED = 0
    STARTED = 1
    STOPPED = 2


class NVTXTracer:
    def __init__(self):

        self.initiated = False
        self.hooks: Optional[List] = None  # lazy init
        self.state: _TracerState = _TracerState.NOT_STARTED
        self.recorder = _NVTXRecorder()
        self.nvtx_lib_enabled = not (os.environ.get("NVTX_INJECTION64_PATH") is None and
                                     os.environ.get("NVTX_INJECTION32_PATH") is None)
        self.nv_nvtx_python_lib_enabled = self.nvtx_lib_enabled and not os.getenv("NVTX_DISABLE", False)

    def _initiate(self):
        self.hooks = [_PytorchNVTXHook(), _NvidiaNVTXHook()]
        self.hooks = [hook for hook in self.hooks if hook.enabled]


    def start_tracing(self):
        if self.state != _TracerState.NOT_STARTED:
            raise_error("NVTX tracer is already started")
        if not self.initiated:
            self.initiated = True
            self._initiate()

        self.state = _TracerState.STARTED
        self.recorder.on_recorder_start()
        assert self.hooks is not None
        for hook in self.hooks:
            hook.activate()

    def stop_tracing(self):
        if self.state != _TracerState.STARTED:
            raise_error("NVTX tracer is not started")
        self.state = _TracerState.STOPPED
        self._deactivate_hooks()

    def _deactivate_hooks(self):
        assert self.hooks is not None
        for hook in self.hooks:
            if hook.active:
                hook.deactivate()

    def reset(self):
        # in case not stopped
        self._deactivate_hooks()
        self.state = _TracerState.NOT_STARTED
        self.recorder.reset()

    def export_data(self, path):
        if self.state != _TracerState.STOPPED:
            raise_error("NVTX tracer must be stopped before exporting data")
        chrome_trace = self.recorder.to_chrome_trace()
        chrome_trace.write_to_file(path)


class _TraceEvent(TypedDict):
    tid: int
    message: str
    start: float
    duration: float


class _ThreadInfo(TypedDict):
    tid: int
    name: str


class _NVTXRecorder:
    def __init__(self):
        self.start_time = time.time()
        self.tls = threading.local()
        self.pending_started_ranges = {}
        self.started_ranges_lock = threading.Lock()
        self.next_range_id = 0

        self.events: list[_TraceEvent] = []
        self.thread_info: list[_ThreadInfo] = []

    def on_recorder_start(self):
        self.start_time = time.time()

    @try_except()
    def on_range_push(self, message):
        if not isinstance(message, str):
            message = str(message)
        if not hasattr(self.tls, "pending_pushed_ranges"):
            self.tls.pending_pushed_ranges = []
        self.tls.pending_pushed_ranges.append({
            "tid": threading.get_native_id(),
            "message": message,
            "start": time.time(),
            "duration": 0,
        })

    @try_except()
    def on_range_pop(self):
        pending_pushed_ranges = getattr(self.tls, "pending_pushed_ranges", None)
        if pending_pushed_ranges is None or len(pending_pushed_ranges) == 0:
            return
        event = pending_pushed_ranges.pop()
        event["duration"] = time.time() - event["start"]
        # cpython guarantees list.append is atomic
        self.events.append(event)
        self.check_thread_info()

    @try_except(default_return=0)
    def on_range_start(self, message, range_id):
        if not isinstance(message, str):
            message = str(message)
        if isinstance(range_id, tuple):
            range_id = range_id[0]
        # Caller should set range_id is none if nvtx lib is not enabled, then this function will
        # allocate a new id and return the auto-increment range id. This way of generating id
        # is the same as nvtx lib.
        with self.started_ranges_lock:
            if range_id is None:
                range_id = self.next_range_id
                self.next_range_id += 1
            self.pending_started_ranges[range_id] = {
                "tid": None,  # use the thread of range end
                "message": message,
                "start": time.time(),
                "duration": 0,
            }
        return range_id

    @try_except()
    def on_range_end(self, range_id):
        if range_id is None:
            return
        if isinstance(range_id, tuple):
            range_id = range_id[0]
        with self.started_ranges_lock:
            if range_id not in self.pending_started_ranges:
                return
            event = self.pending_started_ranges.pop(range_id)
        event["duration"] = time.time() - event["start"]
        event["tid"] = threading.get_native_id()
        self.events.append(event)
        self.check_thread_info()

    @try_except()
    def on_mark(self, message):
        if not isinstance(message, str):
            message = str(message)
        self.events.append({
            "tid": threading.get_native_id(),
            "message": message,
            "start": time.time(),
            "duration": 0,
        })
        self.check_thread_info()

    def reset(self):
        self.tls = threading.local()
        self.pending_started_ranges = {}
        self.thread_info = []
        self.events = []

    def check_thread_info(self):
        if getattr(self.tls, "thread_info_recorded", False):
            return
        self.tls.thread_info_recorded = True
        self.thread_info.append({
            "tid": threading.get_native_id(),
            "name": threading.current_thread().name,
        })

    def to_chrome_trace(self) -> ChromeTraceEvent:
        trace = ChromeTraceEvent()
        trace.add_metadata("displayTimeUnit", "ms")
        trace.add_metadata("pyki_trace_type", "nvtx")
        trace.add_metadata("with_nvtx", 1)
        trace.set_base_time(get_chrome_trace_base_time() / 1e9)
        for event in self.events:
            trace.add_complete_event(
                category="nvtx",
                name=event["message"],
                pid=0,  # put nvtx data together with gpu kernels
                tid=event["tid"] + 10000000,
                ts=event["start"],
                duration=event["duration"],
            )
        for thread_info in self.thread_info:
            trace.add_thread_info(
                thread_name=f"thread {thread_info['tid']} NVTX",
                pid=0,
                tid=thread_info["tid"] + 10000000,
                ts=self.start_time,
            )
        return trace


def noop(*args, **kwargs):
    pass


class NVTXFunctionsClone:
    torch_range_push = noop
    torch_range_pop = noop
    torch_range_start = noop
    torch_range_end = noop
    torch_mark = noop
    nvidia_range_push = noop
    nvidia_range_pop = noop
    nvidia_range_start = noop
    nvidia_range_end = noop
    nvidia_mark = noop


def torch_range_push_hooked(msg):
    try:
        # The namespace used in the hooks is actually the namespace of the original
        # functions, so we reimport pyki here
        import pyki
        tracer = pyki.profiling.nvtx_tracer.nvtx_tracer_instance
        tracer.recorder.on_range_push(msg)
        if tracer.nvtx_lib_enabled:
            return pyki.profiling.nvtx_tracer.NVTXFunctionsClone.torch_range_push(msg)
        else:
            # don't call into original function, in case any api change
            return None
    except Exception as e:
        pass


def torch_range_pop_hooked():
    try:
        import pyki
        tracer = pyki.profiling.nvtx_tracer.nvtx_tracer_instance
        tracer.recorder.on_range_pop()
        if tracer.nvtx_lib_enabled:
            return pyki.profiling.nvtx_tracer.NVTXFunctionsClone.torch_range_pop()
        else:
            return None
    except Exception as e:
        pass

def torch_range_start_hooked(msg):
    try:
        import pyki
        range_id = None
        tracer = pyki.profiling.nvtx_tracer.nvtx_tracer_instance
        if tracer.nvtx_lib_enabled:
            range_id = pyki.profiling.nvtx_tracer.NVTXFunctionsClone.torch_range_start(msg)
        tracer = pyki.profiling.nvtx_tracer.nvtx_tracer_instance
        # when nvtx lib is not enabled, we take responsibility of generating range id
        if not tracer.nvtx_lib_enabled:
            range_id = tracer.recorder.on_range_start(msg, range_id)
        return range_id
    except Exception as e:
        pass


def torch_range_end_hooked(range_id):
    try:
        import pyki
        tracer = pyki.profiling.nvtx_tracer.nvtx_tracer_instance
        tracer.recorder.on_range_end(range_id)
        if tracer.nvtx_lib_enabled:
            return pyki.profiling.nvtx_tracer.NVTXFunctionsClone.torch_range_end(range_id)
        else:
            return None
    except Exception as e:
        pass


def torch_mark_hooked(msg):
    try:
        import pyki
        tracer = pyki.profiling.nvtx_tracer.nvtx_tracer_instance
        tracer.recorder.on_mark(msg)
        if tracer.nvtx_lib_enabled:
            return pyki.profiling.nvtx_tracer.NVTXFunctionsClone.torch_mark(msg)
        else:
            return None
    except Exception as e:
        pass


def nvidia_range_push_hooked(*args, **kwargs):
    try:
        import pyki
        tracer = pyki.profiling.nvtx_tracer.nvtx_tracer_instance
        message = pyki.utils.parse_argument(args, kwargs, 0, "message")
        tracer.recorder.on_range_push(message)
        if tracer.nv_nvtx_python_lib_enabled:
            return pyki.profiling.nvtx_tracer.NVTXFunctionsClone.nvidia_range_push(*args, **kwargs)
        else:
            return None
    except Exception as e:
        pass


def nvidia_range_pop_hooked(*args, **kwargs):
    try:
        import pyki
        tracer = pyki.profiling.nvtx_tracer.nvtx_tracer_instance
        tracer.recorder.on_range_pop()
        if tracer.nv_nvtx_python_lib_enabled:
            return pyki.profiling.nvtx_tracer.NVTXFunctionsClone.nvidia_range_pop(*args, **kwargs)
        else:
            return None
    except Exception as e:
        pass


def nvidia_range_start_hooked(*args, **kwargs):
    try:
        import pyki
        import nvtx
        tracer = pyki.profiling.nvtx_tracer.nvtx_tracer_instance
        range_id = None
        if tracer.nvtx_lib_enabled:
            range_id = pyki.profiling.nvtx_tracer.NVTXFunctionsClone.nvidia_range_start(*args, **kwargs)

        message = pyki.utils.parse_argument(args, kwargs, 0, "message")
        # when nvtx lib is not enabled, we take responsibility of generating range id
        if tracer.nv_nvtx_python_lib_enabled:
            tracer.recorder.on_range_start(message, range_id)
        else:
            range_id = tracer.recorder.on_range_start(message, None)
        return range_id
    except Exception as e:
        pass


def nvidia_range_end_hooked(range_id) -> None:
    try:
        import pyki

        tracer = pyki.profiling.nvtx_tracer.nvtx_tracer_instance
        tracer.recorder.on_range_end(range_id)
        if tracer.nv_nvtx_python_lib_enabled:
            return pyki.profiling.nvtx_tracer.NVTXFunctionsClone.nvidia_range_end(range_id)
        else:
            return None
    except Exception as e:
        pass


def nvidia_mark_hooked(*args, **kwargs):
    try:
        import pyki
        tracer = pyki.profiling.nvtx_tracer.nvtx_tracer_instance
        message = pyki.utils.parse_argument(args, kwargs, 0, "message")
        tracer.recorder.on_mark(message)
        if tracer.nv_nvtx_python_lib_enabled:
            return pyki.profiling.nvtx_tracer.NVTXFunctionsClone.nvidia_mark(*args, **kwargs)
        else:
            return None
    except Exception as e:
        pass


class _HookDescription(TypedDict):
    function: str
    hooked: Callable
    clone_name: str


class _BaseNVTXHook(ABC):
    def __init__(self, module):
        self.module = None
        self.enabled = True
        self.active = False
        self.hooks_descriptions = None
        self.wrapt_hooks = []

        try:
            self.module = import_module(module)
        except ImportError:
            _logger.info(f"Fail to import {module}, {self.__class__.__name__} will not be enabled")
            self.enabled = False
            self.module = None

    @abstractmethod
    def get_hooks_descriptions(self) -> List[_HookDescription]:
        pass

    def activate(self):
        if not self.enabled:
            return

        if self.active:
            _logger.warning(f"{self.__class__.__name__} already activated")
            return

        if self.hooks_descriptions is None:
            self.hooks_descriptions = self.get_hooks_descriptions()
        try:
            for hook_description in self.hooks_descriptions:
                function = getattr(self.module, hook_description["function"], None)
                if function is None:
                    continue
                assert isinstance(function, FunctionType), "other case not supported"
                function_clone = FunctionType(function.__code__,
                                              function.__globals__,
                                              name=function.__name__,
                                              argdefs=function.__defaults__)
                setattr(NVTXFunctionsClone, hook_description["clone_name"], function_clone)
                function.__code__ = hook_description["hooked"].__code__
            self.active = True
            _logger.info(f"{self.__class__.__name__} activated")
        except Exception as e:
            _logger.warning(f"Fail to activate {self.__class__.__name__}: {e}")
            self.deactivate()

    def deactivate(self):
        if not self.enabled:
            return

        if not self.active:
            _logger.warning(f"{self.__class__.__name__} not activated")
            return

        assert self.hooks_descriptions is not None
        for hook_description in self.hooks_descriptions:
            try:
                function = getattr(self.module, hook_description["function"], None)
                if function is None:
                    continue
                function_clone = getattr(NVTXFunctionsClone, hook_description["clone_name"])
                if nvtx_tracer_instance.nvtx_lib_enabled:
                    if function_clone is not None and function_clone != noop:
                        function.__code__ = function_clone.__code__
                        setattr(NVTXFunctionsClone, hook_description["clone_name"], noop)
                else:
                    # make nvtx api noop forever to prevent enter/exit mismatch
                    function.__code__ = noop.__code__
            except Exception as e:
                _logger.warning(f"Fail to deactivate {self.__class__.__name__}: {e}")

        for wrapt_hook in self.wrapt_hooks:
            try:
                module, function = wrapt_hook
                unwrap(module, function)
            except Exception as e:
                _logger.warning(f"Fail to deactivate wrapt hook {self.__class__.__name__}: {e}")

        self.wrapt_hooks = []

        self.active = False
        _logger.info(f"{self.__class__.__name__} deactivated")

    def activate_wrapt_hook(self, module, function, wrapper):
        wrap_function_wrapper(module, function, wrapper)
        self.wrapt_hooks.append((module, function))


class _PytorchNVTXHook(_BaseNVTXHook):
    def __init__(self):
        super().__init__("torch.cuda.nvtx")

    def get_hooks_descriptions(self) -> List[_HookDescription]:
        return [
            {
                "function": "range_push",
                "hooked": torch_range_push_hooked,
                "clone_name": "torch_range_push"
            },
            {
                "function": "range_pop",
                "hooked": torch_range_pop_hooked,
                "clone_name": "torch_range_pop"
            },
            {
                "function": "range_start",
                "hooked": torch_range_start_hooked,
                "clone_name": "torch_range_start"
            },
            {
                "function": "range_end",
                "hooked": torch_range_end_hooked,
                "clone_name": "torch_range_end"
            },
            {
                "function": "mark",
                "hooked": torch_mark_hooked,
                "clone_name": "torch_mark"
            }
        ]


class _NvidiaNVTXHook(_BaseNVTXHook):
    def __init__(self):
        # we will collect data even if NVTX_DISABLE is set
        super().__init__("nvtx")
        self.nvtx_python_lib_enabled = not os.getenv("NVTX_DISABLE", False)
        self.enabled = self.enabled and self._is_supported_version()

    def get_hooks_descriptions(self) -> List[_HookDescription]:
        return [
            {
                "function": "push_range",
                "hooked": nvidia_range_push_hooked,
                "clone_name": "nvidia_range_push"
            },
            {
                "function": "pop_range",
                "hooked": nvidia_range_pop_hooked,
                "clone_name": "nvidia_range_pop"
            },
            {
                "function": "start_range",
                "hooked": nvidia_range_start_hooked,
                "clone_name": "nvidia_range_start"
            },
            {
                "function": "end_range",
                "hooked": nvidia_range_end_hooked,
                "clone_name": "nvidia_range_end"
            },
            {
                "function": "mark",
                "hooked": nvidia_mark_hooked,
                "clone_name": "nvidia_mark"
            }
        ]

    def _is_supported_version(self):
        nvtx_version = get_package_version("nvtx")
        if nvtx_version is None:
            _logger.info(f"Fail to get nvtx version,  {self.__class__.__name__} will not be enabled")
            return False
        nvtx_version_parsed = version.parse(nvtx_version)
        v1, v2 = "0.2.5", "0.2.14"
        if nvtx_version_parsed < version.parse(v1) or nvtx_version_parsed > version.parse(v2):
            _logger.info(f"nvtx version {nvtx_version} is not supported, {self.__class__.__name__} will not be enabled")
            return False
        return True

    @staticmethod
    def get_nvtx_annotate_message(annotate):
        message = ""
        if hasattr(annotate, "init_args"):
            message = annotate.init_args[0]
        elif hasattr(annotate, "attributes") and hasattr(annotate.attributes, "message"):
            message = annotate.attributes.message
            if hasattr(message, "string"):
                message = message.string
        return message

    def activate(self):
        if not self.enabled or self.active:
            return

        super().activate()
        hook_nvtx_annotate_decorator()

        def annotate_enter_wrapper(call_wrapped, instance, args, kwargs):
            try:
                message = _NvidiaNVTXHook.get_nvtx_annotate_message(instance)
                nvtx_tracer_instance.recorder.on_range_push(message)
            except Exception as e:
                pass
            return call_wrapped(*args, **kwargs)

        def annotate_exit_wrapper(call_wrapped, instance, args, kwargs):
            nvtx_tracer_instance.recorder.on_range_pop()
            return call_wrapped(*args, **kwargs)

        def annotate_decorator_pre_hook_wrapper(call_wrapped, instance, args, kwargs):
            message = _NvidiaNVTXHook.get_nvtx_annotate_message(args[0])
            nvtx_tracer_instance.recorder.on_range_push(message)
            return call_wrapped(*args, **kwargs)

        def annotate_decorator_post_hook_wrapper(call_wrapped, instance, args, kwargs):
            nvtx_tracer_instance.recorder.on_range_pop()
            return call_wrapped(*args, **kwargs)

        try:
            self.activate_wrapt_hook("nvtx", "annotate.__enter__", annotate_enter_wrapper)
            self.activate_wrapt_hook("nvtx", "annotate.__exit__", annotate_exit_wrapper)
            self.activate_wrapt_hook("pyki", "nvtx_annotate_decorator_pre_hook",
                                     annotate_decorator_pre_hook_wrapper)
            self.activate_wrapt_hook("pyki", "nvtx_annotate_decorator_post_hook",
                                     annotate_decorator_post_hook_wrapper)

        except Exception as e:
            _logger.warning(f"Fail to activate wrapt hook {self.__class__.__name__}: {e}")


nvtx_tracer_instance = NVTXTracer()


def hook_nvtx_annotate_decorator():
    # 即使热更新了，这个函数也只hook一次
    if getattr(pyki, "nvtx_annotate_decorator_hooked", False):
        return
    try:
        # 这个函数在进程启动的时候调用，所以检查是否安装了包而非是否导入了包
        if not is_package_installed("nvtx"):
            return

        def nvtx_annotate_decorator_pre_hook(instance):
            pass

        def nvtx_annotate_decorator_post_hook(instance):
            pass

        setattr(pyki, "nvtx_annotate_decorator_pre_hook", nvtx_annotate_decorator_pre_hook)
        setattr(pyki, "nvtx_annotate_decorator_post_hook", nvtx_annotate_decorator_post_hook)

        # 给call创建空的前后hook点，并挂在pyki模块上，这样热更新后可以有固定的hook点
        def annotate_call_wrapper(call_wrapped, instance, args1, kwargs1):
            func = call_wrapped(*args1, **kwargs1)

            def inner(*args2, **kwargs2):
                getattr(pyki, "nvtx_annotate_decorator_pre_hook")(instance)
                res = func(*args2, **kwargs2)
                getattr(pyki, "nvtx_annotate_decorator_post_hook")(instance)
                return res

            return inner

        wrap_function_wrapper("nvtx", "annotate.__call__", annotate_call_wrapper)

        if os.getenv("NVTX_DISABLE", False):
            # 这种情况下annotate不会保存message，所以我们帮他们保存一下
            def annotate_init_wrapper(call_wrapped, instance, args, kwargs):
                try:
                    message = parse_argument(args, kwargs, 0, "message")
                    instance.init_args = (message,)
                    setattr(instance, "enter_result", None) # 兼容老版本nvtx
                except Exception as e:
                    pass
                return call_wrapped(*args, **kwargs)

            wrap_function_wrapper("nvtx", "annotate.__init__", annotate_init_wrapper)

    finally:
        setattr(pyki, "nvtx_annotate_decorator_hooked", True)
