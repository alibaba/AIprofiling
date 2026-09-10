#include "os.hpp"
#include <chrono>
#include <errno.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>

namespace pyki {

const size_t OS::page_size = sysconf(_SC_PAGESIZE);
const size_t OS::page_mask = OS::page_size - 1;

void* OS::safe_alloc(size_t size) {
    // Naked syscall can be used inside a signal handler.
    // Also, we don't want to catch our own calls when profiling mmap.
    intptr_t result = syscall(__NR_mmap, NULL, size, PROT_READ | PROT_WRITE,
                              MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (result < 0 && result > -4096) {
        return NULL;
    }
    return (void*)result;
}

void OS::safe_free(void* addr, size_t size) { syscall(__NR_munmap, addr, size); }

u64 OS::current_millis() {
    using namespace std::chrono;
    return duration_cast<milliseconds>(system_clock::now().time_since_epoch())
        .count();
}

int OS::thread_id() {
    return syscall(__NR_gettid);
}

// _CS_GNU_LIBC_VERSION is not defined on musl
const static bool musl = confstr(_CS_GNU_LIBC_VERSION, NULL, 0) == 0 && errno != 0;

bool OS::isMusl() {
    return musl;
}

int OS::mprotect(void* addr, size_t size, int prot) {
    return ::mprotect(addr, size, prot);
}

}  // namespace pyki