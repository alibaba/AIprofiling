#include "recorder.hpp"

#include "buffer.hpp"
#include "demangle.h"
#include "dictionary.hpp"
#include "lock.hpp"
#include "os.hpp"
#include "stack_trace_storage.hpp"

#include <unistd.h>
#include <time.h>

#include <time.h>

namespace pyki {

#define UNKNOWN_ID 0
#define UNKNOWN "<unknown>"

#define NATIVE_ID 1
#define NATIVE "<native>"

static struct ConstantString {
    u32 id;
    const char* string;
    size_t length;
} constant_strings[] = {
    {UNKNOWN_ID, UNKNOWN, strlen(UNKNOWN)},
    {NATIVE_ID, NATIVE, strlen(NATIVE)},
};

static size_t constant_strings_size = std::size(constant_strings);

Recorder::Recorder()
    : _fd(-1) {
    for (int i = 0; i < CONCURRENCY_LEVEL; i++) {
        _buffer[i] = new Buffer();
    }
    _strings = new Dictionary();
}

void Recorder::flush(Buffer* buf) {
    if (buf->offset() == 0) {
        return;
    }
    if (_fd >= 0) {
        write(_fd, buf->data(), buf->offset());
    }
    buf->reset();
}

void Recorder::flush_if_needed(Buffer* buf, int required_size) {
    if (buf->offset() + required_size > BUFFER_SIZE) {
        flush(buf);
    }
}

Recorder* Recorder::instance() {
    static Recorder* _instance = new Recorder();
    return _instance;
}

void Recorder::set_fd(int fd) {
    _fd = fd;
    write_header();
}

void Recorder::record_malloc_event(int level_index, u32 thread_id,
                                   u32 stack_trace_id, u64 size) {
    Buffer* buf = _buffer[level_index];

    flush_if_needed(buf, 1 + 5 + 5 + 9);

    buf->put_8(ALLOCATION_EVENT);
    buf->put_var32(thread_id);
    buf->put_var32(stack_trace_id);
    buf->put_var64(size);
}

void Recorder::record_native_memory_event(int level_index, u32 thread_id, u32 stack_trace_id, u64 size, void* ptr, hooks::Allocator func) {
    Buffer* buf = _buffer[level_index];

    flush_if_needed(buf, 1 + 5 + 5 + 9 + 9 + 1);

    buf->put_8(MALLOC_EVENT);
    buf->put_var32(thread_id);
    buf->put_var32(stack_trace_id);
    buf->put_var64(reinterpret_cast<uintptr_t>(ptr));
    buf->put_var64(size);
    buf->put_8(static_cast<u8>(func));
}

void Recorder::record_native_malloc_event(int level_index, u32 thread_id, u32 stack_trace_id, u64 size, void* ptr, hooks::Allocator func) {
    record_native_memory_event(level_index, thread_id, stack_trace_id, size, ptr, func);
}

void Recorder::record_native_free_event(int level_index, u32 thread_id, u64 size, void* ptr, hooks::Allocator func) {
    record_native_memory_event(level_index, thread_id, 0, size, ptr, func);
}

void Recorder::record_cpu_event(int level_index, u32 thread_id,
                                u32 stack_trace_id) {
    Buffer* buf = _buffer[level_index];

    flush_if_needed(buf, 1 + 5 + 5);

    buf->put_8(CPU_EVENT);
    buf->put_var32(thread_id);
    buf->put_var32(stack_trace_id);
}

void Recorder::flush_all_buffers() {
    for (int i = 0; i < CONCURRENCY_LEVEL; i++) {
        flush(_buffer[i]);
    }
}

void Recorder::write_header() {
    Buffer* buf = _buffer[0];
    buf->put("PYKI-PROFILE", 12);       // magic, 12 bytes
    buf->put_32(1);                     // version, 4 byte
    buf->put_64(0);                     // size, 8 bytes, need patch
    buf->put_64(OS::current_millis());  // start time, 8 bytes,
    buf->put_64(0);                     // end time, 8 bytes, need patch
    buf->put_64(0);  // constant pool offset, 8 bytes, need patch
    flush(buf);
}

void Recorder::patch_header(off_t constant_pool_offset) {
#define SIZE_OFFSET 16
#define ENDTIME_OFFSET 32
#define CONSTANT_POOL_OFFSET 40
    Buffer* buf = _buffer[0];
    const char* pos = buf->data();
    off_t size = lseek(_fd, 0, SEEK_END);
    buf->put_64(size);
    pwrite(_fd, pos, 8, SIZE_OFFSET);  // size
    pos += 8;
    buf->put_64(OS::current_millis());
    pwrite(_fd, pos, 8, ENDTIME_OFFSET);  // end time
    pos += 8;
    buf->put_64(constant_pool_offset);
    pwrite(_fd, pos, 8, CONSTANT_POOL_OFFSET);  // constant pool offset
    buf->reset();
}

void Recorder::write_stack_traces() {
    Buffer* buf = _buffer[0];

    std::map<u32, StackTrace*> stack_traces;
    StackTraceStorage::instance()->collect_stack_traces(stack_traces);

    flush_if_needed(buf, 1 + 5);
    buf->put_8(TRACEBACK);
    buf->put_var32(stack_traces.size());

    for (std::map<u32, StackTrace*>::const_iterator it = stack_traces.begin();
         it != stack_traces.end(); ++it) {
        StackTrace* stack_trace = it->second;
        int nof_python_frames = stack_trace->nof_python_frames;
        int nof_native_frames = stack_trace->nof_native_frames;
        int nframes = nof_python_frames + nof_native_frames;
        flush_if_needed(buf, 5 + 5 + nframes * 15);

        u32 stack_trace_id = it->first;
        buf->put_var32(stack_trace_id);
        buf->put_var32(nframes);

        const char* s = stack_trace->string;
        const PythonFrame* python_frames =
            (PythonFrame*)((u8*)stack_trace + sizeof(StackTrace));

        int ni = 0;
        int pi = 0;

        bool last_python_frame_is_entry = false;

        auto write_python_frame = [this, &python_frames, &s, &buf, &pi,
                                   &last_python_frame_is_entry]() {
            Meta meta = Meta(python_frames->meta);
            size_t length = meta.filename_size();
            u32 filename_id =
                _strings->lookup(s, length, meta.filename_string_kind());
            s += length;

            length = meta.name_size();
            u32 name_id = _strings->lookup(s, length, meta.name_string_kind());
            if (meta.is_entry() || strncmp("<shim>", s, 6) == 0) {
                last_python_frame_is_entry = true;
            }
            s += length;

            buf->put_var32(filename_id);
            buf->put_var32(name_id);
            buf->put_var32(meta.lineno());
            python_frames++;
            pi++;
        };

        const NativeFrame* native_frames =
            (NativeFrame*)((u8*)stack_trace + sizeof(StackTrace) +
                           stack_trace->nof_python_frames *
                               sizeof(PythonFrame));

        for (; ni < nof_native_frames; ni++) {
            const char* library = native_frames->library;
            const char* name = native_frames->name;
            u32 library_id =
                library ? _strings->lookup(library,
                                           std::min(strlen(library),
                                                    (size_t)MAX_STRING_LENGTH),
                                           0)
                        : UNKNOWN_ID;
            u32 name_id = UNKNOWN_ID;
            if (name) {
                if (Demangle::needsDemangling(name)) {
                    const char* demangled_name =
                        Demangle::demangle(name, false);
                    if (demangled_name != nullptr) {
                        name_id = _strings->lookup(
                            demangled_name,
                            std::min(strlen(demangled_name),
                                     (size_t)MAX_STRING_LENGTH),
                            0);
                        free((void*)demangled_name);
                    }
                    else {
                        name_id = _strings->lookup(
                            name,
                            std::min(strlen(name), (size_t)MAX_STRING_LENGTH),
                            0);
                    }
                }
                else {
                    name_id = _strings->lookup(
                        name, std::min(strlen(name), (size_t)MAX_STRING_LENGTH),
                        0);
                }

                if (strstr(name, "PyEval_EvalFrameDefault") != nullptr) {
                    while (pi < nof_python_frames) {
                        write_python_frame();
                        if (last_python_frame_is_entry) {
                            break;
                        }
                    }
                }
            }

            buf->put_var32(library_id);
            buf->put_var32(name_id);
            buf->put_var32(0);
            native_frames++;
        }

        for (; pi < nof_python_frames;) {
            write_python_frame();
        }
    }

    flush(buf);
}

void Recorder::write_strings() {
    Buffer* buf = _buffer[0];

    std::map<u32, StringWithLength> strings;
    _strings->collect(strings);

    flush_if_needed(buf, 1 + 5);
    buf->put_8(STRING);
    buf->put_var32(strings.size() + constant_strings_size);

    for (size_t i = 0; i < constant_strings_size; i++) {
        const ConstantString& cs = constant_strings[i];
        flush_if_needed(buf, 5 + 1 + 5 + cs.length);
        buf->put_var32(cs.id);
        buf->put_8(0);
        buf->put_string(cs.string, cs.length);
    }

    for (std::map<u32, StringWithLength>::const_iterator it = strings.begin();
         it != strings.end(); ++it) {
        const u32 id = it->first;
        const StringWithLength& swl = it->second;

        flush_if_needed(buf, 5 + 1 + 5 + swl.length);
        buf->put_var32(id);
        buf->put_8(swl.kind);
        buf->put_string(swl.string, swl.length);
    }

    flush(buf);
}

void Recorder::rotate_to(int fd) {
    ThreadCritical tc;
    tc.lock_all();

    flush_all_buffers();

    off_t cp_off = lseek(_fd, 0, SEEK_CUR);
    write_stack_traces();
    StackTraceStorage::instance()->clear();
    write_strings();
    _strings->clear();

    patch_header(cp_off);

    fsync(_fd);
    _fd = fd;
    if (fd > 0) {
        write_header();
    }
}

}  // namespace pyki
