#include "profiler_python.h"

#include <atomic>
#include <cstdint>
#include <deque>
#include <limits>
#include <memory>
#include <queue>
#include <string>
#include <utility>
#include <vector>
#include <optional>

#include <Python.h>
#include <frameobject.h>

#include "Macros.h"
#include "util.h"
#include "flat_hash_map.h"
#include "irange.h"
#include "object_ptr.h"
#include "hash.h"
#include <pybind11/pybind11.h>
#include "collection.h"
#include "pythoncapi_compat.h"
#include "chrome_trace.hpp"
#include "../shadow.hpp"

#define DEBUG_LOG 0

namespace py = pybind11;

namespace pyki::torch::profiler::impl {


namespace {

enum CallType { PyCall = 0, PyCCall };
static constexpr size_t CallTypeSize = 2;
using no_ephemeral_t = std::tuple<>;
static constexpr uint64_t NoTID = std::numeric_limits<uint64_t>::max();

// ============================================================================
// == Miscellaneous structs and utils =========================================
// ============================================================================
struct CodeLocation {
  CodeLocation() = default;
  explicit CodeLocation(PyFrameObject* frame)
      : line_number_{PyFrame_GetLineNumber(frame)}, code_{PyFrame_GetCode(frame)} {
    // we don't hold code object, we just use its address as key, so we don't hold its rc
    Py_XDECREF(code_);
  }

  bool operator==(const CodeLocation& other) const {
    return code_ == other.code_ && line_number_ == other.line_number_;
  }

  int line_number_{0};
  PyCodeObject* code_{nullptr};
};


} // namespace
} // namespace torch::profiler::impl

template <>
struct std::hash<pyki::torch::profiler::impl::CodeLocation> {
  size_t operator()(const pyki::torch::profiler::impl::CodeLocation& x) {
    return pyki::c10::get_hash(x.code_, x.line_number_);
  }
};

namespace pyki::torch::profiler::impl {
// ============================================================================
// == CallTypeHelper: Tools for generic programming on specializations. =======
// ============================================================================
template <template <CallType> class ClassT>
class CallTypeHelper final {
 private:
  static_assert(
      CallType::PyCall == 0,
      "CallTypeHelper uses integer math which depends on a zero start.");
  static constexpr size_t End = CallTypeSize;

  template <size_t... I>
  static constexpr std::tuple<ClassT<(CallType)I>...> make_tuple_impl(
      std::index_sequence<I...>);

  template <size_t C, typename T, typename FunctorT, typename... Args>
  static void map(T& t, FunctorT& f, Args&&... args) {
    f(std::get<C>(t), args...);
    if constexpr (C + 1 < End) {
      map<C + 1>(t, f, std::forward<Args>(args)...);
    }
  }

 public:
  using tuple_type = decltype(make_tuple_impl(std::make_index_sequence<End>{}));

  template <typename FunctorT, typename... Args>
  static void map(tuple_type& t, FunctorT& f, Args&&... args) {
    map<0>(t, f, std::forward<Args>(args)...);
  }
};


template <CallType>
struct Config;

template <>
struct Config<CallType::PyCall> {
  using key_t = CodeLocation;
  using ephemeral_t = no_ephemeral_t;
  using cache_t = ska::flat_hash_map<key_t, std::string>;
  static constexpr EventType event_type = EventType::PyCall;
};

template <>
struct Config<CallType::PyCCall> {
  using key_t = PyMethod;
  using ephemeral_t = PyObject*;
  using cache_t = ska::flat_hash_map<key_t, std::string>;
  static constexpr EventType event_type = EventType::PyCCall;
};

// ============================================================================
// == Callsite & ValueCache: Storage during profiling =========================
// ============================================================================

template <CallType C>
class Callsite {
 public:
  static constexpr CallType call_type = C;
  using key_t = typename Config<C>::key_t;

  static_assert(
      std::is_trivially_copyable_v<key_t>,
      "Key should be trivial, as it is passed by value.");

  template <typename U>
  Callsite(U value) : value_(value) {}

  bool operator==(const Callsite& other) const {
    return value_ == other.value_;
  }

  key_t value_;
};

// ============================================================================
// == Type specific store and load implementations. ===========================
// ============================================================================
using PyCallKey = Config<CallType::PyCall>::key_t;
using PyCCallKey = Config<CallType::PyCCall>::key_t;

using FrameOrCFunction = std::variant<PyFrameObject*, PyCFunctionObject*, std::nullptr_t>;

class StackEntry {
 public:
  c10::approx_time_t start_;
  // In fact, We don't need to save the frame since the return event will
  // pass it again. Keep it now since it can simplify the process of
  // unfinished frames.
  FrameOrCFunction value_;
};

class ValueCache {
 public:
  ValueCache() = default;
  ValueCache(const ValueCache&) = delete;
  ValueCache& operator==(const ValueCache&) = delete;
  ValueCache(ValueCache&&) = default;
  ValueCache& operator==(ValueCache&&) = delete;
  ~ValueCache() = default;

  template <CallType C>
  void store(const typename Config<C>::key_t&, typename Config<C>::ephemeral_t);

  auto& state() const {
    return state_;
  }

 private:
  template <CallType C>
  using State = typename Config<C>::cache_t;

  CallTypeHelper<State>::tuple_type state_;
};

template <CallType C>
typename Config<C>::cls_t set_class(
    ValueCache* value_cache,
    typename Config<C>::cache_t& cache,
    const typename Config<C>::key_t& key,
    const typename Config<C>::ephemeral_t& frame) {
  if (C10_UNLIKELY(!cache.location_.has_value())) {
    cache.location_ = PyCallKey(frame);
    value_cache->store<CallType::PyCall>(*cache.location_, no_ephemeral_t());
  }

  auto cls_handle = py::handle((PyObject*)key).attr("__class__");
  auto cls = typename Config<C>::cls_t(cls_handle.ptr());
  if (cache.cls_names_.find(cls) == cache.cls_names_.end()) {
    cache.cls_names_[cls] =
        std::string(py::str(cls_handle.attr("__name__")));
  }
  return cls;
}

template <>
void ValueCache::store<CallType::PyCall>(const PyCallKey& key, no_ephemeral_t) {
  auto& locations = std::get<CallType::PyCall>(state_);
  if (C10_UNLIKELY(locations.find(key) == locations.end())) {
      char buffer[2048];
      auto filename = THPUtils_unpackStringView(key.code_->co_filename).data();
      auto name = THPUtils_unpackStringView(key.code_->co_name).data();
      std::snprintf(buffer, sizeof(buffer), "%s(%d): %s",
                filename,
                key.line_number_,
                name);
      locations[key] = std::string(buffer);
  }
}

template <>
void ValueCache::store<CallType::PyCCall>(
    const PyCCallKey& key,
    Config<CallType::PyCCall>::ephemeral_t arg) {
  auto& names = std::get<CallType::PyCCall>(state_);
  if (C10_UNLIKELY(names.find(key) == names.end())) {
    names[key] = std::string(py::repr(arg));
  }
}

// ============================================================================
// == TraceKey cache ==========================================================
// ============================================================================
using python_tracer::TraceKey;

TraceKey nextKey() {
  static std::atomic<uint64_t> key{0};
  return TraceKey{++key};
}

template <CallType C>
struct TraceKeyCacheState {

  struct Hash {
    size_t operator()(const Callsite<C>& key) {
      return c10::get_hash(key.value_);
    }
  };

  TraceKey intern(
      Callsite<C>& callsite,
      typename Config<C>::ephemeral_t ephemeral,
      ValueCache& value_cache) {
    auto it = state.find(callsite);
    if (C10_UNLIKELY(it == state.end())) {
      value_cache.store<C>(callsite.value_, ephemeral);
      it = state.insert({callsite, nextKey()}).first;
    }
    return it->second;
  }

  ska::flat_hash_map<Callsite<C>, TraceKey, Hash> state;
};

// ============================================================================
// == Core CPython data types =================================================
// ============================================================================
// PyObject that allows different threads to record events without colliding.
// It is passed as the second argument when enabling tracing via
// `PyEval_SetProfile`.
struct ThreadLocalResults;
struct TraceContext {
  PyObject_HEAD
  ThreadLocalResults* thread_local_results_;
};

// CPython boilerplate to define `TraceContext` as a proper python object.
static PyTypeObject TraceContextType = {
    PyVarObject_HEAD_INIT(nullptr, 0)
    "TraceContext", /* tp_name */
    sizeof(TraceContext), /* tp_basicsize */
    0, /* tp_itemsize */
    nullptr, /* tp_dealloc */
    0,
    /* tp_vectorcall_offset */
    nullptr, /* tp_getattr */
    nullptr, /* tp_setattr */
    nullptr, /* tp_reserved */
    nullptr, /* tp_repr */
    nullptr, /* tp_as_number */
    nullptr, /* tp_as_sequence */
    nullptr, /* tp_as_mapping */
    nullptr, /* tp_hash  */
    nullptr, /* tp_call */
    nullptr, /* tp_str */
    nullptr, /* tp_getattro */
    nullptr, /* tp_setattro */
    nullptr, /* tp_as_buffer */
    Py_TPFLAGS_DEFAULT, /* tp_flags */
    "Python tracer TLS", /* tp_doc */
    nullptr, /* tp_traverse */
    nullptr, /* tp_clear */
    nullptr, /* tp_richcompare */
    0, /* tp_weaklistoffset */
    nullptr, /* tp_iter */
    nullptr, /* tp_iternext */
    nullptr, /* tp_methods */
    nullptr, /* tp_members */
    nullptr, /* tp_getset */
    nullptr, /* tp_base */
    nullptr, /* tp_dict */
    nullptr, /* tp_descr_get */
    nullptr, /* tp_descr_set */
    0, /* tp_dictoffset */
    nullptr, /* tp_init */
    nullptr, /* tp_alloc */
    PyType_GenericNew, /* tp_new */
    nullptr /* tp_free */
};

class gil_and_restore_thread {
 public:
  gil_and_restore_thread() : initial_thread_state_{PyThreadState_Get()} {}
  ~gil_and_restore_thread() {
    PyThreadState_Swap(initial_thread_state_);

    // `gil_scoped_acquire` is a bit fragile in on-demand mode:
    // https://github.com/pytorch/pytorch/pull/91684#issuecomment-1413154458
    if (!Py_IsInitialized()) {
      gil_.disarm();
    }
  }

  PyThreadState* initial_thread_state() const {
    return initial_thread_state_;
  }

 private:
  pybind11::gil_scoped_acquire gil_;
  PyThreadState* initial_thread_state_;
};

// ============================================================================
// == Thread local cache ======================================================
// ============================================================================
class PythonTracer;

struct ThreadLocalResults {
  ThreadLocalResults(
      PyThreadState* thread_state,
      ValueCache* value_cache,
      PythonTracer* active_tracer,
      int max_depth,
      c10::approx_time_t threshold)
      : thread_state_{thread_state},
        ctx_{(TraceContext*)TraceContextType.tp_alloc(&TraceContextType, 0)},
        value_cache_{value_cache},
        active_tracer_{active_tracer},
        max_depth_(max_depth),
        depth_(-1),
        threshold_(threshold),
        stack_(max_depth) {
    ctx_->thread_local_results_ = this;
  }

  ThreadLocalResults() = delete;
  ThreadLocalResults(const ThreadLocalResults&) = delete;
  ThreadLocalResults(ThreadLocalResults&&) = delete;
  ThreadLocalResults& operator=(const ThreadLocalResults&) = delete;
  ThreadLocalResults& operator=(const ThreadLocalResults&&) = delete;

  ~ThreadLocalResults() {
    if (Py_IsFinalizing()) {
      return;
    }
    if (PyGILState_Check()) {
      Py_DECREF((PyObject*)ctx_);
    } else {
      pybind11::gil_scoped_acquire gil;
      Py_DECREF((PyObject*)ctx_);
    }
  }

  template <CallType C, EventType E, typename Ephemeral, typename... Args>
  TraceKey intern(Ephemeral ephemeral, Args... args) {
    static_assert(
        Config<C>::event_type == E,
        "ThreadLocalResults.intern called from the wrong typed context.");
    auto callsite = Callsite<C>(std::forward<Args>(args)...);
    return std::get<C>(trace_keys_).intern(callsite, ephemeral, *value_cache_);
  }

  void suspend() {
    suspend_ = true;
  }

  void resume() {
    suspend_ = false;
  }

  static constexpr size_t BLOCK_SIZE = 1024;

  PyThreadState* thread_state_;
  TraceContext* ctx_;
  ValueCache* value_cache_;
  PythonTracer* active_tracer_;
  CallTypeHelper<TraceKeyCacheState>::tuple_type trace_keys_;

  int max_depth_;
  int depth_;
  c10::approx_time_t threshold_;
  std::vector<StackEntry> stack_;
  bool suspend_{false};

  int c_call_depth_limit_{-1};
};

// ============================================================================
// == Tracing implementation ==================================================
// ============================================================================
#define IS_PYTHON_3_12 (PY_MAJOR_VERSION == 3 && PY_MINOR_VERSION == 12)
#if IS_PYTHON_3_12
// forward declarations
struct _PyEventHandler;
static PyObject* c_call_callback(
    _PyEventHandler* self,
    PyObject* const* args,
    size_t nargsf,
    PyObject* kwnames);
#endif

class PythonTracer final : public python_tracer::PythonTracerBase {
 public:
  PythonTracer(torch::profiler::impl::RecordQueue* queue);
  // NOLINTNEXTLINE(bugprone-exception-escape)
  ~PythonTracer();

  static int pyProfileFn(
      PyObject* obj,
      PyFrameObject* frame,
      int what,
      PyObject* arg);

  void stop();

  TraceKey getTraceKey(ThreadLocalResults& tls, PyFrameObject* frame);
  TraceKey getTraceKey(ThreadLocalResults& tls, PyCFunctionObject* func);
  void suspend();
  void resume();
  void export_data(const char* path);
  static void init();

  struct StartFrame {
    TraceKey trace_key_;
    c10::approx_time_t start_time{};
  };

 private:
  void recordPyCall(
      ThreadLocalResults& tls,
      PyFrameObject* frame,
      bool is_startup_frame);

  void recordCCall(
      ThreadLocalResults& tls,
      PyFrameObject* frame,
      PyObject* arg);

  template <bool PyCall, typename V>
  void recordCall(ThreadLocalResults& tls, V value);

  template <bool PyReturn>
  void recordReturn(ThreadLocalResults& tls);

  void processUnfinishedCalls();
  ska::flat_hash_map<TraceKey, std::string> mapKeyToName();
  std::string trimPrefix(std::string s);

  static bool initialized_;

  const std::vector<PyThreadState*> interpreterThreads() const;

  std::atomic<bool> active_lock_{false};
  bool active_{false};

  torch::profiler::impl::RecordQueue* queue_;
  PyInterpreterState* interpreter_{nullptr};

  std::deque<ThreadLocalResults> thread_local_results_;
  ValueCache value_cache_;

  typedef int (*Py_tracefunc)(PyObject *, PyFrameObject *, int , PyObject *);
  Py_tracefunc profile_func_;
  bool ignore_c_functions_;

  c10::approx_time_t threshold_;
  c10::time_t start_time_;
  c10::time_t end_time_;

#if IS_PYTHON_3_12
  friend PyObject* c_call_callback(
      _PyEventHandler* self,
      PyObject* const* args,
      size_t nargsf,
      PyObject* kwnames);
#endif

  void log_trace_py(ThreadLocalResults& tls, int what, PyFrameObject* frame) {
#if DEBUG_LOG
    int depth = what == PyTrace_CALL ? tls.depth_ + 1 : tls.depth_;
    const char* type = what == PyTrace_CALL ? "PyTrace_CALL" : "PyTrace_RETURN";
    auto code = THPCodeObjectPtr(PyFrame_GetCode(frame));
    auto filename = THPUtils_unpackStringView(code->co_filename).data();
    auto name = THPUtils_unpackStringView(code->co_name).data();
    auto line_number = PyFrame_GetLineNumber(frame);
    printf("[%p %d] %s %s %s %d\n", &tls, depth, type, filename, name, line_number);
#endif
  }

  void log_trace_c(ThreadLocalResults& tls, int what, PyObject* c_func) {
#if DEBUG_LOG
    int depth = what == PyTrace_C_CALL ? tls.depth_ + 1 : tls.depth_;
    const char* type = what == PyTrace_C_CALL ? "PyTrace_C_CALL" : (what == PyTrace_C_RETURN ? "PyTrace_C_RETURN" : "PyTrace_C_EXCEPTION");
    const char* repr = at::StringView(py::repr(c_func)).str();
    printf("[%p %d] %s %s\n", &tls, depth, type, repr);
#endif
  }
};

#if IS_PYTHON_3_12
#define PROFILER_ID 2
#define PY_MONITORING_EVENT_CALL 4

static bool should_compensate_c_call_events() {
  static const bool result = []() {
    const char* version = Py_GetVersion();
    const char micro = version[5];
    return micro == '0' || (micro <= '4' && version[6] == ' ');
  }();
  return result;
}

struct _PyEventHandler {
  PyObject_HEAD
  vectorcallfunc vectorcall;
};

static PyTypeObject _PyEventHandler_Type = {
    PyVarObject_HEAD_INIT(&PyType_Type, 0) /* ob_base */
    "torch.profiler.python_tracer_event_handler", /* tp_name */
    sizeof(_PyEventHandler), /* tp_basicsize */
    0, /* tp_itemsize */
    (destructor)PyObject_Free, /* tp_dealloc */
    offsetof(_PyEventHandler, vectorcall), /* tp_vectorcall_offset */
    nullptr, /* tp_getattr */
    nullptr, /* tp_setattr */
    nullptr, /* tp_reserved */
    nullptr, /* tp_repr */
    nullptr, /* tp_as_number */
    nullptr, /* tp_as_sequence */
    nullptr, /* tp_as_mapping */
    nullptr, /* tp_hash */
    PyVectorcall_Call, /* tp_call */
    nullptr, /* tp_str */
    nullptr, /* tp_getattro */
    nullptr, /* tp_setattro */
    nullptr, /* tp_as_buffer */
    Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE | Py_TPFLAGS_HAVE_VECTORCALL |
        Py_TPFLAGS_DISALLOW_INSTANTIATION, /* tp_flags */
};

static PyObject* c_call_callback(
    _PyEventHandler* self,
    PyObject* const* args,
    size_t nargsf,
    PyObject* kwnames) {
  // The logic of this function is based on sys_defile_call_or_return defined
  // in https://github.com/python/cpython/blob/v3.12.5/Python/legacy_tracing.c

  PyThreadState* tstate = PyThreadState_GET();
  if (tstate->c_profilefunc != PythonTracer::pyProfileFn) {
    // We don't care this case if tstate->c_profilefunc is not pyProfileFn,
    // just return normally.
    Py_RETURN_NONE;
  }

  PyObject* callable = args[2];
  if (Py_TYPE(callable) == &PyMethod_Type) {
    // The call event of a method with c function is missing on 3.12.0-3.12.4.
    // See
    // https://github.com/python/cpython/commit/257c413cd16ddabcedde413288d0bb93bf872da7
    // Other cases have already be handled by the legacy_tracing, so we only
    // need to handle this case.
    // The exception branches keep the same behavior as CPython.
    PyObject* func = PyMethod_GET_FUNCTION(callable);
    if (!func) {
      return NULL;
    }
    if (PyCFunction_Check(func)) {
      PyFrameObject* frame = PyEval_GetFrame();
      if (!frame) {
        PyErr_SetString(
            PyExc_SystemError, "Missing frame when calling profile function.");
        return NULL;
      }
      Py_INCREF(frame);
      auto& local_results =
          *reinterpret_cast<TraceContext*>(tstate->c_profileobj)
               ->thread_local_results_;
      local_results.active_tracer_->recordCCall(local_results, frame, func);
      Py_DECREF(frame);
    }
  }
  Py_RETURN_NONE;
}

static void registerMonitoringCallback() {
  if (!should_compensate_c_call_events()) {
    return;
  }

  auto sys_module = THPObjectPtr(PyImport_ImportModule("sys"));
  if (!sys_module) {
    PyErr_Clear();
    return;
  }
  auto monitoring =
      THPObjectPtr(PyObject_GetAttrString(sys_module, "monitoring"));
  if (!monitoring) {
    PyErr_Clear();
    return;
  }
  auto result = THPObjectPtr(PyObject_CallMethod(
      monitoring, "use_tool_id", "is", PROFILER_ID, "PyTorch Profiler"));
  if (!result) {
    PyErr_Clear();
    return;
  }
  auto handler = THPObjectPtr(PyObject_NEW(PyObject, &_PyEventHandler_Type));
  if (!handler) {
    PyErr_Clear();
    return;
  }
  reinterpret_cast<_PyEventHandler*>(handler.get())->vectorcall =
      (vectorcallfunc)c_call_callback;
  result = THPObjectPtr(PyObject_CallMethod(
      monitoring,
      "register_callback",
      "iiO",
      PROFILER_ID,
      1 << PY_MONITORING_EVENT_CALL,
      handler.get()));
  if (!result) {
    PyErr_Clear();
    return;
  }
  result = THPObjectPtr(PyObject_CallMethod(
      monitoring,
      "set_events",
      "ii",
      PROFILER_ID,
      1 << PY_MONITORING_EVENT_CALL));
  if (!result) {
    PyErr_Clear();
    return;
  }
}

static void unregisterMonitoringCallback() {
  if (!should_compensate_c_call_events()) {
    return;
  }

  auto sys_module = THPObjectPtr(PyImport_ImportModule("sys"));
  if (!sys_module) {
    PyErr_Clear();
    return;
  }
  auto monitoring =
      THPObjectPtr(PyObject_GetAttrString(sys_module, "monitoring"));
  if (!monitoring) {
    PyErr_Clear();
    return;
  }
  auto tool_name = THPObjectPtr(
      PyObject_CallMethod(monitoring, "get_tool", "i", PROFILER_ID));
  if (!tool_name) {
    PyErr_Clear();
    return;
  }
  if (!(PyBytes_Check(tool_name) || PyUnicode_Check(tool_name))) {
    return;
  }
  const char* str = THPUtils_unpackStringView(tool_name).data();
  if (strcmp(str, "PyTorch Profiler") != 0) {
    return;
  }
  auto none = THPObjectPtr(Py_None);
  Py_INCREF(Py_None);
  auto result = THPObjectPtr(PyObject_CallMethod(
      monitoring,
      "register_callback",
      "iiO",
      PROFILER_ID,
      1 << PY_MONITORING_EVENT_CALL,
      none.get()));
  if (!result) {
    PyErr_Clear();
    return;
  }
  result = THPObjectPtr(
      PyObject_CallMethod(monitoring, "set_events", "ii", PROFILER_ID, 0));
  if (!result) {
    PyErr_Clear();
    return;
  }
  result = THPObjectPtr(
      PyObject_CallMethod(monitoring, "free_tool_id", "i", PROFILER_ID));
  if (!result) {
    PyErr_Clear();
    return;
  }
}
#endif

PythonTracer::PythonTracer(torch::profiler::impl::RecordQueue* queue)
    : queue_(queue) {
  // TORCH_CHECK(queue_ != nullptr);

  bool expected{false};
  active_ = active_lock_.compare_exchange_strong(expected, true);
  if (!active_) {
    printf(
        "There is already an active Python tracer. "
        "Refusing to register profile functions.");
    return;
  }

  gil_and_restore_thread gil;
  start_time_ = libkineto::getTimeNs();
  interpreter_ = PyInterpreterState_Get();

  if (!gil.initial_thread_state()) {
    // TORCH_WARN("PyThreadState_Get returned NULL");
    return;
  }

  const ProfilerConfig& config = queue->config();
  int max_depth = config.python_tracer_max_depth;
  uint64_t threshold_ns = config.python_tracer_threshold_ns;
  ignore_c_functions_ = config.python_tracer_ignore_c_functions;

  c10::approx_time_t threshold = static_cast<c10::approx_time_t>(0);

  max_depth = max_depth > 0 ? std::min(max_depth, 2048) : 2048;

  threshold = (c10::approx_time_t)((double)threshold_ns);
      (c10::approx_time_t)((double)threshold_ns / queue->scaleFactor());
  profile_func_ = &PythonTracer::pyProfileFn;

  auto interpreter_threads = interpreterThreads();

  // Register the tracer in each thread.
  for (const auto thread_state : interpreter_threads) {
    PyThreadState_Swap(thread_state);

    thread_local_results_.emplace_back(thread_state, &value_cache_, this, max_depth, threshold);

    // When we begin profiling there are already frames on the Python
    // interpreter stack. To ensure a complete trace, we must push calls
    // to all the prior frames onto our event stack. (We stop at depth=128)

    std::vector<THPFrameObjectPtr> current_stack;
    auto frame = PyThreadState_GetFrame(thread_state);

    int depth = 0; // Make sure we can't infinite loop.
    int depth_limit = max_depth;
    while (frame != nullptr) {
      current_stack.emplace_back(frame);
      if (++depth == depth_limit) {
        break;
      }

      // NB: `PyFrame_GetBack` returns a strong reference.
      frame = PyFrame_GetBack(frame);
    }

    ThreadLocalResults& tls = thread_local_results_.back();
    for (auto it = current_stack.rbegin(); it != current_stack.rend(); it++) {
      PyFrameObject* frame = it->get();
      this->log_trace_py(tls, PyTrace_CALL, frame);
      recordPyCall(tls, frame, true);
      auto frame_refcount = Py_REFCNT(frame);
      // We hold one reference in `current_stack`, and the interpreter holds
      // another.
      // TORCH_INTERNAL_ASSERT(frame_refcount >= 2, frame_refcount);
    }
    if (!ignore_c_functions_) {
      tls.c_call_depth_limit_ = tls.depth_;
    }
  }
  int i = 0;
  for (const auto thread_state : interpreter_threads) {
    // Note:
    //   This profile will not compose with other CPython profilers, and
    //   cannot be round tripped via `sys.settrace(sys.gettrace())`
    PyThreadState_Swap(thread_state);
    PyEval_SetProfile(profile_func_, (PyObject*)thread_local_results_[i++].ctx_);
  }
#if IS_PYTHON_3_12
  registerMonitoringCallback();
#endif
}

const std::vector<PyThreadState*> PythonTracer::interpreterThreads() const {
  pybind11::gil_scoped_acquire gil;
  std::vector<PyThreadState*> out;
  if (interpreter_) {
  // if (SOFT_ASSERT(interpreter_)) {
    auto* thread_state = PyInterpreterState_ThreadHead(interpreter_);
    while (thread_state != nullptr) {
      out.push_back(thread_state);
      thread_state = PyThreadState_Next(thread_state);
    }
  }
  return out;
}

void PythonTracer::stop() {
  if (Py_IsFinalizing()) {
    return;
  }
  end_time_ = libkineto::getTimeNs();
  gil_and_restore_thread gil;
  if (active_) {
    // TORCH_INTERNAL_ASSERT(profile_func_ != nullptr, "profile_func_ should not be null");
    for (const auto thread_state : interpreterThreads()) {
      if (thread_state->c_profilefunc == profile_func_) {
        PyThreadState_Swap(thread_state);
        PyEval_SetProfile(nullptr, nullptr);
      }
    }

#if IS_PYTHON_3_12
    unregisterMonitoringCallback();
#endif

    auto lock_returned = active_lock_.compare_exchange_strong(active_, false);
    active_ = false;
    // SOFT_ASSERT(lock_returned, "Failed to return python tracer lock.");
  }
}

// NOLINTNEXTLINE(bugprone-exception-escape)
PythonTracer::~PythonTracer() {
  if (active_) {
    //TORCH_WARN("`PythonTracer::stop()` was not called.");
    stop();
  }
}

TraceKey PythonTracer::getTraceKey(ThreadLocalResults& tls, PyFrameObject* frame) {
  TraceKey key;
  static constexpr auto E = EventType::PyCall;
  key = tls.intern<CallType::PyCall, E>(no_ephemeral_t(), frame);
  return key;
}

TraceKey PythonTracer::getTraceKey(ThreadLocalResults& tls, PyCFunctionObject* func) {
  return tls.intern<CallType::PyCCall, EventType::PyCCall>(
      reinterpret_cast<PyObject*>(func), (void*)(func->m_ml));
}

void PythonTracer::recordPyCall(
    ThreadLocalResults& tls,
    PyFrameObject* frame,
    bool is_startup_frame) {
  recordCall<true>(tls, frame);
}

void PythonTracer::recordCCall(
    ThreadLocalResults& tls,
    PyFrameObject* frame,
    PyObject* arg) {
  // TORCH_INTERNAL_ASSERT_DEBUG_ONLY(PyCFunction_Check(arg));
  recordCall<false>(tls, reinterpret_cast<PyCFunctionObject*>(arg));
}

template <bool PyCall, typename V>
void PythonTracer::recordCall(ThreadLocalResults& tls, V value) {
  int depth = ++tls.depth_;
  if (C10_UNLIKELY(depth >= tls.max_depth_)) {
    return;
  }
  if (C10_LIKELY(!tls.suspend_)) {
    Py_INCREF(value);
    tls.stack_[depth] = {c10::getApproximateTime(), value};
  } else {
    tls.stack_[depth] = {0, nullptr};
  }
}

template <bool PyReturn>
void PythonTracer::recordReturn(ThreadLocalResults& tls) {
  if (C10_UNLIKELY(tls.depth_ == -1)) {
    return;
  }
  int depth = tls.depth_--;
  if (C10_UNLIKELY(depth >= tls.max_depth_)) {
    return;
  }
  auto& entry = tls.stack_[depth];
  if (C10_UNLIKELY(std::holds_alternative<std::nullptr_t>(entry.value_))) {
    return;
  }
  if (C10_UNLIKELY(PyReturn && !std::holds_alternative<PyFrameObject*>(entry.value_))) {
    return;
  }
  if (C10_UNLIKELY(!PyReturn && !std::holds_alternative<PyCFunctionObject*>(entry.value_))) {
    return;
  }

  auto end = c10::getApproximateTime();
  auto start = entry.start_;
  if (end - start < tls.threshold_) {
    if constexpr (PyReturn) {
      Py_DECREF(std::get<PyFrameObject*>(entry.value_));
    } else {
      Py_DECREF(std::get<PyCFunctionObject*>(entry.value_));
    }
    return;
  }
  if constexpr (PyReturn) {
    auto frame = std::get<PyFrameObject*>(entry.value_);
    TraceKey key = getTraceKey(tls, frame);
    queue_->getSubqueue()->emplace_py_call(key, StartEndTime{start, end});
    Py_DECREF(frame);
  } else {
    auto func = std::get<PyCFunctionObject*>(entry.value_);
    TraceKey key = getTraceKey(tls, func);
    queue_->getSubqueue()->emplace_py_call(key, StartEndTime{start, end});
    Py_DECREF(func);
  }
}

void PythonTracer::processUnfinishedCalls() {
  PyGILState_STATE state;
  bool need_release_gil = false;
  if (!PyGILState_Check()) {
    state = PyGILState_Ensure();
    need_release_gil = true;
  }
  for (auto& tls : thread_local_results_) {
    if (tls.depth_ < 0) {
      // No unfinished frames.
      continue;
    }
    for (int i = std::min(tls.depth_, tls.max_depth_ - 1); i >= 0; --i) {
      auto& entry = tls.stack_[i];
      auto start = entry.start_;
      auto& value = entry.value_;
      TraceKey key;
      if (std::holds_alternative<PyFrameObject*>(value)) {
        key = getTraceKey(tls, std::get<PyFrameObject*>(value));
        Py_DECREF(std::get<PyFrameObject*>(value));
      } else if (std::holds_alternative<PyCFunctionObject*>(value)) {
        key = getTraceKey(tls, std::get<PyCFunctionObject*>(value));
        Py_DECREF(std::get<PyCFunctionObject*>(value));
      } else {
        continue;
      }
      queue_->getSubqueue()->emplace_py_call(key, StartEndTime{start, 0});
    }
  }
  if (need_release_gil) {
    PyGILState_Release(state);
  }
}

void PythonTracer::suspend() {
  auto t = PyThreadState_GET();
  for (auto& tls : thread_local_results_) {
    if (tls.thread_state_ == t) {
      tls.suspend();
      return;
    }
  }
}

void PythonTracer::resume() {
  auto t = PyThreadState_GET();
  for (auto& tls : thread_local_results_) {
    if (tls.thread_state_ == t) {
      tls.resume();
      return;
    }
  }
}

// ============================================================================
// == API =====================================================================
// ============================================================================
int PythonTracer::pyProfileFn(
    PyObject* obj,
    PyFrameObject* frame,
    int what,
    PyObject* arg) {
  auto& local_results =
      *reinterpret_cast<TraceContext*>(obj)->thread_local_results_;
  switch (what) {
    case PyTrace_CALL: {
      local_results.active_tracer_->log_trace_py(local_results, what, frame);
      local_results.active_tracer_->recordPyCall(local_results, frame, false);
      break;
    }

    case PyTrace_C_CALL: {
      if (!local_results.active_tracer_->ignore_c_functions_) {
        local_results.active_tracer_->log_trace_c(local_results, what, arg);
        local_results.active_tracer_->recordCCall(local_results, frame, arg);
      }
      break;
    }

    case PyTrace_RETURN: {
      local_results.active_tracer_->log_trace_py(local_results, what, frame);
      local_results.active_tracer_->recordReturn<true>(local_results);
      if (!local_results.active_tracer_->ignore_c_functions_) {
        if (local_results.depth_ < local_results.c_call_depth_limit_) {
          local_results.c_call_depth_limit_ = local_results.depth_;
        }
      }
      break;
    }

    case PyTrace_C_EXCEPTION:
    case PyTrace_C_RETURN: {
      if (!local_results.active_tracer_->ignore_c_functions_) {
        if (local_results.depth_ <= local_results.c_call_depth_limit_) {
          break;
        }
        local_results.active_tracer_->log_trace_c(local_results, what, arg);
        local_results.active_tracer_->recordReturn<false>(local_results);
      }
      break;
    }
  }
  return 0;
}

std::string PythonTracer::trimPrefix(std::string s) {
  static const auto prefixes = []() {
    pybind11::gil_scoped_acquire gil;
    auto prefix_list_handle = py::module::import(PYKI_DOT "profiling.python_tracer")
        .attr("_prefix_regex")();
    auto prefix_list = prefix_list_handle.ptr();
    return pyki::pyListToStringVector(prefix_list);
  }();

  for (const auto& p : prefixes) {
    if (s.compare(0, p.size(), p) == 0) {
      s.erase(0, p.size());
      return s;
    }
  }
  return s;
}
ska::flat_hash_map<TraceKey, std::string> PythonTracer::mapKeyToName() {
  ska::flat_hash_map<TraceKey, std::string> map;
  for (auto &tls : thread_local_results_) {
    auto trace_keys = tls.trace_keys_;
    for (auto& it : std::get<CallType::PyCall>(trace_keys).state) {
      auto trace_key = it.second;
      auto cache = std::get<CallType::PyCall>(value_cache_.state());
      auto name_it = cache.find(it.first.value_);
      auto name = (name_it != cache.end() ? trimPrefix(name_it->second) : "unknown");
      map.insert({trace_key, name});
    }
    for (auto& it : std::get<CallType::PyCCall>(trace_keys).state) {
      auto trace_key = it.second;
      auto cache = std::get<CallType::PyCCall>(value_cache_.state());
      auto name_it = cache.find(it.first.value_);
      auto name = (name_it != cache.end() ? name_it->second : "unknown");
      map.insert({trace_key, name});
    }
  }
  return map;
}

void PythonTracer::export_data(const char* path) {
  auto converter = queue_->converter_func();
  // post process
  processUnfinishedCalls();
  auto name_map = mapKeyToName();

  // write file
  pyki::ChromeTraceEventWritter trace_writer(path);
  int pid = ::getpid();
  trace_writer.addMetadata("pyki_trace_type", "\"python_trace\"");
  trace_writer.addMetadata("with_stack", "1");
  trace_writer.startTraceEvent();
  for (auto& subqueue_it : queue_->sub_queues()) {
    auto& sub_queue = *subqueue_it.second;
    auto tid = sub_queue.os_tid();
    auto thread_name = sub_queue.thread_name();
    for (auto& i : sub_queue.py_calls()) {
      auto start = converter(i.second.start_time_);
      auto end = i.second.end_time_ == static_cast<c10::approx_time_t>(0)
                ? std::max(static_cast<c10::time_t>(end_time_), start)
                : converter(i.second.end_time_);
      auto name_it = name_map.find(i.first);
      auto name = name_it != name_map.end() ? name_it->second.c_str() : "unknown";
      trace_writer.addTraceEvent("python_function", name, pid, tid, start, end - start);
    }
    trace_writer.addThreadMetadata(thread_name.c_str(), pid, tid, start_time_);
  }

  trace_writer.addProcessMetadata("Python", pid, start_time_);
}

bool PythonTracer::initialized_{false};
void PythonTracer::init() {
  if (initialized_) {
    return;
  }
  pybind11::gil_scoped_acquire gil;
  PyType_Ready(&torch::profiler::impl::TraceContextType);
  initialized_ = true;
}

std::unique_ptr<python_tracer::PythonTracerBase> python_tracer::PythonTracerBase::make(RecordQueue* queue) {
  PythonTracer::init();
  return std::make_unique<PythonTracer>(queue);
}

} // namespace torch::autograd::profiler