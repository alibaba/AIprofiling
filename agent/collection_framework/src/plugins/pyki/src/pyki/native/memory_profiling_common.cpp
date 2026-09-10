#include "memory_profiling_common.hpp"

namespace pyki {
void delete_sample_meta(void* ctx) {
    SampleMeta* sample_meta = (SampleMeta*)ctx;
    if (sample_meta != nullptr) {
        delete sample_meta;
    }
}
} // namespace pyki
