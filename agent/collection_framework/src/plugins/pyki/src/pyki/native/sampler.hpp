#pragma once

#include "type.hpp"

namespace pyki {

class Sampler {
   public:
    static u64 generate_random_number();
    static u64 generate_next_sample_point(u64 interval, u64* rnd);
};

}  // namespace pyki