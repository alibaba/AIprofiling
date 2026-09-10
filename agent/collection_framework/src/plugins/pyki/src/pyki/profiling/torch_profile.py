import logging
import math
import os
import threading
import time
import torch
import sys
import pickle

from abc import ABC, abstractmethod
from pyki.packaging import version
from typing import Any, Callable, Optional, List, Union
from threading import current_thread, Lock, main_thread
from torch.utils.hooks import RemovableHandle
from torch.optim.optimizer import register_optimizer_step_post_hook
from torch.profiler import ProfilerActivity, record_function
from pyki.wrapt import wrap_function_wrapper # type: ignore
from pyki.process_util import find_lib


from pyki.error import PyKiError, raise_error
from pyki.module_util import is_module_imported, get_cuda_version, get_package_version
from pyki.native import pyki_extension as pe
from pyki.native.pyki_extension import add_pending_call_for_torch_profile_module, hook_record_function_fast, \
    cuptiSetThreadIdType_wrapper
from pyki.profiling.nvtx_tracer import nvtx_tracer_instance
from pyki.scripts.pyki_merge_traces import merge_files
from pyki.utils import EnvironmentSetter, run_once, unwrap


_logger = logging.getLogger(__name__)



PC_ORIGIN = "ORIGIN"
PC_RUNNING = "RUNNING"
PC_FINISHED = "FINISHED"
PC_CANCELLED = "CANCELLED"


class _PendingCall:

    lock = threading.Lock()
    pending_call_of_main_thread: Optional["_PendingCall"] = None
    need_to_call_Py_AddPendingCall = True

    def __init__(self, func):
        self.lock = threading.Lock()
        self.state = PC_ORIGIN
        self.state_condition = threading.Condition(self.lock)
        self.func = func
        self.main_thread = False

    def __call__(self, *args, **kwargs):
        with self.lock:
            if self.state is not PC_ORIGIN:
                return
            self.state = PC_RUNNING
        try:
            self.func(*args, **kwargs)
        except:
            _logger.error(f"Failed to execute pending call.", exc_info=True)
        finally:
            with self.lock:
                self.state = PC_FINISHED
                self.state_condition.notify_all()

    def try_to_cancel(self):
        with self.lock:
            if self.state is PC_ORIGIN:
                self.state = PC_CANCELLED
                if self.main_thread:
                    with _PendingCall.lock:
                        if self is _PendingCall.pending_call_of_main_thread:
                            _PendingCall.pending_call_of_main_thread = None
                return True
            return False

    def cancel_or_wait(self):
        if self.try_to_cancel():
            return
        with self.lock:
            while self.state is PC_RUNNING:
                self.state_condition.wait()

    def is_origin(self):
        with self.lock:
            return self.state == PC_ORIGIN

    def is_cancelled(self):
        with self.lock:
            return self.state == PC_CANCELLED

    @staticmethod
    def register_for_main_thread(call) -> bool:
        with call.lock:
            call.main_thread = True
            try:
                with _PendingCall.lock:
                    if _PendingCall.need_to_call_Py_AddPendingCall:
                        if add_pending_call_for_torch_profile_module():
                            _PendingCall.pending_call_of_main_thread = call
                            _PendingCall.need_to_call_Py_AddPendingCall = False
                            return True
                    else:
                        _PendingCall.pending_call_of_main_thread = call
                        return True
            except:
                ...
            return False

    @staticmethod
    def invoke():
        while True:
            with _PendingCall.lock:
                call = _PendingCall.pending_call_of_main_thread
                if call is None:
                    _PendingCall.need_to_call_Py_AddPendingCall = True
                    return
                else:
                    _PendingCall.pending_call_of_main_thread = None

            call()


def _pending_call(func):
    return _PendingCall(func)


def _pending_call_if(predicate):
    def decorator(func):
        return _PendingCall(func) if predicate else func
    return decorator


def _invoke_pending_call():
    _PendingCall.invoke()


class _TorchFeatureSupport:
    def __init__(self):
        self.support_dict = {}
        self._check_supports()

    def _check_supports(self):
        experiment_config_args = {
            "cpu_op_probability": 0.1,
            "profile_all_threads": True,
        }
        for key, value in experiment_config_args.items():
            self.support_dict[f"experiment_config.{key}"] = self._experiment_config_support_field(
                key, value)

    def _experiment_config_support_field(self, field_name, filed_value):
        # can not use hasattr to check because experimental_config is a special class created by pybind11
        config_kwargs = {}
        config_kwargs[field_name] = filed_value
        try:
            _ = torch._C._profiler._ExperimentalConfig(**config_kwargs) # type: ignore
        except Exception:
            return False
        return True

    def is_feature_supported(self, feature_name):
        return self.support_dict.get(feature_name, False)

class AdjustSwitchInterval:
    def __init__(self, interval: float):
        self.target_interval = interval
        self.original_interval: float = 0

    def __enter__(self):
        if self.target_interval == 0:
            return
        self.original_interval = sys.getswitchinterval()
        _logger.info(f"Adjusting switch interval to {self.target_interval}.")
        sys.setswitchinterval(self.target_interval)

    def __exit__(self, exc_type, exc_val, exc_tb):
        if self.target_interval == 0 or sys.getswitchinterval() != self.target_interval:
            return
        _logger.info(f"Adjusting switch interval to {self.original_interval}.")
        sys.setswitchinterval(self.original_interval)

class _Arguments:

    # Notice: arguments below except callbacks should be sync with torch_profile() in command.py
    def __init__(self,
                 *,
                 path: str = "",
                 prefix: str = "pyki-",
                 merge: bool = True,
                 compress: bool = False,
                 activities: str = "cpu,cuda",
                 record_shapes: bool = False,
                 profile_memory: bool = False,
                 with_flops: bool = False,
                 with_modules: bool = False,
                 with_stack: bool = False,
                 with_nvtx: bool = False,
                 timeout: float = 30,
                 start_timeout: float = 30,
                 duration: float = 1,
                 num_steps: int = 0,
                 num_skip_steps: int = 0,
                 module: str = "",
                 function: str = "",
                 python_tracer_max_depth: int = 128,
                 python_tracer_threshold_ns: int = 10000,
                 python_tracer_ignore_c_functions: bool = False,
                 cpu_op_probability: float = 1,
                 with_torch_cuda_memory_trace: bool = False,
                 torch_cuda_memory_trace_max_entry: int = 0,
                 stop_on_cuda_oom: bool = True,
                 export_memory_timeline: bool = False,
                 scenario: str = "auto",
                 on_profile_start: Callable = lambda: None,
                 on_profile_stop: Callable = lambda: None,
                 ):
        self.path = path
        self.prefix = prefix
        self.merge = merge
        self.compress = compress

        self.activities = []
        if activities is None:
            raise_error("activities must not be none")
        activity_list = activities.split(",")
        if "cpu" in activity_list:
            self.activities.append(ProfilerActivity.CPU)

        if "gpu" in activity_list or "cuda" in activity_list:
            if torch.cuda.is_available():
                self.activities.append(ProfilerActivity.CUDA)
            else:
                _logger.warning("CUDA activity is requested but CUDA is not available.")

        self.record_shapes = record_shapes
        self.profile_memory = profile_memory
        self.with_stack = with_stack

        if with_flops:
            _logger.warning("with_flops has been set to False because it is not supported.")
        self.with_flops = False

        self.with_modules = with_modules
        self.with_nvtx = with_nvtx

        self.duration = duration
        self.timeout = timeout
        self.start_timeout = start_timeout

        self.num_steps = num_steps
        self.num_skip_steps = num_skip_steps

        self.module = module if module != "" else None
        self.function = function if function != "" else None

        self.python_tracer_max_depth = python_tracer_max_depth
        self.python_tracer_threshold_ns = python_tracer_threshold_ns
        self.python_tracer_ignore_c_functions = python_tracer_ignore_c_functions
        self.cpu_op_probability = cpu_op_probability

        self.with_torch_cuda_memory_trace = with_torch_cuda_memory_trace
        self.torch_cuda_memory_trace_max_entry = torch_cuda_memory_trace_max_entry
        self.stop_on_cuda_oom = stop_on_cuda_oom
        self.export_memory_timeline = export_memory_timeline

        self.scenario = scenario

        self.on_profile_start = on_profile_start
        self.on_profile_stop = on_profile_stop

        self._validate()

    def _validate(self):
        if "," in self.prefix:
            raise_error("prefix can not contain \",\"")

        if self.duration < 0:
            raise_error("duration must be >= 0")

        if self.num_steps < 0:
            raise_error("num_steps must be >= 0")

        if self.num_skip_steps < 0:
            raise_error("num_skip_steps must be >= 0")

        if self.duration == 0 and self.num_steps == 0:
            raise_error("duration and num_steps can not be both 0")

        if self.start_timeout <= 5:
            raise_error("start_timeout must be > 5")

        if self.timeout <= 0:
            raise_error("timeout must be > 0")

        if (self.module is None and self.function is not None) or (self.module is not None and self.function is None):
            raise_error("module and function must be set together")

        if self.module is not None and self.num_steps == 0:
            raise_error("num_steps must be > 0 when module and function are set")

        if self.python_tracer_max_depth < 0:
            raise_error("python_tracer_max_depth must be >= 0")

        if self.python_tracer_threshold_ns < 0:
            raise_error("python_tracer_threshold_ns must be >= 0")

        if self.cpu_op_probability < 0 or self.cpu_op_probability > 1:
            raise_error("cpu_op_probability must be in range [0, 1]")

        if self.torch_cuda_memory_trace_max_entry < 0 or self.torch_cuda_memory_trace_max_entry > 10 ** 9:
            raise_error("torch_cuda_memory_trace_max_entry must be in range [0, 1,000,000,000]")

        torch_version = get_package_version("torch")
        
        if self.with_torch_cuda_memory_trace:
            min_version = "2.3.0"
            if torch_version is not None and version.parse(torch_version) < version.parse(min_version):
                raise_error(f"with_torch_cuda_memory_trace=true requires torch>={min_version}")
        
        if self.export_memory_timeline:
            min_version = "2.5.0"
            if torch_version is not None and version.parse(torch_version) >= version.parse(min_version):
                if not self.with_stack or not self.profile_memory or not self.record_shapes:
                    raise_error(f"export_memory_timeline requires with_stack, profile_memory and record_shapes to be True")
                if ProfilerActivity.CPU not in self.activities:
                    raise_error("export_memory_timeline requires CPU activity")
            else:
                _logger.warning(f"export_memory_timeline has been set to False because it requires torch>={min_version}")
                self.export_memory_timeline = False

        if self.on_profile_start is None:
            raise_error("on_profile_start must not be none")

        if self.on_profile_stop is None:
            raise_error("on_profile_stop must not be none")

        if self.scenario not in Scenario.VALID_SCENARIOS:
            raise_error(f"scenario must be one of {Scenario.VALID_SCENARIOS}")


class Scenario:
    AUTO = "auto"
    UNKNOWN = "unknown"
    TRAIN = "train"
    VLLM = "vllm"
    SGLANG = "sglang"

    VALID_SCENARIOS = {AUTO, TRAIN, VLLM, SGLANG}

    @staticmethod
    def is_infer(scenario):
        return scenario in (Scenario.VLLM, Scenario.SGLANG)

    @staticmethod
    def is_train(scenario):
        return scenario == Scenario.TRAIN


_PID = os.getpid()

class _Request:

    def __init__(self, id, args: _Arguments):
        self.id = id
        self.args = args
        self.should_run_on_main_thread = False

        self.torch_profiler_start_timeout = args.start_timeout
        if args.num_steps > 0:
            self.torch_profiler_stop_timeout = args.start_timeout * 30
        else:
            self.torch_profiler_stop_timeout = args.duration * 30

        self.torch_profiler_timeout = args.timeout

        self.torch_profiler_start_time = 0
        self.torch_profile_data_path: Optional[str] = None

        self.use_torch_own_with_stack = args.with_stack and args.export_memory_timeline

        self.use_python_function_tracer = args.with_stack and not args.export_memory_timeline
        self.python_function_data_path: Optional[str] = None

        self.use_nvtx_tracer = args.with_nvtx
        self.nvtx_data_path: Optional[str]= None

        self.export_memory_timeline = args.export_memory_timeline
        self.memory_timeline_data_path: Optional[str] = None

        self.use_torch_cuda_memory_tracer = args.with_torch_cuda_memory_trace
        max_entry = args.torch_cuda_memory_trace_max_entry
        if max_entry == 0:
            if args.num_steps > 0:
                # 100w entry per step
                max_entry = max(10 ** 6, 10 ** 6 * args.num_steps)
            else:
                # 20w entry per second
                max_entry = max(10 ** 6, 2 * 10 ** 5 * args.duration)
        max_entry = min(max_entry, 10 ** 9)
        self.torch_cuda_memory_trace_max_entry = int(max_entry)
        self.torch_cuda_memory_data_path: Optional[str] = None
        self.stop_on_cuda_oom = args.stop_on_cuda_oom

        self.use_torch_profile = len(args.activities) > 0
        self.use_auxiliary_tracers = (self.use_python_function_tracer or
                                      self.use_nvtx_tracer or
                                      self.use_torch_cuda_memory_tracer)

        if args.scenario == Scenario.AUTO:
            if is_module_imported("sglang"):
                self.scenario = Scenario.SGLANG
            elif is_module_imported("vllm"):
                self.scenario = Scenario.VLLM
            else:
                self.scenario = Scenario.TRAIN
        else:
            self.scenario = args.scenario

        self.error: Optional[Exception] = None

        _logger.info(
            f"Init PyTorch Profiler start timeout to {self.torch_profiler_start_timeout} seconds.")
        _logger.info(
            f"Init PyTorch Profiler stop timeout to {self.torch_profiler_stop_timeout} seconds.")

    def is_step_mode(self):
        return self.args.num_steps > 0

    def update_torch_profiler_start_time(self, start_time):
        self.torch_profiler_start_time = start_time

    def update_torch_profiler_stop_timeout(self, stop_time):
        duration = math.ceil(time.time() - self.torch_profiler_start_time)
        self.torch_profiler_stop_timeout = min(12000, max(60, duration * 30))
        _logger.info(
            f"Update PyTorch Profiler stop timeout to {self.torch_profiler_stop_timeout} seconds.")

    def set_error(self, error):
        if self.error is None:
            self.error = error

    def set_should_run_on_main_thread(self, should_run_on_main_thread: bool):
        self.should_run_on_main_thread = should_run_on_main_thread

    def gen_result_path(self, suffix):
        prefix = self.args.prefix
        if prefix.endswith("-"):
            prefix = self.args.prefix[:-1]
        return os.path.join(self.args.path, f"{prefix}-{_PID}-{self.id}-{suffix}")

    def gen_torch_profile_data_path(self):
        return self.gen_result_path("torch-profile.json")

    def gen_nvtx_data_path(self):
        return self.gen_result_path("nvtx.json")

    def gen_python_function_data_path(self):
        return self.gen_result_path("python-function.json")

    def gen_torch_cuda_memory_path(self):
        return self.gen_result_path("cuda-memory.pickle")

    def gen_memory_timeline_path(self):
        return self.gen_result_path("memory-timeline.json")

    def need_merge_data(self):
        return self.args.merge

    def gen_merged_data_path(self):
        assert self.need_merge_data()
        return self.gen_result_path("data.json.gz" if self.args.compress else "data.json")


class Result:
    def __init__(self):
        self.scenario = None

        self.python_function_data_path : Optional[str] = None
        self.nvtx_data_path: Optional[str] = None
        self.torch_profile_data_path: Optional[str] = None
        self.torch_cuda_memory_data_path: Optional[str] = None
        self.memory_timeline_data_path: Optional[str] = None

        self.data_path: Optional[str] = None

        self.error: Optional[Exception] = None

    @staticmethod
    def from_error(error: Union[Exception, str]):
        r = Result()
        r.error = error if isinstance(error, Exception) else PyKiError(error)
        return r

    @staticmethod
    def from_request(request: _Request):
        r = Result()
        r.scenario = request.scenario

        if request.need_merge_data():
            p = request.gen_merged_data_path()
            sources = []
            if request.python_function_data_path is not None:
                sources.append(request.python_function_data_path)
            if request.nvtx_data_path is not None:
                sources.append(request.nvtx_data_path)
            if request.torch_profile_data_path is not None:
                path = request.torch_profile_data_path
                try:
                    rewrite = False
                    # TODO: maybe comsume too much memory, need to optimize
                    with open(path, "rt", encoding="utf-8") as f:
                        data = f.read()
                        if ", \"Process Group Description\": ," in data:
                            data = data.replace(", \"Process Group Description\": ,", ",")
                            rewrite = True
                    if rewrite:
                        _logger.info(f"Rewriting {path} to fix illegal json format.")
                        with open(path, "wt", encoding="utf-8") as f:
                            f.write(data)
                    sources.append(path)
                except:
                    _logger.error(f"Failed to rewrite {path}.", exc_info=True)

            if len(sources) > 0:
                merge_files(",".join(sources), p)
                r.data_path = p
                for s in sources:
                    if os.path.exists(s):
                        os.remove(s)
        else:
            r.python_function_data_path = request.python_function_data_path
            r.nvtx_data_path = request.nvtx_data_path
            r.torch_profile_data_path = request.torch_profile_data_path
        # data below should not be merged
        r.torch_cuda_memory_data_path = request.torch_cuda_memory_data_path
        r.memory_timeline_data_path = request.memory_timeline_data_path

        r.error = request.error
        return r

    def has_data(self):
        return (self.data_path is not None or
                self.python_function_data_path is not None or
                self.nvtx_data_path is not None or
                self.torch_profile_data_path is not None or
                self.torch_cuda_memory_data_path is not None or
                self.memory_timeline_data_path is not None)

    def get_filenames(self):
        filenames = []
        if not self.has_data():
            return filenames
        if self.data_path is not None:
            filenames.append(os.path.basename(self.data_path))
        else:
            if self.python_function_data_path is not None:
                filenames.append(os.path.basename(self.python_function_data_path))
            if self.nvtx_data_path is not None:
                filenames.append(os.path.basename(self.nvtx_data_path))
            if self.torch_profile_data_path is not None:
                filenames.append(os.path.basename(self.torch_profile_data_path))
        if self.torch_cuda_memory_data_path is not None:
            filenames.append(os.path.basename(self.torch_cuda_memory_data_path))
        if self.memory_timeline_data_path is not None:
            filenames.append(os.path.basename(self.memory_timeline_data_path))
        return filenames

    def __repr__(self) -> str:
        result = f"Identified Scenario: {self.scenario}"

        if self.has_data():
            if self.data_path is not None:
                result += f'\nData written to {self.data_path}'
            else:
                if self.python_function_data_path is not None:
                    result += f'\nPython Function Trace written to {self.python_function_data_path}'
                if self.nvtx_data_path is not None:
                    result += f'\nNVTX Data written to {self.nvtx_data_path}'
                if self.torch_profile_data_path is not None:
                    result += f'\nPyTorch Profile written to {self.torch_profile_data_path}'
            if self.torch_cuda_memory_data_path is not None:
                result += f'\nTorch CUDA Memory Profile written to {self.torch_cuda_memory_data_path}'
            if self.memory_timeline_data_path is not None:
                result += f'\nMemory Timeline written to {self.memory_timeline_data_path}'
        else:
            result += '\nNo Data'

        if self.error is not None:
            result += f'\nError: {self.error}'
        return result


def unwrap_no_exception(module_str, function_str):
    try:
        unwrap(module_str, function_str)
    except:
        _logger.error(f"Failed to unwrap {module_str}.{function_str}.", exc_info=True)


class _Hook(ABC):
    def __init__(self):
        self.lock = Lock()
        self.active = False

    def activate(self):
        assert not self.active
        with self.lock:
            self.do_activate()
            self.active = True

    @abstractmethod
    def do_activate(self):
        pass

    def deactivate(self):
        if not self.active:
            return
        with self.lock:
            if not self.active:
                return
            self.do_deactivate()
            self.active = False

    @abstractmethod
    def do_deactivate(self):
        pass


class _CheckFunctionRunHook(_Hook):
    def __init__(self, module, function):
        super().__init__()
        self.module = module
        self.function = function
        self.function_run = False

    def do_activate(self):
        def _wrapper(call_wrapped, instance, args, kwargs):
            self.function_run = True
            call_wrapped(*args, **kwargs)

        try:
            wrap_function_wrapper(self.module, self.function, _wrapper)
        except:
            _logger.error(f"Failed to wrap {self.module}.{self.function}.", exc_info=True)

    def do_deactivate(self):
        unwrap_no_exception(self.module, self.function)

    def check_function_run(self):
        return self.function_run


class _IterationHook(_Hook):

    def __init__(self, request: _Request, step_mode):
        super().__init__()

        self.request = request
        self.args = request.args
        self.step_mode = step_mode
        self.start_func:Optional[Callable] = None
        self.stop_func:Optional[Callable] = None
        self.stopped = False
        self.add_metadata_func:Optional[Callable] = None

        self.current = 0
        self.skipped = 0
        self.num_steps = self.args.num_steps
        self.num_skip_steps = self.args.num_skip_steps

        self.record_function: Optional[record_function] = None
        self.execution_lock = threading.RLock()

    def set_start_stop_func(self, start_func: Callable, stop_func: Callable):
        self.start_func = start_func
        self.stop_func = stop_func

    def set_add_metadata_func(self, add_metadata_fuc):
        self.add_metadata_func = add_metadata_fuc

    def execute_in_duration_mode(self):
        if not self.active:
            return

        with self.execution_lock:
            self.exit_record_function()
            self.current += 1
            self.enter_record_function()

    def execute_in_step_mode(self):
        if not self.active:
            return

        with self.execution_lock:
            if self.stopped:
                return

            if self.skipped < self.num_skip_steps:
                self.skipped += 1
                return

            self.exit_record_function()

            if self.current == self.num_steps:
                assert self.stop_func is not None
                self.stop_func()
                self.stopped = True
                return

            self.current += 1
            if self.current == 1:
                assert self.start_func is not None
                self.start_func()

            self.enter_record_function()

    def before_stopping_torch_profiler(self):
        pass

    def enter_record_function(self):
        rf = record_function(f"IterationStep - {self.current}")
        self.record_function = rf
        rf.__enter__()

    def exit_record_function(self):
        rf = self.record_function
        if rf is not None:
            self.record_function = None
            if rf.record is not None:
                rf.__exit__(None, None, None)

    def add_metadata_json(self, key, value):
        assert self.add_metadata_func is not None
        self.add_metadata_func(key, value)


class _OptimizerStepInfo:
    def __init__(self, step):
        self.step = step
        self.loss: Optional[Union[int, float]] = None

    def __repr__(self) -> str:
        result = "{"
        result += f'"step": {self.step}'
        if self.loss is not None:
            result += f', "loss": {self.loss}'
        result += "}"
        return result


class _OptimizerStepPostHook(_IterationHook):

    def __init__(self, request: _Request, step_mode: bool):
        super().__init__(request, step_mode)

        self.num_steps = request.args.num_steps
        self.num_skip_steps = request.args.num_skip_steps

        self.skipped = 0
        self.current_step_info: Optional[_OptimizerStepInfo] = None
        self.step_infos = []

        self.record_function = None
        self.hook_handle: Optional[RemovableHandle] = None

    def do_activate(self):
        def _wrapper(call_wrapped, instance, args, kwargs):
            try:
                self.fire_loss(args[0].item())
            except Exception:
                pass
            return call_wrapped(*args, **kwargs)

        wrap_function_wrapper("torch.autograd", "backward", _wrapper)

        execute = self.execute_in_step_mode if self.step_mode else self.execute_in_duration_mode

        self.hook_handle = register_optimizer_step_post_hook(lambda optimizer, *args, **kwargs: execute())

    def do_deactivate(self):
        unwrap_no_exception("torch.autograd", "backward")
        if self.hook_handle is not None:
            self.hook_handle.remove()

    def before_stopping_torch_profiler(self):
        self.add_metadata_json("pyki_steps", str(self.step_infos))

    def fire_loss(self, loss):
        info = self.current_step_info
        if info is not None and (isinstance(loss, float) or isinstance(loss, int)) and (
                not math.isnan(loss) and not math.isinf(loss)):
            info.loss = loss

    def enter_record_function(self):
        super().enter_record_function()
        self.current_step_info = _OptimizerStepInfo(self.current)

    def exit_record_function(self):
        super().exit_record_function()
        if self.current_step_info is not None:
            self.step_infos.append(self.current_step_info)
            self.current_step_info = None


def _queue_get_hook(self, block=True, timeout=None):
    if self._closed:
        raise ValueError(f"Queue {self!r} is closed")

    if block and timeout is None:
        with self._rlock:
            while not self._poll(0.05):
                pass

    from multiprocessing.queues import Queue
    return Queue.__pyki_get_backup(self, block, timeout) # type: ignore


def _socket_poll_hook(self, timeout=None, flags=1):
    from zmq.sugar.socket import Socket
    if timeout is None:
        return Socket.__pyki_poll_backup(self, timeout, flags) # type: ignore
    while True:
        if timeout < 50:
            return Socket.__pyki_poll_backup(self, timeout, flags) # type: ignore
        v = Socket.__pyki_poll_backup(self, 50, flags) # type: ignore
        if v != 0:
            return v
        timeout -= 50


class _InferenceHook(_IterationHook):
    """Base class for inference framework hooks (vLLM, SGLang, etc.)."""

    def __init__(self, request: _Request, step_mode: bool):
        super().__init__(request, step_mode)
        self.wrapped_functions = []
        self.record_function = None

    def _get_wrap_targets(self):
        """Return list of (module, function, ignore_not_exist) tuples to hook."""
        raise NotImplementedError

    def do_activate(self):
        import types
        if not self.step_mode and self.request.should_run_on_main_thread:
            from multiprocessing.queues import Queue
            if not hasattr(Queue, "__pyki_get_backup"):
                __pyki_get_backup = types.FunctionType(
                    Queue.get.__code__,
                    Queue.get.__globals__,
                    name="__pyki_get_backup",
                    argdefs=Queue.get.__defaults__,
                )
                setattr(Queue, "__pyki_get_backup", __pyki_get_backup)
            Queue.get.__code__ = _queue_get_hook.__code__

            if is_module_imported("zmq.sugar.socket"):
                from zmq.sugar.socket import Socket
                if not hasattr(Socket, "__pyki_poll_backup"):
                    __pyki_poll_backup = types.FunctionType(
                        Socket.poll.__code__,
                        Socket.poll.__globals__,
                        name="__pyki_poll_backup",
                        argdefs=Socket.poll.__defaults__,
                    )
                    setattr(Socket, "__pyki_poll_backup", __pyki_poll_backup)
                Socket.poll.__code__ = _socket_poll_hook.__code__

        execute = self.execute_in_step_mode if self.step_mode else self.execute_in_duration_mode

        def _wrapper(call_wrapped, instance, args, kwargs):
            try:
                execute()
            except:
                pass
            return call_wrapped(*args, **kwargs)

        try:
            for module, function in self._get_wrap_targets():
                self._wrap_if_imported(module, function, _wrapper)
        except Exception as e:
            self.restore()
            raise e

    def _wrap_if_imported(self, module, function, wrapper):
        if not is_module_imported(module):
            return
        try:
            wrap_function_wrapper(module, function, wrapper)
            self.wrapped_functions.append((module, function))
        except Exception as e:
            if isinstance(e, ModuleNotFoundError) or isinstance(e, AttributeError):
                return
            raise e

    def do_deactivate(self):
        self.restore()

    def restore(self):
        if not self.step_mode and self.request.should_run_on_main_thread:
            try:
                from multiprocessing.queues import Queue
                if hasattr(Queue, "__pyki_get_backup"):
                    Queue.get.__code__ = getattr(Queue, "__pyki_get_backup").__code__

                if is_module_imported("zmq.sugar.socket"):
                    from zmq.sugar.socket import Socket
                    if hasattr(Socket, "__pyki_poll_backup"):
                        Socket.poll.__code__ = getattr(Socket, "__pyki_poll_backup").__code__
            except:
                pass
        for module, function in self.wrapped_functions:
            unwrap_no_exception(module, function)
        self.wrapped_functions = []


class _VLLMHook(_InferenceHook):

    def _get_wrap_targets(self):
        return [
            ("vllm.worker.worker_base", "WorkerBase.execute_model"),
            ("vllm.worker.worker_base", "DelegateWorkerBase.execute_model"),
            ("vllm.worker.neuron_worker", "NeuronWorker.execute_model"),
            ("vllm.worker.spec_decode_worker", "SpecDecodeWorker.execute_model"),
            ("vllm.worker.smaller_tp_proposer_worker", "SmallerTpProposerWorker.execute_model"),
            ("vllm.worker.proposer_worker_base", "NonLLMProposerWorkerBase.execute_model"),
            ("vllm.worker.worker_base", "LocalOrDistributedWorkerBase.execute_model"),
            ("vllm.worker.hpu_worker", "HPUWorker.execute_model"),
            ("vllm.v1.worker.gpu_worker", "Worker.execute_model"),
        ]


class _SGLangHook(_InferenceHook):

    def _get_wrap_targets(self):
        return [
            ("sglang.srt.model_executor.model_runner", "ModelRunner.forward"),
        ]


class _FunctionHookError(PyKiError):
    def __init__(self, message: str):
        super().__init__(message)


class _FunctionHook(_IterationHook):

    def __init__(self, request: _Request):
        super().__init__(request, True)
        assert request.args.module is not None and request.args.function is not None
        self.module = request.args.module
        self.function = request.args.function

    def do_activate(self):
        def _wrapper(call_wrapped, instance, args, kwargs):
            try:
                self.execute_in_step_mode()
            except:
                pass
            return call_wrapped(*args, **kwargs)

        try:
            if not is_module_imported(self.module):
                raise _FunctionHookError(f"Module {self.module} is not imported")

            wrap_function_wrapper(self.module, self.function, _wrapper)
        except Exception as e:
            if isinstance(e, ModuleNotFoundError) or isinstance(e, AttributeError):
                raise _FunctionHookError(f"Failed to hook {self.module}.{self.function}")
            raise e

    def do_deactivate(self):
        unwrap_no_exception(self.module, self.function)

TRACER_ORIGIN = "ORIGIN"
TRACER_STARTED = "STARTED"
TRACER_STOPPED = "STOPPED"
TRACER_ERROR = "ERROR"


class _TracerBase(ABC):

    def __init__(self, name):
        self.name = name
        self.state = TRACER_ORIGIN
        self.lock = threading.Lock()

        self.request: Optional[_Request] = None

    def start_if_enabled(self, request: _Request):
        if not self.is_enabled(request):
            return

        with self.lock:
            if self.state is not TRACER_ORIGIN:
                _logger.warning(
                    f"Cannot start {self.name} since it's state is {self.state}.")
                return
            try:
                _logger.info(f"Begin to start {self.name}.")
                start_time = time.time()
                self.request = request
                self.do_start()
                self.state = TRACER_STARTED
                _logger.info(f"Success to start {self.name}. Duration: {time.time() - start_time:.2f} seconds.")
            except:
                self.state = TRACER_ERROR
                _logger.error(f"Failed to start {self.name}.", exc_info=True)

    @abstractmethod
    def is_enabled(self, request: _Request) -> bool:
        ...

    @abstractmethod
    def do_start(self):
        ...

    def stop_if_started(self):
        with self.lock:
            if self.state is not TRACER_STARTED:
                return
            try:
                _logger.info(f"Begin to stop {self.name}.")
                start_time = time.time()
                self.do_stop()
                self.state = TRACER_STOPPED
                _logger.info(f"Success to stop {self.name}. Duration: {time.time() - start_time:.2f} seconds.")
            except:
                self.state = TRACER_ERROR
                _logger.info(f"Failed to stop {self.name}.", exc_info=True)

    @abstractmethod
    def do_stop(self):
        ...

    def export_data_if_stopped(self):
        with self.lock:
            if self.state is not TRACER_STOPPED:
                return
            try:
                _logger.info(f"Begin to export {self.name} data.")
                start_time = time.time()
                self.do_export_data()
                _logger.info(f"Success to export {self.name} data. Duration: {time.time() - start_time:.2f} seconds.")
            except:
                self.state = TRACER_ERROR
                _logger.info(
                    f"Failed to export {self.name} data.", exc_info=True)

    @abstractmethod
    def do_export_data(self):
        ...

    def reset(self):
        with self.lock:
            if self.state is TRACER_ORIGIN:
                self.request = None
                return

            _logger.info(f"Begin to reset {self.name}.")
            try:
                self.do_reset()
                self.request = None
                self.state = TRACER_ORIGIN
                _logger.info(f"Success to reset {self.name}.")
            except:
                _logger.info(f"Failed to reset {self.name}.", exc_info=True)
                self.state = TRACER_ERROR

    @abstractmethod
    def do_reset(self):
        ...


class _PythonFunctionTracer(_TracerBase):
    def __init__(self):
        super().__init__("Python Function Tracer")

    def is_enabled(self, request: _Request):
        return request.use_python_function_tracer

    def do_start(self):
        assert self.request is not None
        args = self.request.args
        pe.start_python_tracer(args.python_tracer_max_depth,
                               args.python_tracer_threshold_ns,
                               args.python_tracer_ignore_c_functions)

    def do_stop(self):
        pe.stop_python_tracer()

    def do_export_data(self):
        assert self.request is not None
        data_path = self.request.gen_python_function_data_path()
        pe.export_python_tracer_data(data_path)
        self.request.python_function_data_path = data_path

    def do_reset(self):
        pe.reset_python_tracer()


class _NVTXTracer(_TracerBase):
    def __init__(self):
        super().__init__("NVTX Tracer")
        self.nvtx_tracer = nvtx_tracer_instance

    def is_enabled(self, request: _Request):
        return request.use_nvtx_tracer

    def do_start(self):
        self.nvtx_tracer.start_tracing()

    def do_stop(self):
        self.nvtx_tracer.stop_tracing()

    def do_export_data(self):
        assert self.request is not None
        data_path = self.request.gen_nvtx_data_path()
        self.nvtx_tracer.export_data(data_path)
        self.request.nvtx_data_path = data_path

    def do_reset(self):
        self.nvtx_tracer.reset()

class _TorchCudaMemoryTracer(_TracerBase):
    def __init__(self):
        super().__init__("Torch Cuda Memory Tracer")
        self.snapshot = None

    def is_enabled(self, request: _Request):
        return request.use_torch_cuda_memory_tracer

    def do_start(self):
        assert self.request is not None
        if not pe.enable_gil_check_for_PythonTraceback_gather():
            _logger.warning("fail to enable_gil_check_for_PythonTraceback_gather")
        torch.cuda.memory._record_memory_history(enabled="all",
                                                 max_entries=self.request.torch_cuda_memory_trace_max_entry,
                                                 context="all",
                                                 stacks="all")

    def do_stop(self):
        assert self.request is not None
        # torch 默认会使用addr2line来解析符号，但是在跑到某些比较大的so时addr2line可能会卡住，我们让torch自己来解析符号
        with EnvironmentSetter("TORCH_SYMBOLIZE_MODE", "fast", force_override=True):
            # 这一步实际上会处理数据导致停顿较长时间。目前torch没有提供单独暂停采集而不清理数据的api
            self.snapshot = torch.cuda.memory._snapshot()
        buffer_used = sum(len(trace_list) for trace_list in self.snapshot['device_traces'])
        buffer_request = self.request.torch_cuda_memory_trace_max_entry
        if buffer_used >= buffer_request:
            _logger.warning(f"Torch cuda memory buffer was full during tracing. Current buffer size: {buffer_request}")
        torch.cuda.memory._record_memory_history(enabled=None)

    def do_export_data(self):
        assert self.request is not None
        assert self.snapshot is not None
        data_path = self.request.gen_torch_cuda_memory_path()
        pickle.dump(self.snapshot, open(data_path, 'wb'))
        self.request.torch_cuda_memory_data_path = data_path

    def do_reset(self):
        pe.disable_gil_check_for_PythonTraceback_gather()
        torch.cuda.memory._record_memory_history(enabled=None)
        self.snapshot = None


TP_ORIGIN = "ORIGN"

TP_STARTING = "STARTING"
TP_START_SUCCESS = "START_SUCCESS"
TP_START_PRE_TIMEOUT = "START_PRE_TIMEOUT"
TP_START_TIMEOUT = "START_TIMEOUT"

TP_STOPPING = "STOPPING"
TP_STOP_SUCCESS = "STOP_SUCCESS"
TP_STOP_TIMEOUT = "STOP_TIMEOUT"

TP_ERROR = "ERROR"

TP_STATE_IS_NOT_ORIGIN = "State is not ORIGIN"
TP_CONTROL_THREAD_IS_RUNNING = "Control thread is running"
TP_CUDA_IS_IN_BAD_FORK = "CUDA is in bad fork"
TP_USED_BY_MEGATRON = "PyTorch Profiler is not used by Megatron"
TP_ENABLED_BY_USER = "PyTorch Profiler is already enabled by user"

TP_START_TIMEOUT_ERROR = PyKiError("Start PyTorch Profiler timeout.")
TP_START_TIMEOUT_CANCELLED_ERROR = PyKiError("Start PyTorch Profiler timeout (cancelled).")
TP_STOP_TIMEOUT_ERROR = PyKiError("Stop PyTorch Profiler timeout.")


class _AddPendingCallError(PyKiError):
    def __init__(self, is_start):
        super().__init__(f"Failed to add pending call for {'starting' if is_start else 'stopping'} PyTorch Profiler.")
        self.is_start = is_start


class _Service:
    def __init__(self):
        self.lock = threading.RLock()

        self.active = False
        self.next_request_id = 1

        self.tp_state = TP_ORIGIN
        self.tp_state_condition = threading.Condition(self.lock)

        self.torch_feature_support = _TorchFeatureSupport()

        self.check_cuda_is_in_bad_fork_result = None
        self.check_megatron_profiler_result = None

        self.auxiliary_tracers: List[_TracerBase] = [_PythonFunctionTracer(), _NVTXTracer(), _TorchCudaMemoryTracer()]
        self.cuda_oom_callback = None

        vi = sys.version_info
        self.is_python_version_312_0_4 = vi.major == 3 and vi.minor == 12 and 0 <= vi.micro <= 4

    @run_once
    def register_cuda_oom_observer(self):
        def call_oom_callback(device, alloc, device_alloc, device_free):
            if self.cuda_oom_callback is not None:
                _logger.info("call cuda oom callback")
                self.cuda_oom_callback()
        _logger.info("register cuda oom observer")
        torch._C._cuda_attach_out_of_memory_observer(call_oom_callback)


    def set_error(self, request: _Request, reason: str, disable_auxiliary_tracers: bool):
        error = PyKiError(f"PyTorch Profiler is not available: {reason}.")
        _logger.error(f"{error}")
        request.set_error(error)
        if disable_auxiliary_tracers:
            if request.use_nvtx_tracer:
                request.use_nvtx_tracer = False
                _logger.warning("Disable NVTX Tracer.")
            if request.use_python_function_tracer:
                request.use_python_function_tracer = False
                _logger.warning("Disable Python Function Tracer.")
            request.use_auxiliary_tracers = False

    def check_cuda_is_in_bad_fork(self, request: _Request):
        try:
            if self.check_cuda_is_in_bad_fork_result is None:
                self.check_cuda_is_in_bad_fork_result = not getattr(torch._C, "_cuda_isInBadFork", lambda: False)()
            if self.check_cuda_is_in_bad_fork_result:
                return True
            self.set_error(request, TP_CUDA_IS_IN_BAD_FORK, True)
            return False
        except:
            _logger.error(
                "Failed to check if CUDA is in bad fork.", exc_info=True)
            self.check_cuda_is_in_bad_fork_result = True
            return True

    def check_megatron_profiler(self, request: _Request):
        if self.check_megatron_profiler_result is not None:
            if self.check_megatron_profiler_result:
                return True
            self.set_error(request, TP_USED_BY_MEGATRON, True)
            return False

        try:
            if not is_module_imported("megatron.training"):
                return True
            mt = threading.main_thread()
            id = mt.ident
            assert id is not None
            frame: Optional[Any] = sys._current_frames()[id]
            while frame is not None:
                code = frame.f_code
                if code is not None and code.co_name == "train" and code.co_filename.endswith("megatron/training/training.py"):
                    break
                frame = frame.f_back
            if frame is None:
                return True
            locals = frame.f_locals
            if locals is None:
                return True
            prof = locals.get("prof", None)
            if prof is None:
                return True
            _logger.info("Checking if PyTorch Profiler is used by Megatron.")
            schedule = prof.schedule
            freevars = schedule.__code__.co_freevars
            closure = schedule.__closure__
            wait = closure[freevars.index("wait")].cell_contents
            warmup = closure[freevars.index("warmup")].cell_contents
            active = closure[freevars.index("active")].cell_contents
            repeat = closure[freevars.index("repeat")].cell_contents
            skip_first = closure[freevars.index("skip_first")].cell_contents
            skip_first_wait = 0
            if "skip_first_wait" in freevars:
                skip_first_wait = closure[freevars.index(
                    "skip_first_wait")].cell_contents
            _logger.info(
                f"wait={wait}, warmup={warmup}, active={active}, repeat={repeat}, skip_first={skip_first}, skip_first_wait={skip_first_wait}.")
            if repeat == 0:
                self.check_megatron_profiler_result = False
            else:
                current_step = prof.step_num
                _logger.info(f"step_num={current_step}.")
                min_step_num = (wait + warmup + active) * repeat - (wait if skip_first_wait != 0 else 0) + skip_first
                _logger.info(f"min_step_num={min_step_num}.")
                if current_step >= min_step_num:
                    self.check_megatron_profiler_result = True
                    return True
            self.set_error(request, TP_USED_BY_MEGATRON, True)
            return False
        except:
            _logger.error(
                "Failed to check if PyTorch Profiler is used by Megatron.", exc_info=True)
            self.check_megatron_profiler_result = True
            return True

    def check_if_enabled_by_user(self, request: _Request):
        try:
            import torch.autograd.profiler as p
            if p._is_profiler_enabled:
                self.set_error(request, TP_ENABLED_BY_USER, True)
                return False
        except:
            _logger.error(
                "Failed to check if PyTorch Profiler is already enabled by user.", exc_info=True)
        return True

    def check_available(self, request: _Request):
        with self.lock:
            if self.tp_state is not TP_ORIGIN:
                self.set_error(request, TP_STATE_IS_NOT_ORIGIN, False)
                return False
        if not self.check_cuda_is_in_bad_fork(request):
            return False
        if not self.check_megatron_profiler(request):
            return False
        if not self.check_if_enabled_by_user(request):
            return False
        return True

    def should_teardown_cupti(self, request, cuda_graph_check_hook: _CheckFunctionRunHook):
        cuda_version = get_cuda_version()
        if cuda_version is not None and version.parse(cuda_version) >= version.parse("12.6"):
            return True

        if Scenario.is_infer(request.scenario):
            return False

        if hasattr(torch, "_inductor"):
            import torch._inductor.config as inductor_config
            if inductor_config.triton.cudagraphs:
                return False

        return not cuda_graph_check_hook.check_function_run()

    def transit_tp_state(self, from_state, to_state):
        # Note: this method should be called with self.lock held
        assert self.tp_state is from_state
        assert from_state is not to_state
        self.tp_state = to_state
        self.tp_state_condition.notify_all()
        _logger.info(f"State transited from {from_state} to {to_state}.")

    def wait_until_tp_state_not(self, state, timeout=None):
        # Note: this method should be called with self.lock held
        if timeout is None:
            while self.tp_state is state:
                self.tp_state_condition.wait()
        else:
            if self.tp_state is state:
                end = time.time() + timeout
                while self.tp_state is state:
                    left = end - time.time()
                    if left <= 0:
                        return False
                    self.tp_state_condition.wait(left)
        return True

    def is_already_enabled_error(self, error):
        while error.__cause__:
            error = error.__cause__
        return (
            isinstance(error, RuntimeError) and
            "Profiler is already enabled on this thread" in str(error)
        )

    def is_recoverable_error(self, error):
        if isinstance(error, _AddPendingCallError):
            return error.is_start
        return True

    def do_execute(self, request: _Request):
        is_step_mode = request.is_step_mode()
        is_duration_mode = not is_step_mode

        profile: Optional[torch.profiler.profile] = None

        if is_step_mode and request.args.module is not None:
            iteration_hook: _IterationHook = _FunctionHook(request)
        elif request.scenario == Scenario.SGLANG:
            iteration_hook = _SGLangHook(request, is_step_mode)
        elif request.scenario == Scenario.VLLM:
            iteration_hook = _VLLMHook(request, is_step_mode)
        else:
            iteration_hook = _OptimizerStepPostHook(request, is_step_mode)

        cuda_graph_check_hook = _CheckFunctionRunHook(
            "torch.cuda", "CUDAGraph.replay")

        start_auxiliary_tracers_event = threading.Event()
        stop_auxiliary_tracers_event = threading.Event()
        on_profile_start_called = False
        duration_interrupt_event = threading.Event()

        @run_once
        def on_cuda_oom():
            if request.is_step_mode():
                stop()
            else:
                duration_interrupt_event.set()

        @run_once
        def call_on_profile_start():
            try:
                nonlocal on_profile_start_called
                on_profile_start_called = True
                _logger.info("Call on_profile_start.")
                request.args.on_profile_start()
            except Exception as e:
                _logger.error("Failed to call on_profile_start.", exc_info=True)

        @run_once
        def call_on_profile_stop():
            try:
                if not on_profile_start_called:
                    return
                _logger.info("Call on_profile_stop.")
                request.args.on_profile_stop()
            except Exception as e:
                _logger.error("Failed to call on_profile_stop.", exc_info=True)

        @run_once
        def start_auxiliary_tracers():
            try:
                if not request.use_auxiliary_tracers:
                    return
                _logger.info("Begin to start auxiliary tracers.")
                for tracer in self.auxiliary_tracers:
                    tracer.start_if_enabled(request)
            finally:
                start_auxiliary_tracers_event.set()

        @run_once
        def stop_auxiliary_tracers():
            try:
                if not request.use_auxiliary_tracers:
                    return
                _logger.info("Begin to stop auxiliary tracers.")
                for tracer in reversed(self.auxiliary_tracers):
                    tracer.stop_if_started()
            finally:
                stop_auxiliary_tracers_event.set()

        @run_once
        def export_data_of_auxiliary_tracers():
            if not request.use_auxiliary_tracers:
                return
            _logger.info("Begin to export data of auxiliary tracers.")
            for tracer in self.auxiliary_tracers:
                tracer.export_data_if_stopped()

        @run_once
        def reset_auxiliary_tracers():
            if not request.use_auxiliary_tracers:
                return
            _logger.info("Begin to reset auxiliary tracers.")
            for tracer in self.auxiliary_tracers:
                tracer.reset()

        @run_once
        def run_auxiliary_tracers_directly():
            if is_duration_mode and request.use_auxiliary_tracers:
                start_auxiliary_tracers()
                call_on_profile_start()
                duration_interrupt_event.wait(request.args.duration)
                stop_auxiliary_tracers()
                call_on_profile_stop()
                export_data_of_auxiliary_tracers()
            else:
                start_auxiliary_tracers_event.set()
                stop_auxiliary_tracers_event.set()

        def activate_hooks():
            iteration_hook.activate()
            cuda_graph_check_hook.activate()

        @run_once
        def deactivate_hooks():
            nonlocal iteration_hook, cuda_graph_check_hook
            try:
                iteration_hook.deactivate()
            except:
                _logger.error(
                    f"Failed to deactivate iteration hook.", exc_info=True)
            try:
                cuda_graph_check_hook.deactivate()
            except:
                _logger.error(
                    f"Failed to deactivate cuda graph check hook.")

        def reset():
            nonlocal profile
            deactivate_hooks()
            if profile is not None:
                if hasattr(profile, "action_map"):
                    setattr(profile, "action_map", None)
                profile = None
            with self.lock:
                self.transit_tp_state(self.tp_state, TP_ORIGIN)

        def handle_error(e, message="Exception occurred."):
            _logger.error(message, exc_info=True)
            request.set_error(e)
            if self.is_recoverable_error(e):
                reset()
            else:
                deactivate_hooks()
                with self.lock:
                    self.transit_tp_state(self.tp_state, TP_ERROR)

        @_pending_call
        def do_start(called_by_control_thread=False):
            nonlocal profile
            stop_now = False
            try:
                with self.lock:
                    if self.tp_state is TP_START_PRE_TIMEOUT:
                        reset()
                        return
                if not request.use_torch_profile:
                    _logger.info("don't start torch profiler because activities is empty")
                else:
                    ct = current_thread()
                    is_main_thread = ct == main_thread()

                    args = request.args
                    config_kwargs = {}
                    if not should_start_stop_tp_on_main_thread and self.torch_feature_support.is_feature_supported("experiment_config.profile_all_threads"):
                        config_kwargs["profile_all_threads"] = True
                    args_need_support_check = ["cpu_op_probability"]
                    for arg in args_need_support_check:
                        if self.torch_feature_support.is_feature_supported(f"experiment_config.{arg}"):
                            config_kwargs[arg] = getattr(args, arg)
                    _logger.debug(f"torch._C._profiler._ExperimentalConfig: {config_kwargs}.")
                    experimental_config = torch._C._profiler._ExperimentalConfig(**config_kwargs)  # type: ignore
                    profile = torch.profiler.profile(activities=args.activities,
                                                        record_shapes=args.record_shapes,
                                                        profile_memory=args.profile_memory,
                                                        with_stack=request.use_torch_own_with_stack,
                                                        with_flops=args.with_flops,
                                                        with_modules=args.with_modules,
                                                        experimental_config=experimental_config)

                    _logger.info(
                        f"Begin to start PyTorch Profiler by thread [{ct.name}, {is_main_thread}].")
                    begin_time = time.time()
                    # cupti 默认使用 pthread_self()返回的tid，在有些情况下会有些问题，我们强制它使用系统tid
                    _cupti_use_system_tid()
                    # torch 2.8之后，torch inductor 会 import 更多东西导致时间变长，而万一主线程cpu密集，会导致import时间超长
                    # 所以我们降低 switch_interval 来争取更多GIL时间
                    torch_version = get_package_version("torch")
                    if (not is_main_thread and torch_version is not None
                            and version.parse(torch_version) >= version.parse("2.8.0")):
                        switch_interval = 0.0005
                    else:
                        switch_interval = 0
                    with AdjustSwitchInterval(switch_interval):
                        profile.__enter__()
                    end_time = time.time()
                    duration = end_time - begin_time
                    _logger.info(
                        f"Success to start PyTorch Profiler in {duration:.2f} seconds.")
                    if request.use_torch_own_with_stack and self.is_python_version_312_0_4:
                        try:
                            pe.enable_workaround_for_3_12_0_4()
                        except Exception as e:
                            _logger.warning(
                                f"Failed to enable workaround for CPython 3.12.0-4: {e}")

                    request.update_torch_profiler_start_time(end_time)
                with self.lock:
                    state = self.tp_state
                    if state is TP_STARTING or state is TP_START_PRE_TIMEOUT:
                        self.transit_tp_state(state, TP_START_SUCCESS)
                    else:
                        stop_now = True
            except Exception as e:
                handle_error(e, "Failed to start PyTorch Profiler.")
                return

            if stop_now:
                do_stop()
                return

            if called_by_control_thread:
                with self.lock:
                    while self.tp_state is TP_START_SUCCESS:
                        self.tp_state_condition.wait()
                    stop_now = self.tp_state is TP_STOPPING or self.tp_state is TP_STOP_TIMEOUT
                if stop_now:
                    do_stop()

        @_pending_call
        def do_stop():
            nonlocal profile
            try:
                hook = iteration_hook
                if hook is not None:
                    hook.exit_record_function()
                    hook.before_stopping_torch_profiler()
            except:
                _logger.error(
                    "Failed to call exit_record_function or before_stopping_torch_profiler.", exc_info=True)

            try:
                if not request.use_torch_profile:
                    _logger.info("don't start torch profiler because activities is empty")
                else:
                    assert profile is not None

                    ct = current_thread()
                    is_main_thread = ct == main_thread()
                    profile.add_metadata_json(
                        "pyki_trace_type", '\"torch_profile\"')

                    # avoid high overhead and potential memory leak in old torch version, see https://github.com/pytorch/pytorch/pull/132713
                    if hasattr(profile, "profiler"):
                        if profile.profiler is not None and hasattr(profile.profiler, "_parse_kineto_results"):
                            _logger.debug(
                                "set self.profile.profiler._parse_kineto_results to return empty list.")
                            profile.profiler._parse_kineto_results = lambda *args, **kwargs: []

                    assert cuda_graph_check_hook is not None
                    should_teardown_cupti = self.should_teardown_cupti(request, cuda_graph_check_hook)
                    _logger.debug(
                        f"should_teardown_cupti: {should_teardown_cupti}.")

                    _logger.info(
                        f"Begin to stop PyTorch Profiler by thread [{ct.name}, {is_main_thread}].")
                    begin_time = time.time()
                    with EnvironmentSetter("TEARDOWN_CUPTI", "1", enable=should_teardown_cupti):
                        profile.__exit__(None, None, None)
                    duration = time.time() - begin_time
                    _logger.info(
                        f"Success to stop PyTorch Profiler in {duration:.2f} seconds.")

                    if request.use_torch_own_with_stack and self.is_python_version_312_0_4:
                        try:
                            pe.disable_workaround_for_3_12_0_4()
                        except Exception as e:
                            _logger.warning(
                                f"Failed to disable workaround for CPython 3.12.0-4: {e}")

                with self.lock:
                    if self.tp_state is TP_STOPPING:
                        self.transit_tp_state(self.tp_state, TP_STOP_SUCCESS)
                    else:
                        reset()
            except Exception as e:
                handle_error(e, "Failed to stop PyTorch Profiler.")
            finally:
                deactivate_hooks()

        def add_metadata(key, value):
            if profile is not None:
                profile.add_metadata_json(key, value)

        def export_data():
            nonlocal profile
            if not request.use_torch_profile:
                _logger.info("don't export torch profiler because activities is empty")
            else:
                try:
                    assert profile is not None
                    _logger.info(f"Begin to export PyTorch Profiler data.")

                    if request.export_memory_timeline:
                        data_path = request.gen_memory_timeline_path()
                        start_time = time.time()
                        profile.export_memory_timeline(data_path)
                        duration = time.time() - start_time
                        request.memory_timeline_data_path = data_path
                        _logger.info(
                            f"Success to export memory timeline data in {duration:.2f} seconds.")

                    data_path = request.gen_torch_profile_data_path()
                    if not pe.enable_no_gil_for_torch_profile_export():
                        _logger.warning("failed to enable no_gil for torch.profiler.export_chrome_trace")
                    start_time = time.time()
                    profile.export_chrome_trace(data_path)
                    duration = time.time() - start_time
                    request.torch_profile_data_path = data_path
                    _logger.info(
                        f"Success to export PyTorch Profiler data in {duration:.2f} seconds.")
                except:
                    _logger.error(
                        f"Failed to export PyTorch Profiler data.", exc_info=True)
                    
                finally:
                    pe.disable_no_gil_for_torch_profile_export()

        @_pending_call_if(is_step_mode)
        def start():
            need_to_wait = is_step_mode
            try:
                if not do_start.is_origin():
                    return

                if is_step_mode:
                    with self.lock:
                        if self.tp_state is not TP_STARTING:
                            return

                if should_start_stop_tp_on_main_thread:
                    if threading.current_thread() == threading.main_thread():
                        need_to_wait = False
                        do_start()
                    else:
                        if not _PendingCall.register_for_main_thread(do_start):
                            raise _AddPendingCallError(True)
                else:
                    threading.Thread(target=do_start, args=(
                        True,), name=f"PyTorch Profiler Controller - {request.id}").start()
            except Exception as e:
                handle_error(e)
                return

            success = False
            if need_to_wait:
                with self.lock:
                    self.wait_until_tp_state_not(TP_STARTING, request.args.start_timeout)
                    success = self.tp_state is TP_START_SUCCESS
                if success:
                    start_auxiliary_tracers_event.wait()

        def resolve_pre_timeout_state():
            with self.lock:
                if self.tp_state is TP_START_PRE_TIMEOUT:
                    cancel_starting()
                return self.tp_state

        def cancel_starting():
            # Note: this method should be called with self.lock held
            if do_start.try_to_cancel():
                reset()
                if is_step_mode:
                    start.cancel_or_wait()
                _logger.warning(
                    "Cancelled starting PyTorch Profiler due to timeout.")
                request.set_error(TP_START_TIMEOUT_CANCELLED_ERROR)
            else:
                self.transit_tp_state(self.tp_state, TP_START_TIMEOUT)
                request.set_error(TP_START_TIMEOUT_ERROR)

        def wait_for_starting():
            with self.lock:
                if not self.wait_until_tp_state_not(TP_STARTING, request.args.start_timeout):
                    if self.tp_state is TP_STARTING:
                        if is_duration_mode and request.use_auxiliary_tracers:
                            self.transit_tp_state(
                                self.tp_state, TP_START_PRE_TIMEOUT)
                        else:
                            cancel_starting()
                return self.tp_state

        @_pending_call_if(is_step_mode)
        def stop():
            need_to_wait = is_step_mode
            try:
                if not do_stop.is_origin():
                    return

                with self.lock:
                    if self.tp_state is not TP_START_SUCCESS:
                        return
                    self.transit_tp_state(self.tp_state, TP_STOPPING)

                request.update_torch_profiler_stop_timeout(time.time())

                if should_start_stop_tp_on_main_thread:
                    if threading.current_thread() == threading.main_thread():
                        need_to_wait = False
                        do_stop()
                    else:
                        if not _PendingCall.register_for_main_thread(do_stop):
                            compensate_stop()
            except Exception as e:
                handle_error(e)
                return

            if need_to_wait:
                stop_auxiliary_tracers_event.wait()
                with self.lock:
                    self.wait_until_tp_state_not(TP_STOPPING, request.torch_profiler_stop_timeout)

        def compensate_stop():
            try:
                def compensate():
                    while not _PendingCall.register_for_main_thread(do_stop):
                        _logger.warning("Failed to compensate stop pending call, retrying after 1 second...")
                        time.sleep(1)
                    _logger.warning("Success to compensate stop pending call.")
                threading.Thread(target=compensate, name=f"Compensation thread for stop pending call").start()
            except:
                raise _AddPendingCallError(False)

        def wait_for_stoppping():
            with self.lock:
                if not self.wait_until_tp_state_not(TP_STOPPING, request.torch_profiler_stop_timeout):
                    if self.tp_state is TP_STOPPING:
                        self.transit_tp_state(TP_STOPPING, TP_STOP_TIMEOUT)
                        request.set_error(TP_STOP_TIMEOUT_ERROR)
                state = self.tp_state

            if is_step_mode:
                stop.cancel_or_wait()

            return state

        try:
            if request.stop_on_cuda_oom:
                self.register_cuda_oom_observer()
                self.cuda_oom_callback = on_cuda_oom

            if request.use_torch_profile and not self.check_available(request):
                run_auxiliary_tracers_directly()
                return

            should_start_stop_tp_on_main_thread = False
            if not request.use_torch_profile:
                should_start_stop_tp_on_main_thread = False
            elif request.args.profile_memory:
                _logger.info(
                    "Should start PyTorch Profiler on main thread since profile_memory is enabled.")
                should_start_stop_tp_on_main_thread = True
            elif (ProfilerActivity.CPU in request.args.activities and
                    not self.torch_feature_support.is_feature_supported("experiment_config.profile_all_threads")):
                should_start_stop_tp_on_main_thread = True
            request.set_should_run_on_main_thread(should_start_stop_tp_on_main_thread)

            _logger.info(
                f"Mode: {'Step' if is_step_mode else 'Duration'}. Hook type: {iteration_hook.__class__.__name__}.")

            if is_step_mode:
                iteration_hook.set_start_stop_func(start, stop)

            iteration_hook.set_add_metadata_func(add_metadata)

            with self.lock:
                assert self.tp_state is TP_ORIGIN
                self.transit_tp_state(TP_ORIGIN, TP_STARTING)

            activate_hooks()

            if is_duration_mode:
                start()
                state = wait_for_starting()
                if state is not TP_START_SUCCESS and state is not TP_START_PRE_TIMEOUT:
                    run_auxiliary_tracers_directly()
                    return
                start_auxiliary_tracers()
                call_on_profile_start()
                duration_interrupt_event.wait(request.args.duration)
                if state is TP_START_PRE_TIMEOUT:
                    state = resolve_pre_timeout_state()
                stop_auxiliary_tracers()
                call_on_profile_stop()
                if state is TP_START_SUCCESS:
                    stop()
                    state = wait_for_stoppping()
                    call_on_profile_stop()
                    if state is TP_STOP_SUCCESS:
                        export_data()
                        reset()
                export_data_of_auxiliary_tracers()
            else:
                state = wait_for_starting()
                if state is not TP_START_SUCCESS:
                    stop.cancel_or_wait()
                    return
                start_auxiliary_tracers()
                call_on_profile_start()
                stop_now = False
                with self.lock:
                    if not self.wait_until_tp_state_not(TP_START_SUCCESS, request.torch_profiler_timeout):
                        if self.tp_state is TP_START_SUCCESS:
                            stop_now = True
                stop_auxiliary_tracers()
                call_on_profile_stop()
                if stop_now:
                    stop()
                state = wait_for_stoppping()
                if state is TP_STOP_SUCCESS:
                    export_data()
                    reset()
                export_data_of_auxiliary_tracers()
        except Exception as e:
            handle_error(e)
        finally:
            deactivate_hooks()
            self.cuda_oom_callback = None
            # make sure auxiliary tracers are stopped and reset
            stop_auxiliary_tracers()
            call_on_profile_stop()
            reset_auxiliary_tracers()

    def execute(self, **kwargs):
        with self.lock:
            if not _hook_record_function_fast():
                return Result.from_error("Failed to hook RecordFunctionFast.")
            if self.active:
                return Result.from_error("Previous request is not finished.")
            self.active = True
        id = self.next_request_id
        self.next_request_id += 1
        try:
            request = _Request(id, _Arguments(**kwargs))
            _logger.info(f"Start to execute torch_profiler command. [{id}]")
            _logger.info(f"Args: {request.args.__dict__}.")
            self.do_execute(request)
            result = Result.from_request(request)
            _logger.info(f"Request [{request.id}] completed with result:\n{result}")
            return result
        except Exception as e:
            _logger.error(f"Exception occurred. [{id}]", exc_info=True)
            return Result.from_error(e)
        finally:
            _logger.info(f"Finish executing torch_profiler command. [{id}]")
            with self.lock:
                self.active = False


_hook_record_function_fast_result = None


def _hook_record_function_fast():
    global _hook_record_function_fast_result
    if isinstance(_hook_record_function_fast_result, bool):
        return _hook_record_function_fast_result
    if hasattr(torch._C, "_profiler") and hasattr(torch._C._profiler, "_RecordFunctionFast"):  # type: ignore
        _hook_record_function_fast_result = hook_record_function_fast(torch._C._profiler._RecordFunctionFast)  # type: ignore
    else:
        _hook_record_function_fast_result = True
    return _hook_record_function_fast_result

@run_once
def _cupti_use_system_tid():
    try:
        if torch.cuda.is_available():
            device_name = torch.cuda.get_device_name(0)
            if device_name and "NVIDIA" in device_name.upper():
                # import torch的时候已经加载了libcupti.so，我们直接进行查找
                cupti_lib_path = find_lib("libcupti.so")
                if cupti_lib_path is not None:
                    cuptiSetThreadIdType_wrapper(cupti_lib_path)
                    _logger.info("Use system tid for CUPTI.")
                    return
    except Exception as e:
        _logger.warning(f"Failed to set CUPTI thread id type: {e}")



_service_instance = _Service()


def execute(**kwargs) -> Result:
    return _service_instance.execute(**kwargs)
