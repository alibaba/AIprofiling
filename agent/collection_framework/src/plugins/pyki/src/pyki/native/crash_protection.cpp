#include "crash_protection.hpp"

namespace pyki {

Context CrashProtection::_contexts[CONCURRENCY_LEVEL] = {};

void CrashProtection::check_crash_protection() {
    int thread_id = OS::thread_id();
    int i = get_thread_concurrency_level_index(thread_id);

    for (int j = 0; j < 2; j++) {
        int index = (i + j) % CONCURRENCY_LEVEL;
        Context& ctx = _contexts[index];
        if (ctx.thread_id == thread_id && ctx.buf != nullptr) {
            siglongjmp(*ctx.buf, 1);
        }
    }
}

}  // namespace pyki