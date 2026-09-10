from pyki.perfdata.writer import Metric, register_metric, perfdata_writer
from pyki.perfdata.common import str_type, int_type, float_type, get_datatype_from_value
from pyki.module_util import is_package_installed, is_module_imported
from pyki.utils import run_once
from abc import ABC
from pyki.wrapt import wrap_function_wrapper # type: ignore
import threading
import gc
import platform
import time
from typing import List, Optional


class BaseMetricsSampler(ABC):
    def start_sampling(self):
        pass

    def stop_sampling(self):
        pass


_periodic_tasks = []
_periodic_tasks_lock = threading.Lock()


def _register_periodic_task(task):
    if len(_periodic_tasks) == 0:
        def periodic_task_wrapper():
            while True:
                time.sleep(1)
                with _periodic_tasks_lock:
                    tasks = list(_periodic_tasks)
                for periodic_task in tasks:
                    periodic_task()

        threading.Thread(target=periodic_task_wrapper, daemon=True, name="pyki PeriodicTask").start()
    with _periodic_tasks_lock:
        _periodic_tasks.append(task)


def _remove_periodic_task(task):
    with _periodic_tasks_lock:
        _periodic_tasks.remove(task)


class CPythonEnvironmentSampler(BaseMetricsSampler):

    def start_sampling(self):
        if platform.python_implementation() != "CPython":
            return
        register_metric(str_type, "cpython.platform.version", initial_value=platform.python_version())


class CPythonGCMetricsSampler(BaseMetricsSampler):

    def __init__(self):
        self._gc_callback = None
        self._gc_callback_registered = False
        self._gc_count = [0, 0, 0]
        self._gc_count_metric: Optional[Metric] = None
        self._gc_time = [0.0, 0.0, 0.0]
        self._gc_time_metric: Optional[Metric] = None
        self._gc_start_time = -1

    def start_sampling(self):
        self._create_gc_metrics()

    def _create_gc_metrics(self):
        if platform.python_implementation() != "CPython" or not hasattr(gc, "callbacks"):
            return
        self._gc_time_metric = register_metric(float_type, "cpython.gc.time", length=3)
        self._gc_count_metric = register_metric(int_type, "cpython.gc.occurrence", length=3)

        def _callback(phase, info):
            if phase == "start":
                self._gc_start_time = time.time_ns()
                pass
            elif self._gc_start_time > 0:
                duration_in_s = (time.time_ns() - self._gc_start_time) / (10 ** 9)
                generation = info["generation"]
                self._gc_time[generation] += duration_in_s
                self._gc_count[generation] += 1
                self._gc_start_time = -1
                # we don't immediately update the metrics to prevent race conditions

        self._gc_callback = _callback
        gc.callbacks.append(self._gc_callback)
        self._gc_callback_registered = True

        def flush_result():
            assert self._gc_time_metric is not None
            assert self._gc_count_metric is not None
            self._gc_time_metric.update_value(self._gc_time)
            self._gc_count_metric.update_value(self._gc_count)

        _register_periodic_task(flush_result)


class PytorchMetricsSampler(BaseMetricsSampler):
    def __init__(self):
        self.periodic_task = None
        self.metrics = {}

    def start_sampling(self):
        def sample_metrics():
            if not is_package_installed("torch"):
                _remove_periodic_task(self.periodic_task)
                return
            if not is_module_imported("torch.cuda"):
                return
            from torch.cuda import memory_stats

            stats = memory_stats()
            for key, value in stats.items():
                if key not in self.metrics:
                    metric = register_metric(get_datatype_from_value(value), f"torch.cuda.memory.{key}")
                    self.metrics[key] = metric
                else:
                    metric = self.metrics[key]
                metric.update_value(value)
        self.periodic_task = sample_metrics
        _register_periodic_task(self.periodic_task)


class VllmMetricsSampler(BaseMetricsSampler):
    def __init__(self):
        self.logger_injected = None
        self.periodic_task = None
        self.metrics = {}

    def create_metrics(self):
        self.metrics = {
            "prompt_throughput": register_metric(float_type, "vllm.prompt_throughput"),
            "generation_throughput": register_metric(float_type, "vllm.generation_throughput"),
            "num_prompt_tokens": register_metric(int_type, "vllm.num_prompt_tokens"),
            "num_running_sys": register_metric(int_type, "vllm.num_running_sys"),
            "num_swapped_sys": register_metric(int_type, "vllm.num_swapped_sys"),
            "num_waiting_sys": register_metric(int_type, "vllm.num_waiting_sys"),
            "gpu_cache_usage_sys": register_metric(float_type, "vllm.gpu_cache_usage_sys"),
            "cpu_cache_usage_sys": register_metric(float_type, "vllm.cpu_cache_usage_sys"),
            "cpu_prefix_cache_hit_rate": register_metric(float_type,
                                                         "vllm.cpu_prefix_cache_hit_rate"),
            "gpu_prefix_cache_hit_rate": register_metric(float_type,
                                                         "vllm.gpu_prefix_cache_hit_rate"),
        }
    def start_sampling(self):
        sampler = self
        def wrap_do_log_stats(call_wrapped, instance, args, kwargs):
            try:
                if not self.logger_injected:
                    self.logger_injected = True
                    if instance.log_stats:
                        from vllm.engine.metrics_types import StatLoggerBase
                        from vllm.engine.metrics import local_interval_elapsed, get_throughput

                        class PykiVllmStatLogger(StatLoggerBase):

                            def log(self, stats):
                                self.num_prompt_tokens.append(stats.num_prompt_tokens_iter)
                                self.num_generation_tokens.append(stats.num_generation_tokens_iter)
                                self.maybe_update_spec_decode_metrics(stats)
                                if local_interval_elapsed(stats.now, self.last_local_log,
                                                          self.local_interval):
                                    prompt_throughput = get_throughput(self.num_prompt_tokens,
                                                                       now=stats.now,
                                                                       last_log=self.last_local_log)
                                    generation_throughput = get_throughput(
                                        self.num_generation_tokens,
                                        now=stats.now,
                                        last_log=self.last_local_log)
                                    sampler.metrics["prompt_throughput"].update_value(prompt_throughput)
                                    sampler.metrics["generation_throughput"].update_value(generation_throughput)
                                    sampler.metrics["num_running_sys"].update_value(stats.num_running_sys)
                                    sampler.metrics["num_swapped_sys"].update_value(stats.num_swapped_sys)
                                    sampler.metrics["num_waiting_sys"].update_value(stats.num_waiting_sys)
                                    sampler.metrics["gpu_cache_usage_sys"].update_value(stats.gpu_cache_usage_sys)
                                    sampler.metrics["cpu_cache_usage_sys"].update_value(stats.cpu_cache_usage_sys)
                                    sampler.metrics["cpu_prefix_cache_hit_rate"].update_value(
                                        stats.cpu_prefix_cache_hit_rate)
                                    sampler.metrics["gpu_prefix_cache_hit_rate"].update_value(
                                        stats.gpu_prefix_cache_hit_rate)

                                    # Reset tracked stats for next interval.
                                    self.num_prompt_tokens: List[int] = []
                                    self.num_generation_tokens: List[int] = []
                                    self.last_local_log: float = stats.now
                                    self.spec_decode_metrics = None

                            def info(self, type, obj):
                                raise NotImplementedError

                            @staticmethod
                            def create_instance():
                                # for compatability
                                try:
                                    return PykiVllmStatLogger(5) # type: ignore
                                except Exception as e:
                                    pass
                                try:
                                    return PykiVllmStatLogger(5, None) # type: ignore
                                except Exception as e:
                                    pass
                                return None

                        if instance.stat_loggers is None:
                            instance.stat_loggers = {}
                        stat_logger = PykiVllmStatLogger.create_instance()
                        if stat_logger is not None:
                            instance.stat_loggers["pyki"] = stat_logger
                        if len(sampler.metrics) == 0:
                            sampler.create_metrics()
            except Exception as e:
                print(e)
            call_wrapped(*args, **kwargs)

        def check_vllm_import():
            if not is_package_installed("vllm"):
                _remove_periodic_task(self.periodic_task)
                return
            if not is_module_imported("vllm.engine.llm_engine"):
                return
            wrap_function_wrapper(
                "vllm.engine.llm_engine",
                "LLMEngine.do_log_stats",
                wrap_do_log_stats,
            )
            _remove_periodic_task(self.periodic_task)

        self.periodic_task = check_vllm_import
        _register_periodic_task(self.periodic_task)


def register_sampler(sampler: BaseMetricsSampler):
    sampler.start_sampling()


@run_once
def register_samplers():
    if not perfdata_writer.enabled:
        return
    register_sampler(CPythonEnvironmentSampler())
    register_sampler(CPythonGCMetricsSampler())
    register_sampler(PytorchMetricsSampler())
    # 做的不够健壮，先注释掉了
    # register_sampler(VllmMetricsSampler())
