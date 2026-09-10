import gzip
import json
from pyki.error import raise_error
from typing import Any, Dict


class TraceEventType:
    # Duration trace events
    BEGIN = "B"
    END = "E"
    # Complete trace event
    COMPLETE = "X"
    # Instant trace event
    INSTANT = "I"
    # Counter trace event
    COUNTER = "C"
    # Async trace events
    NESTABLE_ASYNC_BEGIN = "b"
    NESTABLE_ASYNC_END = "e"
    NESTABLE_ASYNC_INSTANT = "n"
    # Flow trace events
    FLOW_BEGIN = "s"
    FLOW_STEP = "t"
    FLOW_END = "f"
    # Metadata trace events
    METADATA = "M"
    # Sample trace event
    SAMPLE = "P"
    # Object trace events
    CREATE_OBJECT = "N"
    SNAPSHOT_OBJECT = "O"
    DELETE_OBJECT = "D"
    # Memory dump trace events
    MEMORY_DUMP_GLOBAL = "V"
    MEMORY_DUMP = "v"
    # Mark trace event
    MARK = "R"
    # Clock sync event
    CLOCK_SYNC = "c"


class ChromeTraceEvent:
    """
    This class partially implements Trace Event Format that can be analyzed on chrome://tracing.
    See https://docs.google.com/document/d/1CvAClvFfyA5R-PhYUmn5OOQtYMH4h6I0nSsKchNAySU for more detail.
    """

    def __init__(self):
        # For ease of processing, all time units are in seconds, except for the data in self.trace
        # which will be filled in according to the correct format.
        self.trace: Dict[str, Any] = {
            "traceEvents": []
        }
        self.baseTime = None

    def set_base_time(self, base_time):
        self.baseTime = base_time
        self.trace["baseTimeNanoseconds"] = int(base_time * 10 ** 9)

    @staticmethod
    def read_from_file(path) -> "ChromeTraceEvent":
        trace_event = ChromeTraceEvent()
        if path.endswith(".gz"):
            with gzip.open(path, 'rt', encoding='utf-8') as f:
                trace_event.trace = json.load(f)
        else:
            with open(path, "r") as f:
                trace_event.trace = json.load(f)
        if "traceEvents" not in trace_event.trace:
            raise_error("traceEvents is not found in trace")
        # torch profile did not have baseTimeNanoseconds unitl https://github.com/pytorch/kineto/pull/910
        if "baseTimeNanoseconds" not in trace_event.trace:
            trace_event.baseTime = 0
        else:
            trace_event.baseTime = trace_event.trace["baseTimeNanoseconds"] / 10 ** 9
        return trace_event

    def write_to_file(self, path):
        if "traceName" in self.trace:
            self.trace["traceName"] = path
        if "displayTimeUnit" not in self.trace:
            self.trace["displayTimeUnit"] = "ms"
        self._order_trace_keys()
        if path.endswith(".gz"):
            with gzip.open(path, 'wt', encoding='utf-8') as f:
                # indent may increase file size by 60%, so let's not indent
                json.dump(self.trace, f, ensure_ascii=False)
        else:
            with open(path, 'w') as f:
                json.dump(self.trace, f, ensure_ascii=False, indent=None)

    @staticmethod
    def merge_traces(traces) -> "ChromeTraceEvent":
        return _ChromeTraceEventMerger().merge_traces(traces)

    def _order_trace_keys(self):
        # order trace keys to make trace more similar to torch profile result
        # after python3.8 dict keys are ordered
        order = [
            # other keys
            "traceEvents",
            "traceName",
            "displayTimeUnit",
            "baseTimeNanoseconds"
        ]
        keys = list(self.trace.keys())
        ordered_keys = [k for k in keys if k not in order] + [k for k in order if k in keys]
        self.trace = {k: self.trace[k] for k in ordered_keys}

    def add_complete_event(self, category, name, pid, tid, ts, duration, args=None):
        # reference：
        #
        # {
        #     "ph": "X", "cat": "python_function", "name": "torch/utils/data/dataset.py(207): <genexpr>",
        #     "pid": 557882, "tid": 557882,
        #     "ts": 15394687259.744, "dur": 0.073,
        #     "args": {
        #         "Python parent id": 6505, "Python id": 6508, "finished": true, "Ev Idx": 28588
        #     }
        # }
        event = {
            "ph": TraceEventType.COMPLETE,
            "cat": category,
            "name": name,
            "pid": pid,
            "tid": tid,
            "ts": self._convert_ts(ts),
            "dur": duration * 10 ** 6,  # unit in trace is us
        }
        if args is not None:
            event["args"] = args
        self._append_event(event)

    def add_process_info(self, process_name, pid, ts, sort_index=None): # pragma: no cover
        # reference:
        #
        # {
        #     "name": "process_name", "ph": "M", "ts": 15394671952.116, "pid": 557882, "tid": 0,
        #     "args": {
        #         "name": "python3"
        #     }
        # },
        # {
        #     "name": "process_labels", "ph": "M", "ts": 15394671952.116, "pid": 557882, "tid": 0,
        #     "args": {
        #         "labels": "CPU"
        #     }
        # },
        # {
        #     "name": "process_sort_index", "ph": "M", "ts": 15394671952.116, "pid": 557882, "tid": 0,
        #     "args": {
        #         "sort_index": 557882
        #     }
        # },
        ts = self._convert_ts(ts)
        if sort_index is None:
            sort_index = pid
        self._append_event({
            "name": "process_name",
            "ph": TraceEventType.METADATA,
            "ts": ts,
            "pid": pid,
            "tid": 0,
            "args": {
                "name": process_name
            }
        })
        self._append_event({
            "name": "process_labels",
            "ph": TraceEventType.METADATA,
            "ts": ts,
            "pid": pid,
            "tid": 0,
            "args": {
                "labels": "CPU"
            }
        })
        self._append_event({
            "name": "process_sort_index",
            "ph": TraceEventType.METADATA,
            "ts": ts,
            "pid": pid,
            "tid": 0,
            "args": {
                "sort_index": sort_index
            }
        })

    def add_thread_info(self, thread_name, pid, tid, ts, sort_index=None):
        # reference:
        #
        # {
        #     "name": "thread_name", "ph": "M", "ts": 15394671952.116, "pid": 0, "tid": 7,
        #     "args": {
        #         "name": "stream 7 "
        #     }
        # },
        # {
        #     "name": "thread_sort_index", "ph": "M", "ts": 15394671952.116, "pid": 0, "tid": 7,
        #     "args": {
        #         "sort_index": 7
        #     }
        # },
        ts = self._convert_ts(ts)
        if sort_index is None:
            sort_index = tid
        self._append_event({
            "name": "thread_name",
            "ph": TraceEventType.METADATA,
            "ts": ts,
            "pid": pid,
            "tid": tid,
            "args": {
                "name": thread_name
            }
        })
        self._append_event({
            "name": "thread_sort_index",
            "ph": TraceEventType.METADATA,
            "ts": ts,
            "pid": pid,
            "tid": tid,
            "args": {
                "sort_index": sort_index
            }
        })

    def _append_event(self, event):
        self.trace["traceEvents"].append(event)

    def add_metadata(self, key, value):
        self.trace[key] = value

    def _convert_ts(self, time):
        assert self.baseTime is not None, "baseTime is not set"
        return (time - self.baseTime) * 10 ** 6  # unit of timestamp is us


_thread_event_names = ["thread_name", "thread_sort_index"]
_process_event_names = ["process_name", "process_labels", "process_sort_index"]


class _ChromeTraceEventMerger:
    def __init__(self):
        self._merged = None

    def merge_traces(self, traces) -> ChromeTraceEvent:
        # The performance of this merge implementation is not good enough. Consider
        # using c++ in the future.
        # This implementation is destructive for better performance. original traces
        # are no longer usable after merging.
        # This is not a universal implementation. It can only merge
        # profiles from pyki, so there are many hardcode.

        # assert traces[0].trace["pyki_trace_type"] == "torch_profile"
        if len(traces) == 1:
            return traces[0]
        self._merged = traces[0]
        self._merge_traces_metadata(traces)
        self._merge_traces_trace_events(traces)
        return self._merged

    def _merge_traces_metadata(self, traces):
        assert self._merged is not None
        merged_trace = self._merged.trace
        for trace in traces[1:]:
            for k, v in trace.trace.items():
                if k != "traceEvents" and k not in merged_trace:
                    merged_trace[k] = v
        self._merged.baseTime = traces[0].baseTime

    def _merge_traces_trace_events(self, traces):
        assert self._merged is not None
        merged_trace_events = self._merged.trace["traceEvents"]
        recorded_threads = set()
        recorded_processes = set()
        for event in merged_trace_events:
            if event["ph"] != TraceEventType.METADATA:
                continue
            if "pid" not in event or "tid" not in event or "name" not in event:
                continue
            pid, tid, name = event["pid"], event["tid"], event["name"]
            if name in _thread_event_names:
                recorded_threads.add((pid, tid, name))
            elif name in _process_event_names:
                recorded_processes.add((pid, name))

        # we need to deduplicate adjusted tid, otherwise perfetto can not
        # display thread name
        for trace in traces[1:]:
            base_diff = (trace.baseTime - self._merged.baseTime) * 10 ** 6
            for event in trace.trace["traceEvents"]:
                if "pid" not in event or "tid" not in event or "name" not in event:
                    continue
                pid, tid, name = event["pid"], event["tid"], event["name"]
                should_append_event = True
                if event["ph"] == TraceEventType.METADATA:
                    if name in _thread_event_names:
                        key: Any = (pid, tid, name)
                        if key not in recorded_threads:
                            recorded_threads.add(key)
                        else:
                            should_append_event = False
                    elif name in _process_event_names:
                        key = (pid, name)
                        if key not in recorded_processes:
                            recorded_processes.add(key)
                        else:
                            should_append_event = False
                if should_append_event:
                    if "ts" in event:
                        event["ts"] += base_diff
                    merged_trace_events.append(event)
