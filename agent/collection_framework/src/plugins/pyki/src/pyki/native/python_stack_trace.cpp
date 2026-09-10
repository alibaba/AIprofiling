#include "python_stack_trace.hpp"
#include "cpython_structs.hpp"

namespace pyki {

namespace {

static inline bool is_aligned(void* ptr, size_t alignment) {
    return (reinterpret_cast<uintptr_t>(ptr) & (alignment - 1)) == 0;
}

static inline bool is_valid_pointer(void* ptr) {
    return ptr != nullptr && is_aligned(ptr, sizeof(void*));
}

template <int minor>
struct StackTracer;

template <int minor>
struct StackTracerBase {
    u32 get_stack_trace(PyThreadState* ts, PythonFrame* frames,
                        int max_nframes) {
        void* frame = get_frame(ts);

        u32 nframes = 0;

        if (is_valid_pointer(frame)) {
            while (true) {
                if (!is_valid_pointer(frame)) {
                    break;
                }

                void* code = get_frame_code(frame);
                if (!is_valid_pointer(code)) {
                    break;
                }

                void* filename = get_code_filename(code);
                size_t filename_size = get_unicode_size(filename);
                u8 filename_kind = get_unicode_kind(filename);
                bool filename_is_ascii = unicode_is_ascii(filename);

                size_t filename_meta = (filename_size & 0x3FF) |
                                       (((size_t)filename_kind & 0x7) << 10) |
                                       (((size_t)filename_is_ascii) << 13);

                void* name = get_code_name(code);
                size_t name_size = get_unicode_size(name);
                u8 name_kind = get_unicode_kind(name);
                bool name_is_ascii = unicode_is_ascii(name);

                size_t name_meta = (name_size & 0x3FF) |
                                   (((size_t)name_kind & 0x7) << 10) |
                                   (((size_t)name_is_ascii) << 13);

                int lasti = get_frame_lasti(frame);
                void* line_table = get_code_line_table(code);
                size_t line_table_size = get_bytes_size(line_table);
                void* line_table_value = get_bytes_value(line_table);

                u32 lineno = get_lineno(lasti, get_code_first_lineno(code),
                                        (u8*)line_table_value, line_table_size);

                size_t meta = filename_meta << 46 | name_meta << 32 |
                              ((size_t)lineno & 0xFFFFFFFF);

                if (frame_is_entry(frame)) {
                    meta |= (size_t)1 << 63;
                }

                frames[nframes++] =
                    PythonFrame{code, get_unicode_address(filename),
                                get_unicode_address(name), meta};

                if (nframes > (u32)max_nframes) {
                    break;
                }
                frame = get_frame_back(frame);
            }
        }

        return nframes;
    }

    void* get_frame(void* thread) {
        if constexpr (minor == 6 || minor == 9 || minor == 10) {
            return (void*)*(
                uintptr_t*)((u8*)thread +
                            StackTracer<minor>::_pythread_frame_offset);
        }
        else if constexpr (minor == 11 || minor == 12) {
            void* cframe = (void*)*(
                uintptr_t*)((u8*)thread +
                            StackTracer<minor>::_pythread_cframe_offset);
            return is_valid_pointer(cframe)
                       ? (void*)*(
                             uintptr_t*)((u8*)cframe +
                                         StackTracer<minor>::
                                             _pycframe_current_frame_offset)
                       : nullptr;
        }
    }

    void* get_frame_code(void* frame) {
        return (void*)*(uintptr_t*)((u8*)frame +
                                    StackTracer<minor>::_pyframe_code_offset);
    }

    void* get_frame_back(void* frame) {
        return (void*)*(uintptr_t*)((u8*)frame +
                                    StackTracer<minor>::_pyframe_back_offset);
    }

    int get_frame_lasti(void* frame) {
        if constexpr (minor == 6 || minor == 9 || minor == 10) {
            return *(int*)((u8*)frame +
                           StackTracer<minor>::_pyframe_lasti_offset);
        }
        else if constexpr (minor == 11 || minor == 12) {
            u8* code = (u8*)get_frame_code(frame);
            u8* prev_inst = (u8*)*(
                uintptr_t*)((u8*)frame +
                            StackTracer<minor>::_pyframe_prev_inst_offset);
            return (int)(prev_inst - code);
        }
    }

    bool frame_is_entry(void* frame) {
        if constexpr (minor == 6 || minor == 9 || minor == 10) {
            return true;
        }
        else if constexpr (minor == 11) {
            return *(bool*)((u8*)frame +
                            StackTracer<minor>::_pyframe_is_entry_offset);
        }
        else if constexpr (minor == 12) {
            char owner = *(char*)((u8*)frame +
                                  StackTracer<minor>::_pyframe_owner_offset);
            return owner == (char)3;
        }
    }

    void* get_code_filename(void* code) {
        return (void*)*(
            uintptr_t*)((u8*)code +
                        StackTracer<minor>::_pycode_filename_offset);
    }

    void* get_code_name(void* code) {
        return (void*)*(uintptr_t*)((u8*)code +
                                    StackTracer<minor>::_pycode_name_offset);
    }

    void* get_code_line_table(void* code) {
        return (void*)*(
            uintptr_t*)((u8*)code +
                        StackTracer<minor>::_pycode_line_table_offset);
    }

    int get_code_first_lineno(void* code) {
        return *(int*)((u8*)code +
                       StackTracer<minor>::_pycode_first_lineno_offset);
    }

    bool unicode_is_ascii(void* unicode) {
        int offset = 6;
        int state =
            *(int*)((u8*)unicode + StackTracer<minor>::_pyunicode_base_offset +
                    StackTracer<minor>::_pycompactunicode_base_offset +
                    StackTracer<minor>::_pyascii_state_offset);

        return ((state >> offset) & 0x01) != 0;
    }

    u8 get_unicode_kind(void* unicode) {
        int offset = 2;
        int width = 3;
        int state =
            *(int*)((u8*)unicode + StackTracer<minor>::_pyunicode_base_offset +
                    StackTracer<minor>::_pycompactunicode_base_offset +
                    StackTracer<minor>::_pyascii_state_offset);
        return (state >> offset) & ((1 << width) - 1);
    }

    size_t get_unicode_size(void* unicode) {
        return *(size_t*)((u8*)unicode +
                          StackTracer<minor>::_pyunicode_base_offset +
                          StackTracer<minor>::_pycompactunicode_base_offset +
                          StackTracer<minor>::_pyascii_length_offset);
    }

    bool unicode_is_compact(void* unicode) {
        int offset = 5;
        int state =
            *(int*)((u8*)unicode + StackTracer<minor>::_pyunicode_base_offset +
                    StackTracer<minor>::_pycompactunicode_base_offset +
                    StackTracer<minor>::_pyascii_state_offset);

        return ((state >> offset) & 0x01) != 0;
    }

    void* get_unicode_address(void* unicode) {
        if (!unicode_is_compact(unicode)) {
            return (void*)*(
                uintptr_t*)((u8*)unicode +
                            StackTracer<minor>::_pyunicode_data_any_offset);
        }

        if (unicode_is_ascii(unicode)) {
            return (void*)((u8*)unicode + StackTracer<minor>::_pyascii_size);
        }

        return (void*)((u8*)unicode +
                       StackTracer<minor>::_pycompactunicode_size);
    }

    int get_lineno(int lasti, int first_lineno, u8* table, size_t table_size) {
        if constexpr (minor == 6 || minor == 9) {
            int lineno = first_lineno;
            int bc_addr = 0;
            size_t index = 0;
            while (index + 1 < table_size) {
                bc_addr += (int)table[index];
                if (bc_addr > lasti) {
                    break;
                }
                int inc = (int)table[index + 1];
                if (inc >= 0x80) {
                    inc -= 0x100;
                }
                lineno += inc;
                index += 2;
            }
            return lineno;
        }
        else if constexpr (minor == 10) {
            lasti = lasti << 1;
            int lineno = first_lineno;
            int bc_addr = 0;
            size_t index = 0;
            while (index + 1 < table_size) {
                u8 delta = table[index];
                char line_delta = (char)table[index + 1];
                index += 2;
                if (line_delta == -128) {
                    continue;
                }
                lineno += (int)line_delta;
                bc_addr += (int)delta;
                if (bc_addr > lasti) {
                    break;
                }
            }
            return lineno;
        }
        else if constexpr (minor == 11 || minor == 12) {
            lasti =
                lasti - (int)StackTracer<minor>::_pycode_code_adaptive_offset;
            int lineno = first_lineno;
            int bc_addr = 0;
            size_t index = 0;
            while (true) {
                if (index >= table_size) {
                    break;
                }
                u8 byte = table[index];
                index += 1;

                int delta = (int)(byte & 7) + 1;
                bc_addr += delta * 2;
                u8 code = (byte >> 3) & 15;
                int line_delta;
                if (code == 15) {
                    line_delta = 0;
                }
                else if (code == 14) {
                    bool valid;
                    ptrdiff_t value =
                        read_signed_varint(index, table, table_size, valid);
                    line_delta = valid ? (int)value : 0;
                    read_varint(index, table, table_size, valid);  // end line
                    read_varint(index, table, table_size,
                                valid);  // start column
                    read_varint(index, table, table_size, valid);  // end column
                }
                else if (code == 13) {
                    bool valid;
                    ptrdiff_t value =
                        read_signed_varint(index, table, table_size, valid);
                    line_delta = valid ? (int)value : 0;
                }
                else if (code >= 10 && code <= 12) {
                    index += 2;
                    line_delta = (int)(code - 10);
                }
                else {
                    index += 1;
                    line_delta = 0;
                }
                lineno += line_delta;
                if (bc_addr >= lasti) {
                    break;
                }
            }
            return lineno;
        }
    }

    size_t read_varint(size_t& index, u8* table, size_t table_size,
                       bool& valid) {
        valid = false;
        if (index >= table_size) {
            return 0;
        }
        size_t result = 0;
        u8 byte = table[index];
        int shift = 0;
        index += 1;
        result = (size_t)(byte & 63);

        while ((result & 64) != 0) {
            if (index >= table_size) {
                return 0;
            }
            byte = table[index];
            index += 1;
            shift += 6;
            result += ((size_t)(byte & 63) << shift);
        }
        valid = true;
        return result;
    }
    ptrdiff_t read_signed_varint(size_t& index, u8* table, size_t table_size,
                                 bool& valid) {
        size_t unsigned_value = read_varint(index, table, table_size, valid);
        if (!valid) {
            return 0;
        }

        if ((unsigned_value & 1) != 0) {
            return -((ptrdiff_t)(unsigned_value >> 1));
        }
        else {
            return (ptrdiff_t)(unsigned_value >> 1);
        }
    }

    size_t get_bytes_size(void* bytes) {
        return *(size_t*)((u8*)bytes +
                          StackTracer<minor>::_pybytes_base_size_offset);
    }

    void* get_bytes_value(void* bytes) {
        return (void*)((u8*)bytes + StackTracer<minor>::_pybytes_value_offset);
    }
};

template <>
struct StackTracer<6> : StackTracerBase<6> {
    static constexpr size_t _pythread_frame_offset =
        cpython_v3_6::PyThreadState_frame_offset;

    static constexpr size_t _pyframe_code_offset =
        cpython_v3_6::PyFrameObject_f_code_offset;

    static constexpr size_t _pyframe_back_offset =
        cpython_v3_6::PyFrameObject_f_back_offset;

    static constexpr size_t _pyframe_lasti_offset =
        cpython_v3_6::PyFrameObject_f_lasti_offset;

    static constexpr size_t _pycode_filename_offset =
        cpython_v3_6::PyCodeObject_co_filename_offset;

    static constexpr size_t _pycode_name_offset =
        cpython_v3_6::PyCodeObject_co_name_offset;

    static constexpr size_t _pycode_line_table_offset =
        cpython_v3_6::PyCodeObject_co_lnotab_offset;

    static constexpr size_t _pycode_first_lineno_offset =
        cpython_v3_6::PyCodeObject_co_firstlineno_offset;

    static constexpr size_t _pyunicode_size =
        cpython_v3_6::PyUnicodeObject_size;

    static constexpr size_t _pyunicode_base_offset =
        cpython_v3_6::PyUnicodeObject_base_offset;

    static constexpr size_t _pyunicode_data_any_offset =
        cpython_v3_6::PyUnicodeObject_data_any_offset;

    static constexpr size_t _pycompactunicode_size =
        cpython_v3_6::PyCompactUnicodeObject_size;

    static constexpr size_t _pycompactunicode_base_offset =
        cpython_v3_6::PyCompactUnicodeObject_base_offset;

    static constexpr size_t _pyascii_size = cpython_v3_6::PyASCIIObject_size;

    static constexpr size_t _pyascii_length_offset =
        cpython_v3_6::PyASCIIObject_length_offset;

    static constexpr size_t _pyascii_state_offset =
        cpython_v3_6::PyASCIIObject_state_offset;

    static constexpr size_t _pybytes_base_size_offset =
        cpython_v3_6::PyBytesObject_ob_base_ob_size_offset;

    static constexpr size_t _pybytes_value_offset =
        cpython_v3_6::PyBytesObject_ob_sval_offset;
};

template <>
struct StackTracer<9> : StackTracerBase<9> {
    static constexpr size_t _pythread_frame_offset =
        cpython_v3_9::PyThreadState_frame_offset;

    static constexpr size_t _pyframe_code_offset =
        cpython_v3_9::PyFrameObject_f_code_offset;

    static constexpr size_t _pyframe_back_offset =
        cpython_v3_9::PyFrameObject_f_back_offset;

    static constexpr size_t _pyframe_lasti_offset =
        cpython_v3_9::PyFrameObject_f_lasti_offset;

    static constexpr size_t _pycode_filename_offset =
        cpython_v3_9::PyCodeObject_co_filename_offset;

    static constexpr size_t _pycode_name_offset =
        cpython_v3_9::PyCodeObject_co_name_offset;

    static constexpr size_t _pycode_line_table_offset =
        cpython_v3_9::PyCodeObject_co_lnotab_offset;

    static constexpr size_t _pycode_first_lineno_offset =
        cpython_v3_9::PyCodeObject_co_firstlineno_offset;

    static constexpr size_t _pyunicode_size =
        cpython_v3_9::PyUnicodeObject_size;

    static constexpr size_t _pyunicode_base_offset =
        cpython_v3_9::PyUnicodeObject_base_offset;

    static constexpr size_t _pyunicode_data_any_offset =
        cpython_v3_9::PyUnicodeObject_data_any_offset;

    static constexpr size_t _pycompactunicode_size =
        cpython_v3_9::PyCompactUnicodeObject_size;

    static constexpr size_t _pycompactunicode_base_offset =
        cpython_v3_9::PyCompactUnicodeObject_base_offset;

    static constexpr size_t _pyascii_size = cpython_v3_9::PyASCIIObject_size;

    static constexpr size_t _pyascii_length_offset =
        cpython_v3_9::PyASCIIObject_length_offset;

    static constexpr size_t _pyascii_state_offset =
        cpython_v3_9::PyASCIIObject_state_offset;

    static constexpr size_t _pybytes_base_size_offset =
        cpython_v3_9::PyBytesObject_ob_base_ob_size_offset;

    static constexpr size_t _pybytes_value_offset =
        cpython_v3_9::PyBytesObject_ob_sval_offset;
};

template <>
struct StackTracer<10> : StackTracerBase<10> {
    static constexpr size_t _pythread_frame_offset =
        cpython_v3_10::PyThreadState_frame_offset;

    static constexpr size_t _pyframe_code_offset =
        cpython_v3_10::PyFrameObject_f_code_offset;

    static constexpr size_t _pyframe_back_offset =
        cpython_v3_10::PyFrameObject_f_back_offset;

    static constexpr size_t _pyframe_lasti_offset =
        cpython_v3_10::PyFrameObject_f_lasti_offset;

    static constexpr size_t _pycode_filename_offset =
        cpython_v3_10::PyCodeObject_co_filename_offset;

    static constexpr size_t _pycode_name_offset =
        cpython_v3_10::PyCodeObject_co_name_offset;

    static constexpr size_t _pycode_line_table_offset =
        cpython_v3_10::PyCodeObject_co_linetable_offset;

    static constexpr size_t _pycode_first_lineno_offset =
        cpython_v3_10::PyCodeObject_co_firstlineno_offset;

    static constexpr size_t _pyunicode_size =
        cpython_v3_10::PyUnicodeObject_size;

    static constexpr size_t _pyunicode_base_offset =
        cpython_v3_10::PyUnicodeObject_base_offset;

    static constexpr size_t _pyunicode_data_any_offset =
        cpython_v3_10::PyUnicodeObject_data_any_offset;

    static constexpr size_t _pycompactunicode_size =
        cpython_v3_10::PyCompactUnicodeObject_size;

    static constexpr size_t _pycompactunicode_base_offset =
        cpython_v3_10::PyCompactUnicodeObject_base_offset;

    static constexpr size_t _pyascii_size = cpython_v3_10::PyASCIIObject_size;

    static constexpr size_t _pyascii_length_offset =
        cpython_v3_10::PyASCIIObject_length_offset;

    static constexpr size_t _pyascii_state_offset =
        cpython_v3_10::PyASCIIObject_state_offset;

    static constexpr size_t _pybytes_base_size_offset =
        cpython_v3_10::PyBytesObject_ob_base_ob_size_offset;

    static constexpr size_t _pybytes_value_offset =
        cpython_v3_10::PyBytesObject_ob_sval_offset;
};

template <>
struct StackTracer<11> : StackTracerBase<11> {
    static constexpr size_t _pythread_cframe_offset =
        cpython_v3_11::PyThreadState_cframe_offset;

    static constexpr size_t _pycframe_current_frame_offset =
        cpython_v3_11::PyCFrame_current_frame_offset;

    static constexpr size_t _pyframe_code_offset =
        cpython_v3_11::PyInterpreterFrame_f_code_offset;

    static constexpr size_t _pyframe_back_offset =
        cpython_v3_11::PyInterpreterFrame_previous_offset;

    static constexpr size_t _pyframe_prev_inst_offset =
        cpython_v3_11::PyInterpreterFrame_prev_instr_offset;

    static constexpr size_t _pyframe_is_entry_offset =
        cpython_v3_11::PyInterpreterFrame_is_entry_offset;

    static constexpr size_t _pycode_filename_offset =
        cpython_v3_11::PyCodeObject_co_filename_offset;

    static constexpr size_t _pycode_name_offset =
        cpython_v3_11::PyCodeObject_co_name_offset;

    static constexpr size_t _pycode_line_table_offset =
        cpython_v3_11::PyCodeObject_co_linetable_offset;

    static constexpr size_t _pycode_first_lineno_offset =
        cpython_v3_11::PyCodeObject_co_firstlineno_offset;

    static constexpr size_t _pycode_code_adaptive_offset =
        cpython_v3_11::PyCodeObject_co_code_adaptive_offset;

    static constexpr size_t _pyunicode_size =
        cpython_v3_11::PyUnicodeObject_size;

    static constexpr size_t _pyunicode_base_offset =
        cpython_v3_11::PyUnicodeObject_base_offset;

    static constexpr size_t _pyunicode_data_any_offset =
        cpython_v3_11::PyUnicodeObject_data_any_offset;

    static constexpr size_t _pycompactunicode_size =
        cpython_v3_11::PyCompactUnicodeObject_size;

    static constexpr size_t _pycompactunicode_base_offset =
        cpython_v3_11::PyCompactUnicodeObject_base_offset;

    static constexpr size_t _pyascii_size = cpython_v3_11::PyASCIIObject_size;

    static constexpr size_t _pyascii_length_offset =
        cpython_v3_11::PyASCIIObject_length_offset;

    static constexpr size_t _pyascii_state_offset =
        cpython_v3_11::PyASCIIObject_state_offset;

    static constexpr size_t _pybytes_base_size_offset =
        cpython_v3_11::PyBytesObject_ob_base_ob_size_offset;

    static constexpr size_t _pybytes_value_offset =
        cpython_v3_11::PyBytesObject_ob_sval_offset;
};

template <>
struct StackTracer<12> : StackTracerBase<12> {
    static constexpr size_t _pythread_cframe_offset =
        cpython_v3_12::PyThreadState_cframe_offset;

    static constexpr size_t _pycframe_current_frame_offset =
        cpython_v3_12::PyCFrame_current_frame_offset;

    static constexpr size_t _pyframe_code_offset =
        cpython_v3_12::PyInterpreterFrame_f_code_offset;

    static constexpr size_t _pyframe_back_offset =
        cpython_v3_12::PyInterpreterFrame_previous_offset;

    static constexpr size_t _pyframe_prev_inst_offset =
        cpython_v3_12::PyInterpreterFrame_prev_instr_offset;

    static constexpr size_t _pyframe_owner_offset =
        cpython_v3_12::PyInterpreterFrame_owner_offset;

    static constexpr size_t _pycode_filename_offset =
        cpython_v3_12::PyCodeObject_co_filename_offset;

    static constexpr size_t _pycode_name_offset =
        cpython_v3_12::PyCodeObject_co_name_offset;

    static constexpr size_t _pycode_line_table_offset =
        cpython_v3_12::PyCodeObject_co_linetable_offset;

    static constexpr size_t _pycode_first_lineno_offset =
        cpython_v3_12::PyCodeObject_co_firstlineno_offset;

    static constexpr size_t _pycode_code_adaptive_offset =
        cpython_v3_12::PyCodeObject_co_code_adaptive_offset;

    static constexpr size_t _pyunicode_size =
        cpython_v3_12::PyUnicodeObject_size;

    static constexpr size_t _pyunicode_base_offset =
        cpython_v3_12::PyUnicodeObject_base_offset;

    static constexpr size_t _pyunicode_data_any_offset =
        cpython_v3_12::PyUnicodeObject_data_any_offset;

    static constexpr size_t _pycompactunicode_size =
        cpython_v3_12::PyCompactUnicodeObject_size;

    static constexpr size_t _pycompactunicode_base_offset =
        cpython_v3_12::PyCompactUnicodeObject_base_offset;

    static constexpr size_t _pyascii_size = cpython_v3_12::PyASCIIObject_size;

    static constexpr size_t _pyascii_length_offset =
        cpython_v3_12::PyASCIIObject_length_offset;

    static constexpr size_t _pyascii_state_offset =
        cpython_v3_12::PyASCIIObject_state_offset;

    static constexpr size_t _pybytes_base_size_offset =
        cpython_v3_12::PyBytesObject_ob_base_ob_size_offset;

    static constexpr size_t _pybytes_value_offset =
        cpython_v3_12::PyBytesObject_ob_sval_offset;
};

}  // namespace

u32 get_python_stack_trace(PythonFrame* frames, int max_nframes) {
    PyThreadState* ts = PyGILState_GetThisThreadState();
    if (ts == nullptr) {
        return 0;
    }
#if PY_MINOR_VERSION >= 13
    // FIXME: Python 3.13+ not supported yet
    return 0;
#else
    StackTracer<PY_MINOR_VERSION> tracer;
    return tracer.get_stack_trace(ts, frames, max_nframes);
#endif
}

}  // namespace pyki