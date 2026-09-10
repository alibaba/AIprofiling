#include <time.h>
#define PY_SSIZE_T_CLEAN
#include <Python.h>
#include "memory_profiling_common.hpp"
#include "native_memory_profiling.hpp"
#include "arch.hpp"
#include "crash_protection.hpp"
#include "logging.hpp"
#include "recorder.hpp"
#include "sampler.hpp"
#include "lock.hpp"
#include "stack_trace.hpp"
#include "hooks.h"

namespace pyki {
static pthread_key_t _sample_meta_tls;
static bool _sample_meta_tls_created = false;

#ifdef __linux__
PYKI_FAST_TLS thread_local bool RecursionGuard::_isActive = false;
#else
pthread_key_t RecursionGuard::isActiveKey;
#endif

static struct
{
    PyMemAllocatorEx raw;
    PyMemAllocatorEx mem;
    PyMemAllocatorEx obj;
} s_orig_pymalloc_allocators;

std::unique_ptr<std::mutex> NativeMemoryProfiling::s_mutex(new std::mutex);

NativeMemoryProfiling* NativeMemoryProfiling::_instance = new NativeMemoryProfiling();

NativeMemoryProfiling::NativeMemoryProfiling(): _mutex(), _enabled(false) {}

pyki::linker::SymbolPatcher NativeMemoryProfiling::d_patcher;

u64 NativeMemoryProfiling::_interval = DEFAULT_SAMPLE_INTERVAL;
int NativeMemoryProfiling::_max_depth = DEFAULT_MAX_NFRAMES;
bool NativeMemoryProfiling::_leaks = true;

void
NativeMemoryProfiling::registerPymallocHooks() const noexcept
{
    PyMemAllocatorEx alloc;

    PyMem_GetAllocator(PYMEM_DOMAIN_RAW, &alloc);
    if (alloc.free == &intercept::pymalloc_free) {
        // Nothing to do; our hooks are already installed.
        return;
    }

    alloc.malloc = intercept::pymalloc_malloc;
    alloc.calloc = intercept::pymalloc_calloc;
    alloc.realloc = intercept::pymalloc_realloc;
    alloc.free = intercept::pymalloc_free;
    PyMem_GetAllocator(PYMEM_DOMAIN_RAW, &s_orig_pymalloc_allocators.raw);
    PyMem_GetAllocator(PYMEM_DOMAIN_MEM, &s_orig_pymalloc_allocators.mem);
    PyMem_GetAllocator(PYMEM_DOMAIN_OBJ, &s_orig_pymalloc_allocators.obj);
    alloc.ctx = &s_orig_pymalloc_allocators.raw;
    PyMem_SetAllocator(PYMEM_DOMAIN_RAW, &alloc);
    alloc.ctx = &s_orig_pymalloc_allocators.mem;
    PyMem_SetAllocator(PYMEM_DOMAIN_MEM, &alloc);
    alloc.ctx = &s_orig_pymalloc_allocators.obj;
    PyMem_SetAllocator(PYMEM_DOMAIN_OBJ, &alloc);
}

void
NativeMemoryProfiling::unregisterPymallocHooks() const noexcept
{
    PyMem_SetAllocator(PYMEM_DOMAIN_RAW, &s_orig_pymalloc_allocators.raw);
    PyMem_SetAllocator(PYMEM_DOMAIN_MEM, &s_orig_pymalloc_allocators.mem);
    PyMem_SetAllocator(PYMEM_DOMAIN_OBJ, &s_orig_pymalloc_allocators.obj);
}

PyObject* NativeMemoryProfiling::start(PyObject* args, PyObject* keywds) {
    MutexLocker locker(_mutex);
    if (_enabled) {
        SET_ERROR_MESSAGE("Native memory profiling is already enabled");
        return NULL;
    }

    if (!_sample_meta_tls_created) {
        if (pthread_key_create(&_sample_meta_tls, delete_sample_meta) != 0) {
            SET_ERROR_MESSAGE("Failed to create pthread key for sample meta TLS");
            return NULL;
        }
        _sample_meta_tls_created = true;
    }

    int interval_bytes;
    int max_depth;
    int leaks;
    static const char* kwlist[] = {"interval_bytes", "max_depth", "leaks", NULL};

    if (!PyArg_ParseTupleAndKeywords(args, keywds, "|$iip", (char**)kwlist, &interval_bytes, &max_depth, &leaks)) {
        return NULL;
    }
    NativeMemoryProfiling::_interval = interval_bytes;
    NativeMemoryProfiling::_max_depth = max_depth;
    NativeMemoryProfiling::_leaks = leaks;

    RecursionGuard guard;
    std::unique_lock<std::mutex> lock(*s_mutex);
    hooks::ensureAllHooksAreValid();
    registerPymallocHooks();
    d_patcher.overwrite_symbols();

    _enabled = true;
    Py_RETURN_NONE;
}

PyObject* NativeMemoryProfiling::stop() {
    MutexLocker locker(_mutex);
    RecursionGuard guard;
    if (!_enabled) {
        SET_ERROR_MESSAGE("Native memory profiling is not enabled");
        return NULL;
    }
    std::unique_lock<std::mutex> lock(*s_mutex);
    d_patcher.restore_symbols();
    unregisterPymallocHooks();
    _enabled = false;
    Py_RETURN_NONE;
}

void
NativeMemoryProfiling::trackAllocationImpl(void* ptr, size_t size, StackTraceArgs* args, hooks::Allocator func) {
    if (size == 0) return;

    SampleMeta* meta = (SampleMeta*)pthread_getspecific(_sample_meta_tls);
    if (meta == nullptr) {
        meta = new SampleMeta();
        meta->sample_interval = ULLONG_MAX;
        meta->rnd = (u64)time(nullptr);
        meta->sample_point = ULLONG_MAX;
        meta->sample_remaining = ULLONG_MAX;
        pthread_setspecific(_sample_meta_tls, (void*)meta);
    }

    if (meta->sample_interval != NativeMemoryProfiling::_interval) {
        meta->sample_interval = NativeMemoryProfiling::_interval;
        meta->sample_point = Sampler::generate_next_sample_point(NativeMemoryProfiling::_interval, &meta->rnd);
        meta->sample_remaining = meta->sample_point;
    }

    if (size >= meta->sample_remaining) {
        // hit sample point
        {
            ThreadCritical tc;
            if (tc.try_enter()) {
                int level_index = tc.get_index();
                u32 thread_id = tc.get_thread_id();
                size_t _size = size > meta->sample_point ?
                    size : // large allocation
                    meta->sample_point + (size - meta->sample_remaining); // sample-point + overflow
                CrashProtection::call(tc, [=] {
                    u32 stack_trace_id = get_stack_trace(level_index, *args);
                    Recorder::instance()->record_native_malloc_event(level_index, thread_id, stack_trace_id, _size, ptr, func);
                });
            }
        }
        meta->sample_point = Sampler::generate_next_sample_point(NativeMemoryProfiling::_interval, &meta->rnd);
        meta->sample_remaining = meta->sample_point;
    } else {
        meta->sample_remaining -= size;
    }
}

void
NativeMemoryProfiling::trackDeallocationImpl(void* ptr, size_t size, hooks::Allocator func) {
    if (!NativeMemoryProfiling::_leaks) {
        return;
    }
    ThreadCritical tc;
    if (tc.try_enter()) {
        int level_index = tc.get_index();
        u32 thread_id = tc.get_thread_id();
        CrashProtection::call(tc, [=] {
            Recorder::instance()->record_native_free_event(level_index, thread_id, size, ptr, func);
        });
    }
}

void
NativeMemoryProfiling::invalidate_module_cache_impl()
{
    std::unique_lock<std::mutex> lock(*s_mutex);
    d_patcher.overwrite_symbols();
}

}  // namespace pyki
