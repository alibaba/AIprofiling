#pragma once

#include "pyki.hpp"
#include "hooks.h"

namespace pyki {

enum { // If this enum is modified, then profile_converter should also be modified accordingly.
    STRING = 1,
    TRACEBACK = 2,
    CPU_EVENT = 32,
    ALLOCATION_EVENT = 33,
    MALLOC_EVENT = 34,
};

class Buffer;

class Dictionary;

class Recorder {
   private:
    Buffer* _buffer[CONCURRENCY_LEVEL];
    u64 _start_time;
    int _fd;

    Dictionary* _strings;

    Recorder();

    void flush(Buffer* buf);

    void flush_if_needed(Buffer* buf, int required_size);

    void flush_all_buffers();

    void write_header();

    void patch_header(off_t constant_pool_offset);

    void write_stack_traces();

    void write_strings();

    void record_native_memory_event(int level_index, u32 thread_id, u32 stack_trace_id, u64 size, void* ptr, hooks::Allocator func);

   public:
    static Recorder* instance();

    void set_fd(int fd);

    void record_malloc_event(int level_index, u32 thread_id, u32 stack_trace_id,
                             u64 size);

    void record_native_malloc_event(int level_index, u32 thread_id, u32 stack_trace_id, u64 size, void* ptr, hooks::Allocator func);

    void record_native_free_event(int level_index, u32 thread_id, u64 size, void* ptr, hooks::Allocator func);

    void record_cpu_event(int level_index, u32 thread_id, u32 stack_trace_id);

    void rotate_to(int fd);
};

}  // namespace pyki