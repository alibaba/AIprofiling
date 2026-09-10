#include <cassert>
#include <cstdio>
#include <mutex>
#include <unordered_set>

#include "hooks.h"
#include "arch.hpp"
#include "native_memory_profiling.hpp"
#include "memory_profiling_common.hpp"

namespace pyki::hooks {

#define NATIVE_STACK_TRACE_ARGS                        \
    StackTraceArgs args = {                            \
        .ucontext = nullptr,                           \
        .max_nframes = DEFAULT_MAX_NFRAMES,            \
        .native = true,                                \
        .pc = callerPC(),                              \
        .sp = (const void*)callerSP(),                 \
        .fp = (const void*)callerFP(),                 \
    }

#if defined(__linux__)
int
phdr_symfind_callback(dl_phdr_info* info, [[maybe_unused]] size_t size, void* data) noexcept
{
    auto result = reinterpret_cast<symbol_query*>(data);

    // From all maps without name, we only want to visit the executable (first map)
    if (result->maps_visited++ != 0 && !info->dlpi_name[0]) {
        return 0;
    }

    if (strstr(info->dlpi_name, "linux-vdso.so.1")) {
        // This is an evil place that don't have symbols
        return 0;
    }

    for (auto phdr = info->dlpi_phdr, end = phdr + info->dlpi_phnum; phdr != end; ++phdr) {
        if (phdr->p_type != PT_DYNAMIC) {
            continue;
        }

        const auto* dyn = reinterpret_cast<const Dyn*>(phdr->p_vaddr + info->dlpi_addr);
        SymbolTable symbols(info->dlpi_addr, dyn);

        const auto offset = symbols.getSymbolAddress(result->symbol_name);
        if (offset == 0) {
            continue;
        }

        result->address = reinterpret_cast<void*>(offset);
        return 1;
    }

    return 0;
}
#endif

AllocatorKind
allocatorKind(const Allocator& allocator)
{
    switch (allocator) {
        case Allocator::CALLOC:
        case Allocator::MALLOC:
        case Allocator::MEMALIGN:
        case Allocator::POSIX_MEMALIGN:
        case Allocator::ALIGNED_ALLOC:
        case Allocator::PVALLOC:
        case Allocator::REALLOC:
        case Allocator::VALLOC:
        case Allocator::PYMALLOC_MALLOC:
        case Allocator::PYMALLOC_CALLOC:
        case Allocator::PYMALLOC_REALLOC: {
            return AllocatorKind::SIMPLE_ALLOCATOR;
        }
        case Allocator::FREE:
        case Allocator::PYMALLOC_FREE: {
            return AllocatorKind::SIMPLE_DEALLOCATOR;
        }
        case Allocator::MMAP: {
            return AllocatorKind::RANGED_ALLOCATOR;
        }
        case Allocator::MUNMAP: {
            return AllocatorKind::RANGED_DEALLOCATOR;
        }
    }
    __builtin_unreachable();
}

bool
isDeallocator(const Allocator& allocator)
{
    switch (allocatorKind(allocator)) {
        case AllocatorKind::SIMPLE_ALLOCATOR:
        case AllocatorKind::RANGED_ALLOCATOR:
            return false;
        case AllocatorKind::SIMPLE_DEALLOCATOR:
        case AllocatorKind::RANGED_DEALLOCATOR:
            return true;
    }
    __builtin_unreachable();
}

#define FOR_EACH_HOOKED_FUNCTION(f) SymbolHook<decltype(&::f)> PYKI_ORIG_NO_NS(f)(#f, &::f);
PYKI_HOOKED_FUNCTIONS
#undef FOR_EACH_HOOKED_FUNCTION

void
ensureAllHooksAreValid()
{
#define FOR_EACH_HOOKED_FUNCTION(f) PYKI_ORIG(f).ensureValidOriginalSymbol();
    PYKI_HOOKED_FUNCTIONS
#undef FOR_EACH_HOOKED_FUNCTION
}

} // namespace pyki::hooks

namespace pyki::intercept {

void* pymalloc_malloc(void* ctx, size_t size) noexcept {
    auto* alloc = (PyMemAllocatorEx*)ctx;

    void* ptr;
    {
        pyki::RecursionGuard guard;
        ptr = alloc->malloc(alloc->ctx, size);
    }
    if (ptr != NULL) {
        NATIVE_STACK_TRACE_ARGS;
        pyki::NativeMemoryProfiling::trackAllocation(ptr, size, &args, hooks::Allocator::PYMALLOC_MALLOC);
    }
    return ptr;
}

void*
pymalloc_realloc(void* ctx, void* ptr, size_t size) noexcept
{
    auto* alloc = (PyMemAllocatorEx*)ctx;
    void* ret;
    {
        pyki::RecursionGuard guard;
        ret = alloc->realloc(alloc->ctx, ptr, size);
    }
    if (ret) {
        if (ptr) {
            pyki::NativeMemoryProfiling::trackDeallocation(ptr, 0, hooks::Allocator::PYMALLOC_FREE);
        }
        NATIVE_STACK_TRACE_ARGS;
        pyki::NativeMemoryProfiling::trackAllocation(ret, size, &args, hooks::Allocator::PYMALLOC_REALLOC);
    }
    return ret;
}

void*
pymalloc_calloc(void* ctx, size_t nelem, size_t size) noexcept
{
    auto* alloc = (PyMemAllocatorEx*)ctx;
    void* ptr;
    {
        pyki::RecursionGuard guard;
        ptr = alloc->calloc(alloc->ctx, nelem, size);
    }
    if (ptr != NULL) {
        NATIVE_STACK_TRACE_ARGS;
        pyki::NativeMemoryProfiling::trackAllocation(ptr, nelem * size, &args, hooks::Allocator::PYMALLOC_CALLOC);
    }

    return ptr;
}

void
pymalloc_free(void* ctx, void* ptr) noexcept
{
    auto* alloc = (PyMemAllocatorEx*)ctx;
    {
        pyki::RecursionGuard guard;
        alloc->free(alloc->ctx, ptr);
    }
    if (ptr) {
        pyki::NativeMemoryProfiling::trackDeallocation(ptr, 0, hooks::Allocator::PYMALLOC_FREE);
    }
}

void*
malloc(size_t size) noexcept
{
    assert(PYKI_ORIG(malloc));

    void* ptr;
    {
        pyki::RecursionGuard guard;
        ptr = PYKI_ORIG(malloc)(size);
    }
    if (ptr) {
        NATIVE_STACK_TRACE_ARGS;
        pyki::NativeMemoryProfiling::trackAllocation(ptr, size, &args, hooks::Allocator::MALLOC);
    }
    return ptr;
}

void
free(void* ptr) noexcept
{
    assert(PYKI_ORIG(free));

    // We need to call our API before we call the real free implementation
    // to make sure that the pointer is not reused in-between.
    if (ptr != nullptr) {
        pyki::NativeMemoryProfiling::trackDeallocation(ptr, 0, hooks::Allocator::FREE);
    }
    {
        pyki::RecursionGuard guard;
        PYKI_ORIG(free)(ptr);
    }
}

void*
realloc(void* ptr, size_t size) noexcept
{
    assert(PYKI_ORIG(realloc));

    void* ret;
    {
        pyki::RecursionGuard guard;
        ret = PYKI_ORIG(realloc)(ptr, size);
    }
    if (ret) {
        if (ptr != nullptr) {
            pyki::NativeMemoryProfiling::trackDeallocation(ptr, 0, hooks::Allocator::FREE);
        }
        NATIVE_STACK_TRACE_ARGS;
        pyki::NativeMemoryProfiling::trackAllocation(ret, size, &args, hooks::Allocator::REALLOC);
    }
    return ret;
}

void*
calloc(size_t num, size_t size) noexcept
{
    assert(PYKI_ORIG(calloc));

    void* ret;
    {
        pyki::RecursionGuard guard;
        ret = PYKI_ORIG(calloc)(num, size);
    }
    if (ret) {
        NATIVE_STACK_TRACE_ARGS;
        pyki::NativeMemoryProfiling::trackAllocation(ret, num * size, &args, hooks::Allocator::CALLOC);
    }
    return ret;
}

void*
mmap(void* addr, size_t length, int prot, int flags, int fd, off_t offset) noexcept
{
    assert(PYKI_ORIG(mmap));
    void* ptr;
    {
        pyki::RecursionGuard guard;
        ptr = PYKI_ORIG(mmap)(addr, length, prot, flags, fd, offset);
    }
    if (ptr != MAP_FAILED) {
        NATIVE_STACK_TRACE_ARGS;
        pyki::NativeMemoryProfiling::trackAllocation(ptr, length, &args, hooks::Allocator::MMAP);
    }
    return ptr;
}

#if defined(__GLIBC__)
void*
mmap64(void* addr, size_t length, int prot, int flags, int fd, off64_t offset) noexcept
{
    assert(PYKI_ORIG(mmap64));
    void* ptr;
    {
        pyki::RecursionGuard guard;
        ptr = PYKI_ORIG(mmap64)(addr, length, prot, flags, fd, offset);
    }
    if (ptr != MAP_FAILED) {
        NATIVE_STACK_TRACE_ARGS;
        pyki::NativeMemoryProfiling::trackAllocation(ptr, length, &args, hooks::Allocator::MMAP);
    }
    return ptr;
}
#endif

int
munmap(void* addr, size_t length) noexcept
{
    assert(PYKI_ORIG(munmap));
    pyki::NativeMemoryProfiling::trackDeallocation(addr, length, hooks::Allocator::MUNMAP);
    {
        pyki::RecursionGuard guard;
        return PYKI_ORIG(munmap)(addr, length);
    }
}

void*
valloc(size_t size) noexcept
{
    assert(PYKI_ORIG(valloc));

    void* ret;
    {
        pyki::RecursionGuard guard;
        ret = PYKI_ORIG(valloc)(size);
    }
    if (ret) {
        NATIVE_STACK_TRACE_ARGS;
        pyki::NativeMemoryProfiling::trackAllocation(ret, size, &args, hooks::Allocator::VALLOC);
    }
    return ret;
}

int
posix_memalign(void** memptr, size_t alignment, size_t size) noexcept
{
    assert(PYKI_ORIG(posix_memalign));

    int ret;
    {
        pyki::RecursionGuard guard;
        ret = PYKI_ORIG(posix_memalign)(memptr, alignment, size);
    }
    if (!ret) {
        NATIVE_STACK_TRACE_ARGS;
        pyki::NativeMemoryProfiling::trackAllocation(*memptr, size, &args, hooks::Allocator::POSIX_MEMALIGN);
    }
    return ret;
}

void*
dlopen(const char* filename, int flag) noexcept
{
    assert(PYKI_ORIG(dlopen));
    void* ret = nullptr;
    {
        pyki::RecursionGuard guard;
#if defined(__GLIBC__)
        // In GLIBC, dlopen() will respect the RPATH/RUNPATH of the caller when searching for the
        // library, which won't work if we intercept dlopen() as we will be the caller. This means that
        // callers that rely on RUNPATH to find their dependencies will fail to load. To work around
        // this, we need to manually find our caller and walk the linker search path to know what we need
        // to dlopen().
        if (filename != nullptr && filename[0] != '\0' && std::strchr(filename, '/') == nullptr) {
            void* const callerAddr = __builtin_extract_return_addr(__builtin_return_address(0));

            Dl_info info;
            if (dladdr(callerAddr, &info)) {
                const char* dlname = info.dli_fname;
                {
                    // Check if we are being called from the main executable
                    Dl_info main_info;
                    void* main_sym = nullptr;
                    void* self_handle = PYKI_ORIG(dlopen)(nullptr, RTLD_LAZY | RTLD_NOLOAD);
                    if (self_handle) {
                        main_sym = dlsym(self_handle, "main");
                        PYKI_ORIG(dlclose)(self_handle);
                    }
                    if (main_sym && dladdr(main_sym, &main_info)
                        && strcmp(main_info.dli_fname, info.dli_fname) == 0)
                    {
                        dlname = nullptr;
                    }
                }

                void* caller = PYKI_ORIG(dlopen)(dlname, RTLD_LAZY | RTLD_NOLOAD);
                if (caller != nullptr) {
                    Dl_serinfo size;
                    if (dlinfo(caller, RTLD_DI_SERINFOSIZE, &size) == 0) {
                        std::vector<char> paths_buf;
                        paths_buf.resize(size.dls_size);
                        auto paths = reinterpret_cast<Dl_serinfo*>(paths_buf.data());
                        *paths = size;
                        if (dlinfo(caller, RTLD_DI_SERINFO, paths) == 0) {
                            for (unsigned int i = 0; i != paths->dls_cnt; ++i) {
                                const char* name = paths->dls_serpath[i].dls_name;
                                if (name == nullptr || name[0] == '\0') {
                                    continue;
                                }
                                std::string dir = name;
                                if (dir.back() != '/') {
                                    dir += '/';
                                }

                                dir += filename;
                                ret = PYKI_ORIG(dlopen)(dir.c_str(), flag);
                                if (ret) {
                                    break;
                                }
                            }
                        }
                    }
                    PYKI_ORIG(dlclose)(caller);
                }
            }
        }
#endif
        // Fallback if we found nothing
        if (ret == nullptr) {
            ret = PYKI_ORIG(dlopen)(filename, flag);
        }
    }
    if (ret) {
        pyki::NativeMemoryProfiling::invalidate_module_cache();
    }
    return ret;
}

int
dlclose(void* handle) noexcept
{
    assert(PYKI_ORIG(dlclose));

    int ret;
    {
        pyki::RecursionGuard guard;
        ret = PYKI_ORIG(dlclose)(handle);
    }
    if (!ret) pyki::NativeMemoryProfiling::invalidate_module_cache();
    return ret;
}

void*
aligned_alloc(size_t alignment, size_t size) noexcept
{
    assert(PYKI_ORIG(aligned_alloc));

    void* ret;
    {
        pyki::RecursionGuard guard;
        ret = PYKI_ORIG(aligned_alloc)(alignment, size);
    }
    if (ret) {
        NATIVE_STACK_TRACE_ARGS;
        pyki::NativeMemoryProfiling::trackAllocation(ret, size, &args, hooks::Allocator::ALIGNED_ALLOC);
    }
    return ret;
}

#if defined(__linux__)

void*
memalign(size_t alignment, size_t size) noexcept
{
    assert(PYKI_ORIG(memalign));

    void* ret;
    {
        pyki::RecursionGuard guard;
        ret = PYKI_ORIG(memalign)(alignment, size);
    }
    if (ret) {
        NATIVE_STACK_TRACE_ARGS;
        pyki::NativeMemoryProfiling::trackAllocation(ret, size, &args, hooks::Allocator::MEMALIGN);
    }
    return ret;
}

#if defined(__GLIBC__)
void*
pvalloc(size_t size) noexcept
{
    assert(PYKI_ORIG(pvalloc));

    void* ret;
    {
        pyki::RecursionGuard guard;
        ret = PYKI_ORIG(pvalloc)(size);
    }
    if (ret) {
        NATIVE_STACK_TRACE_ARGS;
        pyki::NativeMemoryProfiling::trackAllocation(ret, size, &args, hooks::Allocator::PVALLOC);
    }
    return ret;
}
#endif

#endif

}  // namespace pyki::intercept
