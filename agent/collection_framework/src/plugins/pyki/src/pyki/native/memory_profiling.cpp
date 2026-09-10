#include "memory_profiling_common.hpp"
#include "memory_profiling.hpp"
#include "arch.hpp"
#include "crash_protection.hpp"
#include "logging.hpp"
#include "recorder.hpp"
#include "sampler.hpp"
#include "lock.hpp"
#include "stack_trace.hpp"

namespace pyki {

struct PyKiAllocatorContext {
    PyMemAllocatorEx allocator;
    PyMemAllocatorDomain domain;
    u64 sample_interval;
    int max_nframes;
    bool native;
};

static const int MEM_DOMAIN_COUNT = PYMEM_DOMAIN_OBJ + 1;
static const bool DEFAULT_NATIVE = false;

static PyKiAllocatorContext allocator_contexts[MEM_DOMAIN_COUNT]{
    {{},
     PYMEM_DOMAIN_RAW,
     DEFAULT_SAMPLE_INTERVAL,
     DEFAULT_MAX_NFRAMES,
     DEFAULT_NATIVE},
    {{},
     PYMEM_DOMAIN_MEM,
     DEFAULT_SAMPLE_INTERVAL,
     DEFAULT_MAX_NFRAMES,
     DEFAULT_NATIVE},
    {{},
     PYMEM_DOMAIN_OBJ,
     DEFAULT_SAMPLE_INTERVAL,
     DEFAULT_MAX_NFRAMES,
     DEFAULT_NATIVE},
};

static pthread_key_t _sample_meta_tls;
static bool _sample_meta_tls_created = false;

static void handle_malloc(PyKiAllocatorContext* ctx, size_t size,
                          StackTraceArgs* args) {
    SampleMeta* meta = (SampleMeta*)pthread_getspecific(_sample_meta_tls);
    if (meta == NULL) {
        meta = new SampleMeta();
        meta->sample_interval = ULLONG_MAX;
        meta->rnd = Sampler::generate_random_number();
        meta->sample_point = ULLONG_MAX;
        meta->sample_remaining = ULLONG_MAX;
        pthread_setspecific(_sample_meta_tls, (void*)meta);
    }

    if (meta->sample_interval != ctx->sample_interval) {
        meta->sample_interval = ctx->sample_interval;
        meta->sample_point = Sampler::generate_next_sample_point(
            ctx->sample_interval, &meta->rnd);
        meta->sample_remaining = meta->sample_point;
    }

    if (size >= meta->sample_remaining) {
        // hit sample point
        {
            ThreadCritical tc;
            if (tc.try_enter()) {
                int level_index = tc.get_index();
                u32 thread_id = tc.get_thread_id();
                CrashProtection::call(tc, [=] {
                    u32 stack_trace_id = get_stack_trace(level_index, *args);
                    Recorder::instance()->record_malloc_event(
                        level_index, thread_id, stack_trace_id,
                        size > meta->sample_point ?
                            size : // large allocation
                            meta->sample_point + (size - meta->sample_remaining)); // sample-point + overflow
                });
            }
        }
        meta->sample_point = Sampler::generate_next_sample_point(
            ctx->sample_interval, &meta->rnd);
        meta->sample_remaining = meta->sample_point;
    }
    else {
        meta->sample_remaining -= size;
    }
}

#define STACK_TRACE_ARGS                               \
    StackTraceArgs args = {                            \
        .ucontext = nullptr,                           \
        .max_nframes = allocator_context->max_nframes, \
        .native = allocator_context->native,           \
        .pc = callerPC(),                              \
        .sp = (const void*)callerSP(),                 \
        .fp = (const void*)callerFP(),                 \
    }

static void* pyki_malloc(void* ctx, size_t size) {
    PyKiAllocatorContext* allocator_context = (PyKiAllocatorContext*)ctx;

    void* result = allocator_context->allocator.malloc(
        allocator_context->allocator.ctx, size);
    if (result != NULL) {
        STACK_TRACE_ARGS;
        handle_malloc(allocator_context, size, &args);
    }
    return result;
}

static void* pyki_calloc(void* ctx, size_t nelem, size_t elsize) {
    PyKiAllocatorContext* allocator_context = (PyKiAllocatorContext*)ctx;
    void* result = allocator_context->allocator.calloc(
        allocator_context->allocator.ctx, nelem, elsize);
    if (result != NULL) {
        STACK_TRACE_ARGS;
        handle_malloc(allocator_context, nelem * elsize, &args);
    }
    return result;
}

static void* pyki_realloc(void* ctx, void* ptr, size_t new_size) {
    PyKiAllocatorContext* allocator_context = (PyKiAllocatorContext*)ctx;
    void* result = allocator_context->allocator.realloc(
        allocator_context->allocator.ctx, ptr, new_size);
    if (result != NULL) {
        STACK_TRACE_ARGS;
        handle_malloc(allocator_context, new_size, &args);
    }
    return result;
}

static void pyki_free(void* ctx, void* ptr) {
    PyKiAllocatorContext* allocator_context = (PyKiAllocatorContext*)ctx;
    allocator_context->allocator.free(allocator_context->allocator.ctx, ptr);
}

static void hook_allocator(PyMemAllocatorDomain domain, int sample_interval,
                           bool native) {
    PyMem_GetAllocator(domain, &allocator_contexts[domain].allocator);
    PyMemAllocatorEx new_allocator = PyMemAllocatorEx{
        .ctx = &allocator_contexts[domain],
        .malloc = pyki_malloc,
        .calloc = pyki_calloc,
        .realloc = pyki_realloc,
        .free = pyki_free,
    };
    allocator_contexts[domain].sample_interval = sample_interval;
    allocator_contexts[domain].native = native;
    PyMem_SetAllocator(domain, &new_allocator);
}

static void restore_allocator(PyMemAllocatorDomain domain) {
    PyMem_SetAllocator(domain, &allocator_contexts[domain].allocator);
    allocator_contexts[domain].sample_interval = DEFAULT_SAMPLE_INTERVAL;
    allocator_contexts[domain].max_nframes = DEFAULT_MAX_NFRAMES;
    allocator_contexts[domain].native = DEFAULT_NATIVE;
}

MemoryProfiling* MemoryProfiling::_instance = new MemoryProfiling();

MemoryProfiling::MemoryProfiling()
    : _mutex(),
      _enabled(false),
      _interval(0),
      _max_depth(0),
      _native(false) {}

PyObject* MemoryProfiling::start(PyObject* args, PyObject* keywds) {
    MutexLocker locker(_mutex);
    if (_enabled) {
        SET_ERROR_MESSAGE("Memory profiling is already enabled");
        return NULL;
    }

    if (!_sample_meta_tls_created) {
        pthread_key_create(&_sample_meta_tls, delete_sample_meta);
        _sample_meta_tls_created = true;
    }

    int interval_bytes = 512 * 1024;
    int max_depth = 64;
    int native = false;
    static const char* kwlist[] = {"interval_bytes", "max_depth", "native",
                                   NULL};

    if (!PyArg_ParseTupleAndKeywords(args, keywds, "|$iip", (char**)kwlist,
                                     &interval_bytes, &max_depth, &native)) {
        return NULL;
    }

    this->_interval = interval_bytes;
    this->_max_depth = max_depth;
    this->_native = native;

    hook_allocator(PYMEM_DOMAIN_OBJ, interval_bytes, native);
    hook_allocator(PYMEM_DOMAIN_MEM, interval_bytes, native);
    hook_allocator(PYMEM_DOMAIN_RAW, interval_bytes, native);
    _enabled = true;
    Py_RETURN_NONE;
}

PyObject* MemoryProfiling::stop() {
    MutexLocker locker(_mutex);
    if (!_enabled) {
        SET_ERROR_MESSAGE("Memory profiling is not enabled");
        return NULL;
    }
    restore_allocator(PYMEM_DOMAIN_OBJ);
    restore_allocator(PYMEM_DOMAIN_MEM);
    restore_allocator(PYMEM_DOMAIN_RAW);
    _enabled = false;
    Py_RETURN_NONE;
}

}  // namespace pyki