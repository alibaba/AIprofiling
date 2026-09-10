from pyki.module_util import guarantee_packages_installed, is_module_imported, reimport_module, is_module_exist
from pyki.error import raise_error
from pyki.profiling.base_profiler import BaseProfiler
from pyki.wrapt import wrap_function_wrapper # type: ignore
import logging
import importlib
import json
import base64
import traceback

_logger = logging.getLogger(__name__)

_VLLM_TRACING_MODULE_NAME = "vllm.tracing"
_VLLM_LLM_ENGINE_MODULE_NAME = "vllm.engine.llm_engine"

_OT_PACKAGES_NEEDED = [
    'opentelemetry-sdk==1.28.2',
    'opentelemetry-api==1.28.2',
    'opentelemetry-exporter-otlp==1.28.2',
    'opentelemetry-semantic-conventions-ai==0.4.2',
    'protobuf==5.28.3'
]


class VllmOTTraceConfig:
    def __init__(self,
                 path: str = "vllm_trace.json"):
        """
        - path (str): The path to save the trace file. Defaults to "vllm_trace.json".
        """
        assert path is not None
        self.path = path


class VllmOTTraceProfiler(BaseProfiler):
    def __init__(self):
        super().__init__("VLLM OpenTelemetry Trace")
        self._exporter = None
        self._tracing_module_injected = False
        self._request_ot_export = False

    def _do_inject(self, vllm_tracing_module):
        # assert ot is already installed
        from opentelemetry.trace import get_tracer_provider
        from opentelemetry.sdk.trace.export import SimpleSpanProcessor, SpanExportResult
        from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter
        from opentelemetry.exporter.otlp.proto.common.trace_encoder import (
            encode_spans,
        )
        from google.protobuf.json_format import MessageToJson

        assert not self._tracing_module_injected

        class FileExporter(InMemorySpanExporter):
            def export(self, spans):
                with self._lock:
                    if not self._stopped:
                        self._finished_spans.extend(spans)
                return SpanExportResult.SUCCESS

            def start(self):
                with self._lock:
                    self._stopped = False

            def shutdown(self):
                with self._lock:
                    self._stopped = True

            def _fix_trace_data(self, spans):
                def _fix_id(original, length):
                    try:
                        original_bytes = base64.b64decode(original + "=" * ((4 - len(original) % 4) % 4))
                        integer = int.from_bytes(original_bytes, byteorder="big", signed=False)
                        return format(integer, 'X').zfill(length)
                    except Exception as e:
                        _logger.warning(f"failed to fix id: {original} with error {e}")
                        return "0" * length

                def _fix_trace_data_recursive(obj):
                    if isinstance(obj, list):
                        for item in obj:
                            _fix_trace_data_recursive(item)
                    elif isinstance(obj, dict):
                        for key, value in obj.items():
                            if isinstance(value, dict) or isinstance(value, list):
                                _fix_trace_data_recursive(value)
                            elif isinstance(value, str):
                                if "traceId" == key:
                                    obj[key] = _fix_id(value, 32)
                                elif "spanId" == key:
                                    obj[key] = _fix_id(value, 16)
                                elif "parentSpanId" == key:
                                    obj["parentSpanId"] = _fix_id(obj["parentSpanId"], 16)

                encoded_str = MessageToJson(encode_spans(spans))
                encoded_json = json.loads(encoded_str)
                _fix_trace_data_recursive(encoded_json)
                encoded_str = json.dumps(encoded_json)
                return encoded_str

            def export_to_file(self, path):
                with self._lock:
                    spans = self._finished_spans
                    encoded = self._fix_trace_data(spans)
                    with open(path, "w") as f:
                        f.write(encoded)

        def wrap_get_span_exporter(call_wrapped, instance, args, kwargs):
            self._exporter = FileExporter()
            return self._exporter

        wrap_function_wrapper(
            _VLLM_TRACING_MODULE_NAME,
            "get_span_exporter",
            wrap_get_span_exporter,
        )

        self._tracing_module_injected = True

        def wrap_do_tracing(call_wrapped, instance, args, kwargs):
            engine = instance
            try:
                if engine.tracer is None:
                    _logger.info("injecting tracer")
                    engine.tracer = vllm_tracing_module.init_tracer("vllm.llm_engine", "ignore")
                if self._exporter is None:
                    _logger.info("injecting span exporter")
                    span_exporter = vllm_tracing_module.get_span_exporter()
                    get_tracer_provider().add_span_processor(SimpleSpanProcessor(span_exporter))
                if self._request_ot_export:
                    assert self._exporter is not None
                    self._exporter.clear()
                    self._exporter.start()
                    self._request_ot_export = False
            except Exception as e:
                traceback.print_exc()
            return call_wrapped(*args, **kwargs)

        wrap_function_wrapper(
            _VLLM_LLM_ENGINE_MODULE_NAME,
            "LLMEngine.do_tracing",
            wrap_do_tracing,
        )

    def _start_profile(self, config):
        # current implementation does not consider concurrency
        # assert vllm is already installed
        if not self._tracing_module_injected:
            if not is_module_exist(_VLLM_TRACING_MODULE_NAME):
                raise_error("current vllm version does not support opentelemetry")
            if not is_module_imported(_VLLM_TRACING_MODULE_NAME):
                raise_error("vllm is not serving")

            guarantee_packages_installed(*_OT_PACKAGES_NEEDED)

            vllm_tracing_module = importlib.import_module(_VLLM_TRACING_MODULE_NAME)
            ot_was_installed = vllm_tracing_module.is_otel_available()
            if not ot_was_installed:
                reimport_module(_VLLM_TRACING_MODULE_NAME)
                vllm_tracing_module = importlib.import_module(_VLLM_TRACING_MODULE_NAME)
                vllm_engine_module = importlib.import_module(_VLLM_LLM_ENGINE_MODULE_NAME)
                setattr(vllm_engine_module, "SpanAttributes", vllm_tracing_module.SpanAttributes)
                setattr(vllm_engine_module, "SpanKind", vllm_tracing_module.SpanKind)
                assert vllm_tracing_module.is_otel_available()
            self._do_inject(vllm_tracing_module)

        self._request_ot_export = True

    def _stop_profile(self):
        if self._exporter is None:
            raise_error("no traces collected")
        assert self.profile_config is not None
        self._exporter.export_to_file(self.profile_config.path)
        self._exporter.clear()
        self._exporter.shutdown()
        self._request_ot_export = False


vllm_ot_trace_profiler_instance = VllmOTTraceProfiler()
