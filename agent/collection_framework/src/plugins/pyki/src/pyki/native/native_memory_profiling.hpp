#pragma once

#include <mutex>
#include <memory>
#include "pyki.hpp"
#include "mutex.hpp"
#include "stack_trace.hpp"
#include "linker_shenanigans.h"
#include "hooks.h"

namespace pyki {

#if defined(__GLIBC__)
#    define PYKI_FAST_TLS __attribute__((tls_model("initial-exec")))
#else
#    define PYKI_FAST_TLS __attribute__((tls_model("local-dynamic")))
#endif

struct RecursionGuard
{
    RecursionGuard()
    : wasLocked(isActive())
    {
        setValue(true);
    }

    ~RecursionGuard()
    {
        setValue(wasLocked);
    }

    const bool wasLocked;

#ifdef __linux__
    PYKI_FAST_TLS static thread_local bool _isActive;

    static void initialize()
    {
    }

    static __attribute__((always_inline)) inline bool isActive()
    {
        return _isActive;
    }

    static __attribute__((always_inline)) inline void setValue(bool value)
    {
        _isActive = value;
    }
#else
    static pthread_key_t isActiveKey;

    static void initialize()
    {
        if (0 != pthread_key_create(&isActiveKey, nullptr)) {
            throw std::runtime_error{"Failed to create pthread key"};
        }
    }

    static __attribute__((always_inline)) inline bool isActive()
    {
        return pthread_getspecific(isActiveKey) != 0;
    }

    static __attribute__((always_inline)) inline void setValue(bool value)
    {
        static bool true_constant = true;
        if (pthread_setspecific(isActiveKey, value ? &true_constant : (void*)0) != 0) {
            abort();
        }
    }
#endif
};

class NativeMemoryProfiling {
   public:
    NativeMemoryProfiling();

    PyObject* start(PyObject* args, PyObject* keywds);
    PyObject* stop();

    static NativeMemoryProfiling* instance() { return _instance; };
    void registerPymallocHooks() const noexcept;
    void unregisterPymallocHooks() const noexcept;

    __attribute__((always_inline)) inline static void
    trackAllocation(void* ptr, size_t size, StackTraceArgs* args, hooks::Allocator func)
    {
        if (RecursionGuard::isActive()) {
            return;
        }
        RecursionGuard guard;
        trackAllocationImpl(ptr, size, args, func);
    }

    __attribute__((always_inline)) inline static void
    trackDeallocation(void* ptr, size_t size, hooks::Allocator func)
    {
        if (RecursionGuard::isActive()) {
            return;
        }
        RecursionGuard guard;
        trackDeallocationImpl(ptr, size, func);
    }

    __attribute__((always_inline)) inline static void invalidate_module_cache()
    {
        if (RecursionGuard::isActive()) {
            return;
        }
        RecursionGuard guard;
        invalidate_module_cache_impl();
    }

   private:
    static NativeMemoryProfiling* _instance;
    static pyki::linker::SymbolPatcher d_patcher;

    static void trackAllocationImpl(void* ptr, size_t size, StackTraceArgs* args, hooks::Allocator func);
    static void trackDeallocationImpl(void* ptr, size_t size, hooks::Allocator func);
    static void invalidate_module_cache_impl();

    static std::unique_ptr<std::mutex> s_mutex;
    Mutex _mutex;

    bool _enabled;
    static u64 _interval;
    static int _max_depth;
    static bool _leaks;
};

}  // namespace pyki
