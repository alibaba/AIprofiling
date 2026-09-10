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
#include "containers.h"
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

class ThreadLocalSubqueue {
 public:
  ThreadLocalSubqueue(const uint64_t tid, ProfilerConfig config);

  template <class... Args>
  void emplace_py_call(Args&&... args) {
    py_calls_.emplace_back(std::forward<Args>(args)...);
  }

  uint64_t tid() const {
    return tid_;
  }

  uint64_t os_tid() const {
    return os_tid_;
  }

  auto& thread_name() const {
    return thread_name_;
  }

  auto& py_calls() {
    return py_calls_;
  }

 private:
  uint64_t tid_;
  uint64_t os_tid_;
  std::string thread_name_;
  ProfilerConfig config_;
  

  friend class RecordQueue;
  // See `containers.h` for block size benchmarks.
  static constexpr size_t BlockSize = 512;



  AppendOnlyList<
      std::pair<python_tracer::TraceKey, StartEndTime>,
      BlockSize>
      py_calls_;
};

class RecordQueue {
 public:
  RecordQueue(ProfilerConfig config);

  ThreadLocalSubqueue* getSubqueue();

  // python public api
  // profile starts when this object is created, data cleared when this object
  // is destroyed
  void stop();
  void export_data(const char* path);
  void suspendPythonTracerForCurrentThread();
  void resumePythonTracerForCurrentThread();
  /////////////


  // // NB: This is a destructive operation.
  // std::pair<
  //     std::vector<std::shared_ptr<Result>>,
  //     std::unique_ptr<torch::profiler::impl::kineto::ActivityTraceWrapper>>
  // getRecords(uint64_t start_time_ns, uint64_t end_time_ns);

  const ProfilerConfig& config() const {
    return config_;
  }

  long double scaleFactor() const {
    return scale_factor_;
  }

  auto& converter_func() const {
    return converter_func_;
  }

  auto& sub_queues() const {
    return sub_queues_;
  }

 private:
  uint32_t id_;
  ProfilerConfig config_;
  ska::flat_hash_map<uint64_t, std::unique_ptr<ThreadLocalSubqueue>>
      sub_queues_;
  std::mutex sub_queue_mutex_;
  std::unique_ptr<python_tracer::PythonTracerBase> python_tracer_;

  c10::ApproximateClockToUnixTimeConverter clock_converter_;
  std::function<c10::time_t(c10::approx_time_t)> converter_func_;
  long double scale_factor_;
};

} // namespace torch::profiler::impl
