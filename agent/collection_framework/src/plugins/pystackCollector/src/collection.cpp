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

#include <unistd.h>
#include <sys/syscall.h>

namespace pyki::torch::profiler::impl {

// See `RecordQueue::getSubqueue()` for an overview of this cache.
struct SubQueueThreadCache {
  uint32_t key_;
  ThreadLocalSubqueue* ref_;
};

std::atomic<uint32_t> queue_id_{0};
thread_local SubQueueThreadCache sub_queue_cache_{0, nullptr};

ThreadLocalSubqueue::ThreadLocalSubqueue(
    const uint64_t tid,
    ProfilerConfig config)
    : tid_{tid},
      config_{std::move(config)},
      os_tid_{syscall(__NR_gettid)},
      thread_name_{pyki::get_current_python_thread_name()}
      {
}

RecordQueue::RecordQueue(ProfilerConfig config)
    : id_(++queue_id_),
      config_{std::move(config)},
      converter_func_{clock_converter_.makeConverter(&scale_factor_)} {
    python_tracer_ = python_tracer::PythonTracerBase::make(this);
}

ThreadLocalSubqueue* RecordQueue::getSubqueue() {
  // In the most common case, a thread will want to write to the same sub-queue
  // that it wrote to last call. The only time that isn't true is if:
  //  A) The profiler context has ended and we are in a new one.
  //  B) Two profilers are active in different TLS contexts, and this thread
  //     is a worker helping with intra-op parallelism.
  // Since we expect this to be the OVERWHELMINGLY common case (>99%), we add a
  // special thread_local cache so that we can skip the overall `flat_hash_map`
  // (and corresponding lock).
  if (id_ == sub_queue_cache_.key_) {
    return sub_queue_cache_.ref_;
  }

  const auto tid = at::currentThreadId();
  std::lock_guard<std::mutex> guard(sub_queue_mutex_);
  auto it = sub_queues_.find(tid);
  if (it == sub_queues_.end()) {
    it = sub_queues_
             .emplace(tid, std::make_unique<ThreadLocalSubqueue>(tid, config_))
             .first;
  }

  sub_queue_cache_ = SubQueueThreadCache{id_, it->second.get()};
  return it->second.get();
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
