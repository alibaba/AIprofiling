# AIProf test suite

Four layers, none of which needs a GPU:

```
test/
  native/               # C++11 — regression test for the vendored cuprof patches
    test_cuprof_trace_writer.cc
    Makefile
  workloads/            # CPU-only stand-ins for AI training / inference
    training_workload.py
    inference_workload.py
  integration/          # pytest — end-to-end against a live stack
    conftest.py
    test_e2e_upload_query_report.py
    test_workload_profile.py
  unit/                 # pytest — pure-function coverage
    test_folded_parser.py
    test_storage_extract.py
    test_report_top_stacks.py
  data/                 # golden fixtures used by unit tests
    sample.folded
```

## Run everything

```bash
# from the repo root
python3.11 -m pip install pytest numpy       # one-off
python3.11 -m pytest test/ -v
make test-native                             # C++11 only, no Python needed
```

`test/native` covers the timestamp conversion in AIProf's local cuprof patch
(`patches/0002-align-cupti-epoch-to-clock-monotonic.patch`). `trace_writer.cc`
has no CUDA or CUPTI dependency, so it compiles anywhere; the rest of cuprof
needs a GPU. Note `test/unit/test_folded_parser.py` and
`test/unit/test_gpu_collector.py` currently fail at import: they load
`agent-py/aiprof_agent.py` and `agent-py/aiprof_gpu.py`, and no `agent-py/`
directory exists in this repository.

Integration tests need the AIProf stack running locally (collector 7101,
query 7102, report 7103, BFF 8080). They will `pytest.skip` if the
services are unreachable, so `pytest test/unit` always works standalone.

## CPU-only "GPU workloads"

`workloads/training_workload.py` and `inference_workload.py` are pure NumPy
processes that behave like a mini training loop and a batched inference
server respectively. On a machine without a GPU they let AIProf capture a
non-trivial call graph via `perf record`, so the folded flamegraph and the
AI report have real content to work with.
