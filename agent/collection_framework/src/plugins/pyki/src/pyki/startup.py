import os

_started = False

def on_startup():
    global _started
    if not _started:
        try:
            if os.environ.get("PYKI_STARTUP_HOOK", "0") == "0":
                return
            from pyki.profiling.nvtx_tracer import hook_nvtx_annotate_decorator
            hook_nvtx_annotate_decorator()

        finally:
            _started = True
