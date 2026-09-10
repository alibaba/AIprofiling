#pragma once

#include "pyki.hpp"
#include "stack_trace.hpp"
#include "linear_allocator.hpp"
#include <map>

namespace pyki {

class LongHashTable;

static const u32 EMPTY_STACK_TRACE_ID = 0x7ffffffe;
static const u32 OVERFLOW_STACK_TRACE_ID = 0x7fffffff;

class StackTraceStorage {
   private:
    LinearAllocator _allocator;
    LinearAllocator _string_allocator;
    LongHashTable* _current_table;
    u64 _overflow;

    u64 calc_hash(int nof_python_frames, PythonFrame* python_frames,
                  int nof_native_frames, NativeFrame* native_frames);
    StackTrace* store_stack_trace(int nof_python_frames,
                                  PythonFrame* python_frames,
                                  int nof_native_frames,
                                  NativeFrame* native_frames);
    StackTrace* find_stack_trace(LongHashTable* table, u64 hash);

   public:
    StackTraceStorage();
    ~StackTraceStorage();

    void clear();

    u32 put(int nof_python_frames, PythonFrame* python_frames,
            int nof_native_frames, NativeFrame* native_frames);

    void collect_stack_traces(std::map<u32, StackTrace*>& map);

    static StackTraceStorage* instance();
};

}  // namespace pyki