import importlib.util
import inspect
import logging
import os.path
import pyki.constant
import sys
import tempfile
import threading
import time
import uuid

from abc import ABC

from pyki import ProfileFormat
from pyki.error import PyKiError, raise_error
from pyki.module_util import is_module_imported, get_package_version
from pyki.native import pyki_extension as pe
from pyki.process_util import get_rss
from pyki.utils import format_bytes
from typing import Any, Dict, Iterable, Optional

_lock = threading.Lock()
_commands = {}
_logger = logging.getLogger(__name__)


class _ArgumentInfo:
    def __init__(self, type, has_default_value, default_value, description):
        self.type = type
        self.has_default_value = has_default_value
        self.default_value = default_value
        self.description = description


class SpecialCommandArgument(ABC):
    args: Any = None

    def __init(self, s):
        self.s = s

    def to_string(self) -> str:
        return self.s

    @staticmethod
    def set_args(args):
        SpecialCommandArgument.args = args

    @staticmethod
    def get_args():
        return SpecialCommandArgument.args


class PathArgument(SpecialCommandArgument):

    def __init__(self, s):
        self.s = s
        self.tmp_path = None

    def to_string(self) -> str:
        return self.get_final_path()

    def get_final_path(self) -> str:
        # the file will finally be sent to host
        args = SpecialCommandArgument.get_args()
        path = self.s
        if not os.path.isabs(path):
            working_dir = SpecialCommandArgument.get_args()[
                "working_dir"] or os.getcwd()
            path = os.path.join(working_dir, path)
        if "{PID}" in path:
            pid = args.get("host_pid", os.getpid())
            path = path.replace("{PID}", str(pid))
        return path

    def get_file_generation_path(self):
        # if this command is running in container, the file will be first be generated in /tmp
        args = SpecialCommandArgument.get_args()
        if not args.get("in_container", False):
            return self.get_final_path()
        if self.tmp_path is None:
            self.tmp_path = os.path.join(
                tempfile.gettempdir(), str(uuid.uuid4()))
        return self.tmp_path

    def get_move_to_host_command(self):
        args = SpecialCommandArgument.get_args()
        if not args.get("in_container", False):
            return None
        return {
            "name": "move_file_from_container",
            "args": {
                "src": f"/proc/{args['host_pid']}/root/{self.get_file_generation_path()}",
                "dest": self.get_final_path()
            }
        }


class DirArgument(SpecialCommandArgument):

    def __init__(self, s):
        self.s = s
        self.tmp_path = None
        self.files = []

    def to_string(self) -> str:
        return self.get_dest_dir()

    def get_dest_dir(self) -> str:
        args = SpecialCommandArgument.get_args()
        path = self.s
        if not os.path.isabs(path):
            working_dir = SpecialCommandArgument.get_args()[
                "working_dir"] or os.getcwd()
            path = os.path.join(working_dir, path)
        return path

    def resolve_dir(self):
        args = SpecialCommandArgument.get_args()
        if not args.get("in_container", False):
            return self.get_dest_dir()
        if self.tmp_path is None:
            self.tmp_path = tempfile.gettempdir()
        return self.tmp_path

    def add_filenames(self, filenames):
        self.files.extend(filenames)

    def gen_move_to_host_commands(self):
        args = SpecialCommandArgument.get_args()
        if not args.get("in_container", False):
            return []
        commands = []
        src_dir = self.resolve_dir()
        dest_dir = self.get_dest_dir()
        for filename in self.files:
            commands.append({
                "name": "move_file_from_container",
                "args": {
                    "src": f"/proc/{args['host_pid']}/root/{src_dir}/{filename}",
                    "dest": f"{dest_dir}/{filename}"
                }
            })
        return commands


class _CommandInfo:
    def __init__(self, name, description, func, argument_info_dict: Dict[str, _ArgumentInfo],
                 dependencies: Iterable[str]):
        self.name = name
        self.description = description
        self.func = func
        self.argument_info_dict = argument_info_dict
        self.dependencies = dependencies


def _build_command_info(name, func, args_description, dependencies) -> _CommandInfo:
    spec = inspect.getfullargspec(func)
    if len(spec.args) > 0:
        raise_error("length of args should be 0")

    if spec.varargs is not None:
        raise_error("varrags should be None")

    if spec.varkw is not None:
        raise_error("varkw should be None")

    d = {}
    if spec.kwonlyargs:
        annos = spec.annotations
        defaults = spec.kwonlydefaults
        for arg in spec.kwonlyargs:
            if arg not in annos:
                raise_error(f"missing type hint for argument '{arg}'")
            t = annos[arg]
            if ((t is not int) and (t is not float) and (t is not bool) and (t is not str)
                    and not issubclass(t, SpecialCommandArgument)):
                raise_error(
                    f"argument '{arg}' should be int, float, bool, str or SpecialCommandArgument")
            has_default_value = False
            default_value = None
            if defaults is not None and arg in defaults:
                has_default_value = True
                default_value = defaults[arg]
            # python 不像c一样局部变量每个循环重新初始化，所以要重新赋值
            description = None
            if args_description is not None and arg in args_description:
                description = args_description[arg]
            d[arg] = _ArgumentInfo(t, has_default_value,
                                   default_value, description)
    doc = func.__doc__
    if doc is None:
        doc = ""
    else:
        doc = doc.strip()
    return _CommandInfo(name, doc, func, d, dependencies)


def _register(name: str, func, args_description: Optional[Dict[str, str]], dependencies: Optional[Iterable[str]]):
    with _lock:
        if name in _commands:
            _logger.error(
                f"Failed to register command '{name}': already exists")
        try:
            _commands[name] = _build_command_info(
                name, func, args_description, dependencies)
        except Exception as e:
            _logger.error(f"Failed to register command '{name}': {e}")


def _get_command(name: str) -> _CommandInfo:
    with _lock:
        if name not in _commands:
            raise_error(f"Command '{name}' not found")
        return _commands[name]


def command(name: Optional[str] = None, args_description: Optional[Dict[str, str]] = None, dependencies: Optional[Iterable[str]] = None):
    def command_wrapper(func):
        func_name = name if name is not None else func.__name__
        _register(func_name, func, args_description, dependencies)
        return func

    return command_wrapper


def _check_command_dependencies(dependencies):
    if dependencies is None:
        return
    for d in dependencies:
        spec = importlib.util.find_spec(d)
        if spec is None:
            raise_error(f"Dependency {d} not found on client")


def run_command(name: str, **kwargs):
    # check arguments
    cmd = _get_command(name)
    _check_command_dependencies(cmd.dependencies)
    argument_info_dict = cmd.argument_info_dict

    for k, v in argument_info_dict.items():
        if not v.has_default_value and (kwargs is None or k not in kwargs):
            raise_error(f"Missing argument '{k}'")

    args = {}
    if kwargs is not None:
        for k, v in kwargs.items():
            if k == pyki.constant.ADDITIONAL_ARGS:
                SpecialCommandArgument.set_args(v)
                continue
            if k not in argument_info_dict:
                raise_error(f"Invalid argument '{k}'")
            t = argument_info_dict[k].type
            if isinstance(v, t):
                args[k] = v
                continue
            elif isinstance(v, str):
                try:
                    if t is int:
                        args[k] = int(v)
                        continue
                    elif t is float:
                        args[k] = float(v)
                        continue
                    elif t is bool:
                        if "true" == v.lower():
                            args[k] = True
                            continue
                        elif "false" == v.lower():
                            args[k] = False
                            continue
                    elif issubclass(t, SpecialCommandArgument):
                        args[k] = t(v)
                        continue
                except Exception:
                    pass
            raise_error(f"Invalid argument '{k}'")
    return cmd.func(**args)


@command()
def list_commands():
    """list all commands pyki supports
    """
    output = ""
    with _lock:
        for name, cmd in _commands.items():
            output += (f"[{name}]\n")
            if cmd.description is not None:
                output += f"description:\n"
                output += f"  {cmd.description}\n"
            args = cmd.argument_info_dict
            if len(args) > 0:
                output += f"arguments:\n"

                max_prefix_length = 0
                prefixes = []
                descriptions = []
                for arg_name, arg_info in args.items():
                    prefix = f"  {arg_name}:"
                    if issubclass(arg_info.type, SpecialCommandArgument):
                        prefix += str.__name__
                    else:
                        prefix += arg_info.type.__name__
                    if arg_info.has_default_value:
                        default_value = arg_info.default_value
                        if arg_info.type == str:
                            default_value = f"\"{default_value}\""
                        elif issubclass(arg_info.type, SpecialCommandArgument):
                            default_value = default_value.to_string()
                        prefix += f" = {default_value}"
                    max_prefix_length = max(max_prefix_length, len(prefix))
                    prefixes.append(prefix)
                    descriptions.append(arg_info.description)

                for i in range(len(prefixes)):
                    prefix = prefixes[i]
                    description = descriptions[i]
                    output += f"{prefix}"
                    if description is not None:
                        output += " " * (max_prefix_length - len(prefix))
                        output += f"    {description}"
                    output += "\n"
            else:
                output += "arguments:\n  no argument\n"
            output += "\n"
    return output


def check_torch_version():
    from pyki.packaging import version
    torch_version = get_package_version("torch")
    if torch_version is None:
        return False, "Unsupported PyTorch Version No Version Found"
    va = version.parse(torch_version)
    ve = version.parse("2.1.0")
    if va < ve:
        return False, f"Unsupported PyTorch Version {torch_version}"
    return True, None


@command(args_description={
    "path": "the directory path to save data files (default: current working directory)",
    "prefix": "prefix of data filenames",
    "merge": "merge data files into one file",
    "compress": "compress the merged data file",
    "activities": "PyTorch Profiler activities (separated by \",\", only \"cpu\" and \"cuda\"/\"gpu\" are supported now)",
    "record_shapes": "PyTorch Profiler record_shapes",
    "profile_memory": "PyTorch Profiler profile_memory",
    "with_flops": "PyTorch Profiler with_flops",
    "with_modules": "PyTorch Profiler with modules",
    "with_stack": "enable Python function trace",
    "with_nvtx": "enable NVTX trace",
    "timeout": "timeout for running PyTorch Profiler as step mode, unit is second(s)",
    "start_timeout": "timeout for strating PyTorch Profiler, unit is second(s)",
    "duration": "run PyTorch Profiler as duration mode, unit is second(s)",
    "num_steps": "run PyTorch Profiler as step mode (0 which means running as duration mode)",
    "num_skip_steps": "number of steps to skip in step mode",
    "module": "module to hook in step mode",
    "function": "function to hook in step mode",
    "python_tracer_max_depth": "max depth for Python function trace (0 means no limit)",
    "python_tracer_threshold_ns": "threshold for Python function trace in ns (0 means no threshold)",
    "python_tracer_ignore_c_functions": "ignore C functions for Python function trace",
    "cpu_op_probability": "probability of sampling CPU operations",
    "with_torch_cuda_memory_trace": "use torch.cuda.memory_tracer() to trace cuda memory usage",
    "torch_cuda_memory_trace_max_entry": "max entry number for torch.cuda.memory_tracer() (0 means adaptive)",
    "stop_on_cuda_oom": "stop profiling on CUDA out of memory",
    "export_memory_timeline": "export memory timeline to file",
    "scenario": "specify the scenario (\"auto\" for auto-detection, \"train\" for training, \"vllm\" for vLLM inference, \"sglang\" for SGLang inference)",
}, dependencies=["torch"])
# Notice: arguments below should be sync with _Arguments.__init__() in torch_profile.py
def torch_profile(*,
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
                  ):
    """start torch profiler
    """
    if sys.version_info < (3, 7):
        return ("torch_profile is not supported on Python {}.{}. "
                "PyTorch dropped Python 3.6 support after 1.10, and pyki's "
                "Python tracer / nvtx integration require Python 3.7+. "
                "Please use Python 3.7 or newer for torch_profile.").format(
                    sys.version_info[0], sys.version_info[1])
    if not is_module_imported("torch"):
        return "PyTorch has not been imported before. Torch profile is not allowed."
    ok, msg = check_torch_version()
    if not ok:
        return msg
    try:
        dir_arg = DirArgument(path)
        resolved_dir = dir_arg.resolve_dir()
        if not os.path.exists(resolved_dir):
            try:
                os.makedirs(resolved_dir, exist_ok=True)
            except:
                return "Path does not exist and cannot be created."
        elif not os.path.isdir(resolved_dir):
            return "Path should be a directory."

        if os.access(resolved_dir, os.R_OK | os.W_OK | os.X_OK) is False:
            return f"Cannot access the path {resolved_dir}."

        from pyki.profiling.torch_profile import execute
        result = execute(path=resolved_dir,
                         prefix=prefix,
                         merge=merge,
                         compress=compress,
                         activities=activities,
                         record_shapes=record_shapes,
                         profile_memory=profile_memory,
                         with_flops=with_flops,
                         with_modules=with_modules,
                         with_stack=with_stack,
                         with_nvtx=with_nvtx,
                         timeout=timeout,
                         start_timeout=start_timeout,
                         duration=duration,
                         num_steps=num_steps,
                         num_skip_steps=num_skip_steps,
                         module=module,
                         function=function,
                         python_tracer_max_depth=python_tracer_max_depth,
                         python_tracer_threshold_ns=python_tracer_threshold_ns,
                         python_tracer_ignore_c_functions=python_tracer_ignore_c_functions,
                         cpu_op_probability=cpu_op_probability,
                         with_torch_cuda_memory_trace=with_torch_cuda_memory_trace,
                         torch_cuda_memory_trace_max_entry=torch_cuda_memory_trace_max_entry,
                         stop_on_cuda_oom=stop_on_cuda_oom,
                         export_memory_timeline=export_memory_timeline,
                         scenario=scenario,
                         )
    except Exception as e:
        if isinstance(e, PyKiError):
            return e.error_message
        else:
            raise e
    dir_arg.add_filenames(result.get_filenames())
    r = {
        "message": f"{result}",
        "commands": dir_arg.gen_move_to_host_commands()
    }
    del (result)
    return r


@command(args_description={
    "path": "path to save the trace result",
    "duration": "duration of the profiling in seconds"
}, dependencies=["vllm"])
def vllm_export_ot_trace(*,  # pragma: no cover
                         path: PathArgument = PathArgument("vllm_trace.json"),
                         duration: float = 60):
    """enable opentelemtry trace of vllm and export traces to file
    """
    import pyki.profiling.vllm as vl
    profiler = vl.vllm_ot_trace_profiler_instance
    config = vl.VllmOTTraceConfig(path.get_file_generation_path())
    profiler.start_profile(config)
    time.sleep(duration)
    profiler.stop_profile()
    return {
        "message": "vllm opentelemetry traces exported to " + path.get_final_path(),
        "commands": [path.get_move_to_host_command()]
    }


@command(args_description={
    "modules": "modules to check, separated by \",\""
})
def check_imports(*, modules: str = ""):
    """check if the modules are imported
    """
    for module in modules.split(","):
        if not is_module_imported(module):
            return "false"
    return "true"


@command(
    args_description={
        "path": "path to save profile data",
        "duration": "duration of the profiling in seconds",
        "interval": "CPU sample interval in milliseconds",
        "profile_format": "profile format"
    }
)
def profile_cpu(*, path: str, duration: int, interval: int = 10,
                profile_format: str = ProfileFormat.RAW.value):  # pragma: no cover
    from pyki.profiling import CPUOptions, Profile, profiler
    import shutil

    event = threading.Event()

    def process_data(data: Profile):
        shutil.copy(data.path, path)
        event.set()

    profiler.start(
        cpu_options=CPUOptions(enabled=True, interval_ms=interval),
        duration=duration,
        profile_listener=process_data,
        profile_format=ProfileFormat.from_string(profile_format)
    )

    event.wait()
    return "Profile data written to " + path

@command(
    args_description={
        "path": "path to save profile data",
        "duration": "duration of the profiling in seconds",
        "interval": "memory sample interval",
        "profile_format": "profile format"
    }
)
def profile_memory(*, path: str, duration: int, interval: int = 512 * 1024,
                   profile_format: str = ProfileFormat.RAW.value):  # pragma: no cover
    from pyki.profiling import MemoryOptions, Profile, profiler
    import shutil

    event = threading.Event()

    def process_data(data: Profile):
        shutil.copy(data.path, path)
        event.set()

    profiler.start(
        memory_options=MemoryOptions(enabled=True, interval_bytes=interval),
        duration=duration,
        profile_listener=process_data,
        profile_format=ProfileFormat.from_string(profile_format)
    )

    event.wait()
    return "Profile data written to " + path

@command(
    args_description={
        "path": "path to save profile data",
        "duration": "duration of the profiling session in seconds",
        "interval": "memory sample interval",
        "leaks": "profile memory leaks",
        "profile_format": "profile format"
    }
)
def profile_native_memory(*, path: str, duration: int, interval: int = 512 * 1024, leaks: bool = True,
                          profile_format: str = ProfileFormat.RAW.value):  # pragma: no cover
    from pyki.profiling import NativeMemoryOptions, Profile, profiler
    import shutil

    event = threading.Event()

    def process_data(data: Profile):
        shutil.copy(data.path, path)
        event.set()

    profiler.start(
        native_memory_options=NativeMemoryOptions(enabled=True, interval_bytes=interval, leaks=leaks),
        duration=duration,
        profile_listener=process_data,
        profile_format=ProfileFormat.from_string(profile_format)
    )

    event.wait()
    return "Profile data written to " + path

@command()
def gc():
    """trigger garbage collection
    """
    import gc
    begin_time = time.time()
    rss_before = get_rss()
    gc.collect()
    rss_after = get_rss()
    duration = time.time() - begin_time
    message = f"Garbage collection done in {duration:.3f} seconds."
    if rss_before >= 0 and rss_after >= 0:
        message += f"\nRSS before: {format_bytes(rss_before)}, after: {format_bytes(rss_after)}."
    else:
        message += "\nRSS information is not available."
    _logger.info(message)
    return message


@command()
def trim_memory():
    """trim native memory
    """
    begin_time = time.time()
    rss_before = get_rss()
    pe.trim_native_memory()
    rss_after = get_rss()
    duration = time.time() - begin_time
    message = f"Native memory trimmed in {duration:.3f} seconds."
    if rss_before >= 0 and rss_after >= 0:
        message += f"\nRSS before: {format_bytes(rss_before)}, after: {format_bytes(rss_after)}."
    else:
        message += "\nRSS information is not available."
    _logger.info(message)
    return message