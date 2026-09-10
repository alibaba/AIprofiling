from dataclasses import dataclass
from enum import Enum
from pyki.error import raise_error
from pyki.profiling.parser import get_start_end_time
from pyki.native import pyki_extension as ext
from pyki.utils import SingletonMeta
from threading import Condition, Lock, Thread
from typing import Callable, Dict, Optional

import atexit
import logging
import os
import time

__all__ = [
    "CPUOptions",
    "MemoryOptions",
    "NativeMemoryOptions",
    "PPROFSampleType",
    "Profile",
    "ProfileFormat",
    "Profiler",
    "State",
    "profiler",
    "start"
]


class ProfileFormat(Enum):
    """Represents the format of a profile.
    """

    RAW = "RAW"
    PPROF = "PPROF"

    @classmethod
    def from_string(cls, s: str) -> "ProfileFormat":
        """
        Parse a string to ProfileFormat in a case-insensitive way.
        Raises ValueError if the string does not match any format.
        """
        s = s.strip().upper()
        for fmt in cls:
            if fmt.value == s:
                return fmt
        raise ValueError(f"Unknown ProfileFormat: {s}")


class PPROFSampleType(Enum):
    """Represents the sample type of a pprof profile.
    """

    CPU = "CPU"
    MEMORY = "MEMORY"
    NATIVE_MEMORY = "NATIVE-MEMORY"


class Profile:
    """Represents a profile.
    """

    def __init__(self, path: str, format: ProfileFormat, start_time: int, end_time: int):
        self.path = path
        self.format = format

        self.start_time = start_time
        self.end_time = end_time

        self.sample_type: Optional[PPROFSampleType] = None

    def release(self):
        _clean(self.path)

    def _set_sample_type(self, sample_type: PPROFSampleType):
        self.sample_type = sample_type


@dataclass(frozen=True)
class Options:
    """Base options for a profiling type.

    Attributes:
        enabled (bool): A flag to enable or disable the profiling type.
                        Defaults to False.
    """
    enabled: bool = False


@dataclass(frozen=True)
class StackTraceOptions:
    """Options for recording stack traces.

    Attributes:
        max_depth (int): The maximum depth to record.
                         Defaults to 64.
        native (bool): Whether to record native frame.
                       Defaults to False.
    """
    max_depth: int = 64
    native: bool = False

    def __post_init__(self):
        if self.max_depth <= 0 or self.max_depth > 128:
            raise_error(f"max_depth is illegal: {self.max_depth}")


@dataclass(frozen=True)
class CPUOptions(Options, StackTraceOptions):
    """Options for CPU profiling.

    Attributes:
        interval_ms (int): The sampling interval in milliseconds.
                           Defaults to 10 ms.
                           Valid range is (5, 1000].
    """
    interval_ms: int = 10

    def __post_init__(self):
        StackTraceOptions.__post_init__(self)
        if self.interval_ms <= 5 or self.interval_ms > 1000:
            raise_error(f"interval_ms is illegal: {self.interval_ms}")


@dataclass(frozen=True)
class MemoryOptions(Options, StackTraceOptions):
    """Options for memory profiling.

    Attributes:
        interval_bytes (int): The sampling interval in bytes.
                              Defaults to 512 KB.
                              Valid range is (0, 32 MB].
    """
    interval_bytes: int = 512 * 1024

    def __post_init__(self):
        StackTraceOptions.__post_init__(self)
        if self.interval_bytes <= 0 or self.interval_bytes > 32 * 1024 * 1024:
            raise_error(f"interval_bytes is illegal: {self.interval_bytes}")

@dataclass(frozen=True)
class NativeMemoryOptions(MemoryOptions):
    """Options for native memory profiling.

    Attributes:
        leaks (bool): If False, all native allocations are recorded.
                      If True, only leaked (allocated but not freed) allocations are recorded.
    """
    leaks: bool = True

    def __post_init__(self):
        MemoryOptions.__post_init__(self)

_logger = logging.getLogger(__name__)

_default_profile_dir = os.path.join(
    os.getcwd(), ".pyki_profiles", str(os.getpid()))

_profile_file_prefix = f"profile-{os.getpid()}-{int(time.time() * 1000)}-"


def _clean(path):
    try:
        if os.path.exists(path):
            os.remove(path)
            _logger.info("Delete profile: %s", path)
    except Exception as e:
        _logger.warning(
            "Failed to delete profile %s: %s", path, e)


class State(Enum):
    """Represents the current state of the profiler.
    """

    UNSTARTED = "UNSTARTED"
    STARTED = "STARTED"
    ERROR = "ERROR"


class Profiler(metaclass=SingletonMeta):
    """Profiler for Python applications.
    """

    def __init__(self):
        self._cpu_options: CPUOptions = CPUOptions()
        self._new_cpu_options: Optional[CPUOptions] = None

        self._memory_options: MemoryOptions = MemoryOptions()
        self._new_memory_options: Optional[MemoryOptions] = None

        self._native_memory_options: NativeMemoryOptions = NativeMemoryOptions()
        self._new_native_memory_options: Optional[NativeMemoryOptions] = None

        self._profile_format: ProfileFormat = ProfileFormat.RAW
        self._profile_dir: str = _default_profile_dir
        self._profile_listener: Callable[[
            Profile], None] = lambda profile: None
        self._profile_rotation_period: int = -1
        self._compress: bool = False
        self._duration: int = -1

        self._error: Optional[Exception] = None
        self._thread: Optional[Thread] = None

        self._lock = Lock()
        self._condition = Condition(self._lock)

        self._next_profile_id = 1

        self._should_stop = False
        self._state = State.UNSTARTED

    def start(self, *,
              cpu_options: CPUOptions = CPUOptions(enabled=False),
              memory_options: MemoryOptions = MemoryOptions(enabled=False),
              native_memory_options: NativeMemoryOptions = NativeMemoryOptions(enabled=False),

              profile_format: ProfileFormat = ProfileFormat.RAW,
              profile_dir: str = _default_profile_dir,
              profile_listener: Callable[[Profile], None],
              profile_rotation_period: int = -1,
              compress: bool = False,
              duration: int = -1,
              ):
        """Starts the profiler.

        Args:
            cpu_options (CPUOptions): Options for CPU profiling.
            memory_options (MemoryOptions): Options for memory profiling.
            native_memory_options (NativeMemoryOptions): Options for native memory profiling.
            profile_format (ProfileFormat): Format of the profile.
            profile_dir (str, optional): Directory to save profiles.
                                         Defaults to _default_profile_dir.
            profile_listener (Callable[[Profile], None]): Listener to process profiles.
            profile_rotation_period (int, optional): Period to rotate profiles in seconds.
                                                     Defaults to -1 (no rotation).
                                                     Valid range is -1 or [1, 3600].
            compress (bool, optional): Whether to compress the profile files, only for PPROF format.
                                      Defaults to False.
            duration (int, optional): Duration to run the profiler in seconds.
                                      Defaults to -1 (continuous).
                                      Valid range is -1 or [1, 3600].
        """
        if not cpu_options.enabled and not memory_options.enabled and not native_memory_options.enabled:
            raise_error("At least one profiling type is enabled")

        if type(profile_rotation_period) != int:
            raise_error("profile_rotation_period shoud be a int")

        if profile_rotation_period != -1 and (profile_rotation_period < 1 or profile_rotation_period > 3600):
            raise_error(
                f"profile_rotation_period is illegal: {profile_rotation_period}")

        if type(duration) != int:
            raise_error("duration shoud be a int")

        if duration != -1 and (duration < 1 or duration > 3600):
            raise_error(f"duration is illegal: {duration}")

        if not os.path.exists(profile_dir):
            os.makedirs(profile_dir)
        else:
            if not os.path.isdir(profile_dir):
                raise_error("%s is not a directory" % profile_dir)

        with self._condition:
            if self._state != State.UNSTARTED:
                raise_error(f"current state is {self._state}")

            try:
                self._profile_format = profile_format
                self._profile_dir = profile_dir
                self._profile_listener = profile_listener
                self._profile_rotation_period = profile_rotation_period
                self._compress = compress
                self._duration = duration

                self._cpu_options = cpu_options
                self._memory_options = memory_options
                self._native_memory_options = native_memory_options

                self._thread = Thread(
                    name="PyKi Profiler Thread", target=self._run, daemon=True)

                self._thread.start()
                self._condition.wait()
            except Exception as e:
                _logger.error(
                    f"Failed to start profiler: {e}", exc_info=True)
                self._reset_options()
                self._state = State.ERROR

            if self._state != State.STARTED:
                raise_error("Failed to start profiler")

    def stop(self):
        """Stops the profiler.
        """
        with self._condition:
            if self._state != State.STARTED:
                raise_error(f"Profiler is not started")
            self._should_stop = True
            self._condition.notify()
            self._condition.wait()

            if self._state != State.UNSTARTED:
                raise_error("Failed to stop profiler")

    def update_options(self, *,
                       cpu_options: Optional[CPUOptions],
                       memory_options: Optional[MemoryOptions],
                       native_memory_options: Optional[NativeMemoryOptions]):
        """Update profiling options.

        Args:
            cpu_options (Optional[CPUOptions]): New CPU profiling options.
            memory_options (Optional[MemoryOptions]): New memory profiling options.
            native_memory_options (Optional[NativeMemoryOptions]): New native memory profiling options.
        """
        if cpu_options is None and memory_options is None and native_memory_options is None:
            raise_error("No option to update")

        with self._condition:
            if self._new_cpu_options is not None or self._new_memory_options is not None or self._native_memory_options is not None:
                raise_error("Previous options update is still pending")

            self._new_cpu_options = cpu_options
            self._new_memory_options = memory_options
            self._new_native_memory_options = native_memory_options

            self._condition.notify()

    def get_state(self) -> State:
        return self._state

    def get_options(self) -> Dict[str, Options]:
        return {
            "cpu_options": self._cpu_options,
            "memory_options": self._memory_options,
            "native_memory_options": self._native_memory_options,
        }

    def _reset_options(self):
        self._cpu_options = CPUOptions(enabled=False)
        self._new_cpu_options = None
        self._memory_options = MemoryOptions(enabled=False)
        self._new_memory_options = None
        self._native_memory_options = NativeMemoryOptions(enabled=False)
        self._new_native_memory_options = None

    def _should_update_options(self) -> bool:
        return self._new_cpu_options is not None or self._new_memory_options is not None or self._native_memory_options is not None

    def _run(self):
        current_path = ""
        current_fd = -1
        listener = self._profile_listener
        _native_memory_leaks = self._native_memory_options.leaks

        def new_profile():
            nonlocal current_path
            nonlocal current_fd

            current_path = os.path.join(
                self._profile_dir, _profile_file_prefix + str(self._next_profile_id))
            current_fd = os.open(current_path, os.O_CREAT | os.O_WRONLY, 0o644)
            self._next_profile_id += 1

        def dump(rotate):
            nonlocal current_path
            nonlocal current_fd

            prev_path = current_path
            prev_fd = current_fd
            try:
                if rotate:
                    new_profile()
                    ext.rotate_to(current_fd)
                else:
                    current_path = ""
                    current_fd = -1
                    ext.rotate_to(-1)
            finally:
                os.close(prev_fd)
            return prev_path

        def process_profile(path: str):
            try:
                start_time, end_time = get_start_end_time(path)
            except Exception as e:
                _logger.error(
                    f"Failed to get start/end time from profile {path}: {e}", exc_info=True)
                _clean(path)
                return

            profiles = []
            if self._profile_format == ProfileFormat.RAW:
                profiles.append(Profile(path, self._profile_format,
                                        start_time, end_time))
            else:
                cpu_pprof_path = path + ".cpu.pprof" + \
                    (".gz" if self._compress else "")
                allocation_pprof_path = path + ".allocation.pprof" + \
                    (".gz" if self._compress else "")
                native_memory_pprof_path = path + ".native-memory.pprof" + \
                    (".gz" if self._compress else "")
                try:
                    ext.convert_to_pprof_files(input_path=path,
                                               cpu_pprof_path=cpu_pprof_path,
                                               allocation_pprof_path=allocation_pprof_path,
                                               native_memory_pprof_path=native_memory_pprof_path,
                                               native_memory_leaks=_native_memory_leaks,
                                               compress=self._compress)
                except Exception as e:
                    _logger.error(
                        "Failed to convert profile to pprof format", exc_info=True)
                    _clean(path)
                    _clean(cpu_pprof_path)
                    _clean(allocation_pprof_path)
                    _clean(native_memory_pprof_path)
                    return

                if (os.path.exists(cpu_pprof_path)):
                    cpu_profile = Profile(
                        cpu_pprof_path, ProfileFormat.PPROF, start_time, end_time)
                    cpu_profile._set_sample_type(PPROFSampleType.CPU)
                    profiles.append(cpu_profile)

                if (os.path.exists(allocation_pprof_path)):
                    memory_profile = Profile(
                        allocation_pprof_path, ProfileFormat.PPROF, start_time, end_time)
                    memory_profile._set_sample_type(PPROFSampleType.MEMORY)
                    profiles.append(memory_profile)

                if (os.path.exists(native_memory_pprof_path)):
                    native_memory_profile = Profile(
                        native_memory_pprof_path, ProfileFormat.PPROF, start_time, end_time)
                    native_memory_profile._set_sample_type(PPROFSampleType.NATIVE_MEMORY)
                    profiles.append(native_memory_profile)

                _clean(path)

            for profile in profiles:
                try:
                    listener(profile)
                except Exception as e:
                    _logger.warning(
                        f"Failed to call profile listener {path}: {e}", exc_info=True)

        def update_options(current_options, new_options, profiling_wrapper):
            if current_options.enabled:
                if new_options.enabled:
                    profiling_wrapper.update_options(new_options)
                else:
                    profiling_wrapper.stop()
            else:
                if new_options.enabled:
                    profiling_wrapper.start(new_options)

        end_time = -1.0

        with self._condition:
            try:
                new_profile()
                ext.set_profiling_data_file(current_fd)
                if self._duration > 0:
                    end_time = time.time() + self._duration

                if self._cpu_options.enabled:
                    _cpu_profiling_wrapper.start(self._cpu_options)

                # ! important !
                # If both heap and native memory profiler are started, the stop order should be reversed
                # start:
                #   heap: pymalloc -> heap_hook
                #   native: heap_hook -> native_hook
                # stop: (this order ensures that the allocator can eventually be restored to pymalloc)
                #   native: native_hook -> heap_hook
                #   heap: heap_hook -> pymalloc
                if self._memory_options.enabled:
                    _memory_profiling_wrapper.start(self._memory_options)

                if self._native_memory_options.enabled:
                    _native_memory_profiling_wrapper.start(self._native_memory_options)

                self._state = State.STARTED
                self._condition.notify()
                _logger.info(
                    f"Profiling is started. Data files are saved in {self._profile_dir}")
            except Exception as e:
                _logger.error(f"Failed to start profiler: {e}", exc_info=True)
                self._state = State.ERROR
                self._condition.notify()
                return

        with self._condition:
            rotation_period = self._profile_rotation_period

            while True:
                stop_now = False
                wait_time = float(rotation_period)
                if end_time > 0:
                    remaining_duration = end_time - time.time()
                    if remaining_duration <= 0:
                        stop_now = True
                    elif rotation_period > 0:
                        wait_time = min(remaining_duration, rotation_period)
                    else:
                        wait_time = remaining_duration

                if not stop_now:
                    self._condition.wait(
                        timeout=wait_time if wait_time > 0 else None)
                    if self._should_stop or (end_time > 0 and time.time() >= end_time):
                        stop_now = True

                current_cpu_options = self._cpu_options
                current_memory_options = self._memory_options
                current_native_memory_options = self._native_memory_options

                if stop_now:
                    if current_cpu_options.enabled:
                        _cpu_profiling_wrapper.stop()

                    # ! important !
                    # If both heap and native memory profiler are started, the stop order should be reversed
                    # see above for details
                    if current_native_memory_options.enabled:
                        _native_memory_profiling_wrapper.stop()

                    if current_memory_options.enabled:
                        _memory_profiling_wrapper.stop()

                elif self._should_update_options():
                    new_cpu_options = self._new_cpu_options
                    if new_cpu_options is not None:
                        update_options(current_cpu_options,
                                       new_cpu_options,
                                       _cpu_profiling_wrapper)
                        self._cpu_options = new_cpu_options
                        self._new_cpu_options = None

                    new_memory_options = self._new_memory_options
                    if new_memory_options is not None:
                        update_options(current_memory_options,
                                       new_memory_options,
                                       _memory_profiling_wrapper)
                        self._memory_options = new_memory_options
                        self._new_memory_options = None

                    new_native_memory_options = self._new_native_memory_options
                    if new_native_memory_options is not None:
                        update_options(current_native_memory_options,
                                       new_native_memory_options,
                                       _native_memory_profiling_wrapper)
                        self._native_memory_options = new_native_memory_options
                        self._new_native_memory_options = None

                    if not self._cpu_options.enabled and not self._memory_options.enabled and not self._native_memory_options.enabled:
                        stop_now = True

                profile_path = dump(rotate=not stop_now)
                Thread(target=process_profile, args=(profile_path,),
                       daemon=True).start()

                if stop_now:
                    self._reset_options()
                    self._should_stop = False
                    self._state = State.UNSTARTED
                    self._condition.notify()
                    return


class _ProfilingWrapper:

    def __init__(self, start_func, update_options_func, stop_func):
        self._start_func = start_func
        self._update_options_func = update_options_func
        self._stop_func = stop_func

    def start(self, options):
        self._start_func(options)

    def update_options(self, options):
        self._update_options_func(options)

    def stop(self):
        self._stop_func()


def _update_cpu_profiling_options(interval_ms, max_depth, native):
    ext.stop_cpu_profiling()
    ext.start_cpu_profiling(interval_ms=interval_ms,
                            max_depth=max_depth,
                            native=native)


def _update_memory_profiling_options(interval_bytes, max_depth, native):
    ext.stop_memory_profiling()
    ext.start_memory_profiling(interval_bytes=interval_bytes,
                               max_depth=max_depth,
                               native=native)

def _update_native_memory_profiling_options(interval_bytes, max_depth, leaks):
    ext.stop_native_memory_profiling()
    ext.start_native_memory_profiling(interval_bytes=interval_bytes, max_depth=max_depth, leaks=leaks)

_cpu_profiling_wrapper = _ProfilingWrapper(
    start_func=lambda options: ext.start_cpu_profiling(interval_ms=options.interval_ms,
                                                       max_depth=options.max_depth,
                                                       native=options.native),
    update_options_func=lambda options: _update_cpu_profiling_options(
        options.interval_ms, options.max_depth, options.native),
    stop_func=ext.stop_cpu_profiling
)


_memory_profiling_wrapper = _ProfilingWrapper(
    start_func=lambda options: ext.start_memory_profiling(interval_bytes=options.interval_bytes,
                                                          max_depth=options.max_depth,
                                                          native=options.native),
    update_options_func=lambda options: _update_memory_profiling_options(
        options.interval_bytes, options.max_depth, options.native),
    stop_func=ext.stop_memory_profiling
)

_native_memory_profiling_wrapper = _ProfilingWrapper(
    start_func=lambda options: ext.start_native_memory_profiling(interval_bytes=options.interval_bytes,
                                                                 max_depth=options.max_depth,
                                                                 leaks=options.leaks),
    update_options_func=lambda options: _update_native_memory_profiling_options(
        options.interval_bytes, options.max_depth, options.leaks),
    stop_func=ext.stop_native_memory_profiling
)


def start(*,
          cpu_options: CPUOptions = CPUOptions(enabled=True),
          memory_options: MemoryOptions = MemoryOptions(enabled=True),
          native_memory_options: NativeMemoryOptions = NativeMemoryOptions(enabled=False),

          profile_format: ProfileFormat = ProfileFormat.RAW,
          profile_dir: str = _default_profile_dir,
          profile_listener: Callable[[Profile], None],
          profile_rotation_period: int = -1,
          compress: bool = False,
          duration: int = -1,
          ):

    class _WithWrapper:

        def __init__(self, **kwargs):
            self._profiler = Profiler()

            def start():
                self._profiler.start(**kwargs)
            self._start = start

        def __enter__(self) -> Profiler:
            self._start()
            return self._profiler

        def __exit__(self, exc_type, exc_value, traceback):
            if self._profiler.get_state() == State.STARTED:
                self._profiler.stop()

    return _WithWrapper(cpu_options=cpu_options,
                        memory_options=memory_options,
                        native_memory_options=native_memory_options,
                        profile_format=profile_format,
                        profile_dir=profile_dir,
                        profile_listener=profile_listener,
                        profile_rotation_period=profile_rotation_period,
                        compress=compress,
                        duration=duration)


profiler = Profiler()


def _on_exit():
    if profiler.get_state() == State.STARTED:
        profiler.stop()


atexit.register(_on_exit)
