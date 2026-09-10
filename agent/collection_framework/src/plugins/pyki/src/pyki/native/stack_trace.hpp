#pragma once
#include "pyki.hpp"

namespace pyki {

struct PythonFrame {
    void* code;
    void* filename_address;
    void* name_address;
    size_t meta;
};

static_assert(sizeof(PythonFrame) % 8 == 0,
              "PythonFrame must be 8-byte aligned");

struct NativeFrame {
    const char* library;
    const char* name;
};

static_assert(sizeof(NativeFrame) % 8 == 0,
              "NativeFrame must be 8-byte aligned");

struct StackTrace {
    u32 nof_python_frames;
    u32 nof_native_frames;
    const char* string;
};

static_assert(sizeof(StackTrace) % 8 == 0, "StackTrace must be 8-byte aligned");

void init_stack_trace_buffer(int max_nframes);

struct StackTraceArgs {
    void* ucontext;

    int max_nframes;
    bool native;
    // int skip_native_frames;

    const void* pc;
    const void* sp;
    const void* fp;
};

u32 get_stack_trace(int concurrency_level_index, StackTraceArgs& args);

class Meta {
   public:
    Meta(size_t meta)
        : _meta(meta) {}

    u32 lineno() { return _meta & 0xFFFF'FFFF; }

    u16 filename_size() {
        return ((_meta >> 46) & 0x3FF) << (filename_kind() >> 1);
    }
    u8 filename_kind() { return (_meta >> 56) & 0x07; }
    bool filename_is_ascii() { return (_meta >> 59 & 0x01) == 1; }

    u8 filename_string_kind() {
        return to_string_kind(filename_kind(), filename_is_ascii());
    }

    u16 name_size() { return ((_meta >> 32) & 0x3FF) << (name_kind() >> 1); }
    u8 name_kind() { return (_meta >> 42) & 0x07; }
    bool name_is_ascii() { return (_meta >> 45 & 0x01) == 1; }

    u8 name_string_kind() {
        return to_string_kind(name_kind(), name_is_ascii());
    }

    bool is_entry() { return (_meta & 0x8000'0000'0000'0000) != 0; }

   private:
    size_t _meta;

    u8 to_string_kind(u8 kind, bool is_ascii) {
        static u8 map[3][2] = {{1, 0}, {2, 2}, {3, 3}};
        return map[kind >> 1][is_ascii ? 1 : 0];
    }
};
}  // namespace pyki