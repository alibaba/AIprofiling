#pragma once

#include "lock.hpp"
#include <setjmp.h>

namespace pyki {

struct Context {
    sigjmp_buf* buf;
    int thread_id;
};

class CrashProtection {
   public:
    template <typename F>
    static bool call(ThreadCritical& thread_critical, F func) {
        int index = thread_critical.get_index();

        sigjmp_buf buf;
        if (sigsetjmp(buf, 1) != 0) {
            _contexts[index] = {};
            return false;
        };

        _contexts[index] = {
            .buf = &buf,
            .thread_id = thread_critical.get_thread_id(),
        };

        func();

        _contexts[index] = {};
        return true;
    }

    static void check_crash_protection();

   private:
    static Context _contexts[CONCURRENCY_LEVEL];
};

}  // namespace pyki