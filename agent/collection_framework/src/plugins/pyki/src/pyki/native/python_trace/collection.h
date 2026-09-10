#pragma once

#include <cstdint>
#include <memory>
#include <mutex>
#include <type_traits>
#include <utility>
#include <variant>

#include <Python.h>
#include "strong_type.h"
#include "flat_hash_map.h"
#include "util.h"
#include "profiler_python.h"
#include "ApproximateClock.h"

namespace pyki::torch::profiler::impl {

namespace python_tracer {

class PythonTracerBase;

using TraceKey = strong::type<
    uint64_t,
    struct TraceKey_,
    strong::regular,
    strong::hashable,
    strong::ostreamable>;

struct CompressedEvent {
  TraceKey key_;
  uint64_t system_tid_{};
  c10::time_t enter_t_{};
  c10::time_t exit_t_{};
};

} // namespace python_tracer

struct ProfilerConfig {
  int python_tracer_max_depth;
  int python_tracer_threshold_ns;
  bool python_tracer_ignore_c_functions;
};


enum class EventType : uint8_t {
  PyCall,
  PyCCall,
};

template <typename T, typename Tag>
using strong_t = strong::
    type<T, Tag, strong::regular, strong::convertible_to<T>, strong::hashable>;

using PyMethod = strong_t</*PyMethodDef*/ void*, struct PyMethod_>;

struct StartEndTime {
  c10::approx_time_t start_time_;
  c10::approx_time_t end_time_;
};

class RecordQueue {
 public:
  RecordQueue(ProfilerConfig config);

  // python public api
  // profile starts when this object is created, data cleared when this object
  // is destroyed
  void stop();
  void export_data(const char* path);
  void suspendPythonTracerForCurrentThread();
  void resumePythonTracerForCurrentThread();
  /////////////

  const ProfilerConfig& config() const {
    return config_;
  }

  long double scaleFactor() const {
    return scale_factor_;
  }

  auto& converter_func() const {
    return converter_func_;
  }


 private:
  ProfilerConfig config_;
  std::unique_ptr<python_tracer::PythonTracerBase> python_tracer_;

  c10::ApproximateClockToUnixTimeConverter clock_converter_;
  std::function<c10::time_t(c10::approx_time_t)> converter_func_;
  long double scale_factor_;
};

} // namespace torch::profiler::impl
