#include "collection.h"
#include "util.h"

#include <atomic>
#include <algorithm>
#include <functional>
#include <limits>
#include <memory>
#include <queue>
#include <type_traits>
#include <utility>

namespace pyki::torch::profiler::impl {

RecordQueue::RecordQueue(ProfilerConfig config)
    : config_{std::move(config)},
      converter_func_{clock_converter_.makeConverter(&scale_factor_)} {
    python_tracer_ = python_tracer::PythonTracerBase::make(this);
}

void RecordQueue::stop() {
  if (python_tracer_) {
    python_tracer_->stop();
  }
}

void RecordQueue::export_data(const char* path) {
  if (python_tracer_) {
    python_tracer_->export_data(path);
  }
}

void RecordQueue::suspendPythonTracerForCurrentThread() {
  if (python_tracer_) {
    python_tracer_->suspend();
  }
}

void RecordQueue::resumePythonTracerForCurrentThread() {
  if (python_tracer_) {
    python_tracer_->resume();
  }
}

} // namespace torch::profiler::impl
