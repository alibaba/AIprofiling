#pragma once

#include "pyki.hpp"
#include <sys/types.h>

namespace pyki {

static_assert(sizeof(pid_t) == 4, "sizeof(pid_t) should be 4");
static_assert((pid_t)(-1) < 0, "pid_t should be signed");

const u32 BYTE_PER_BLOCK = 1 << 16;  // 64k
const u32 PID_PER_BYTE = 8;
const u32 PID_PER_BLOCK = PID_PER_BYTE * BYTE_PER_BLOCK;

const u32 NUMBER_OF_PID = 1 << 31;
const u32 NUMBER_OF_BLOCK = NUMBER_OF_PID / PID_PER_BLOCK;

class ThreadSet {
   private:
    u8* _bit_map[NUMBER_OF_BLOCK];
};

}  // namespace pyki