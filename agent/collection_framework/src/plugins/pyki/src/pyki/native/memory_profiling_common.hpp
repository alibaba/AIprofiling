#pragma once
#include "Python.h"
#include "arch.hpp"

const int DEFAULT_SAMPLE_INTERVAL = 512 * 1024;
const int DEFAULT_MAX_NFRAMES = 64;


namespace pyki {
class SampleMeta {
   public:
    SampleMeta() : sample_interval(0), rnd(0), sample_point(0), sample_remaining(0) {}
    
    u64 sample_interval;
    u64 rnd;
    u64 sample_point;
    u64 sample_remaining;
};

void delete_sample_meta(void* ctx);
} // namespace pyki
