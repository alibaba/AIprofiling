#pragma once

#include <stddef.h>
#include "type.hpp"

namespace pyki {

class OS {
   public:
    static const size_t page_size;
    static const size_t page_mask;

    static void* safe_alloc(size_t size);
    static void safe_free(void* addr, size_t size);

    static u64 current_millis();

    static int thread_id();

    static bool isMusl();

    static int mprotect(void* addr, size_t size, int prot);
};

}  // namespace pyki