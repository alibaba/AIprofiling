#include "stack_trace.hpp"

#include "codeCache.hpp"
#include "dwarf.hpp"
#include "python_stack_trace.hpp"
#include "symbols.hpp"
#include "stack_trace_storage.hpp"
#include "stackFrame.hpp"

namespace pyki {

const uintptr_t MAX_WALK_SIZE = 0x100000;
const intptr_t MAX_FRAME_SIZE = 0x40000;
const intptr_t MAX_INTERPRETER_FRAME_SIZE = 0x1000;
const intptr_t DEAD_ZONE = 0x1000;

PythonFrame* python_frame_buffer[CONCURRENCY_LEVEL];
NativeFrame* native_frame_buffer[CONCURRENCY_LEVEL];

void init_stack_trace_buffer(int max_nframes) {
    for (int i = 0; i < CONCURRENCY_LEVEL; i++) {
        python_frame_buffer[i] =
            (PythonFrame*)malloc(max_nframes * sizeof(PythonFrame));
        native_frame_buffer[i] =
            (NativeFrame*)malloc(max_nframes * sizeof(NativeFrame));
    }
}

CodeCache* findLibraryByAddress(const void* address) {
    CodeCacheArray& native_libs = Symbols::nativeLibs();
    const int native_lib_count = native_libs.count();
    for (int i = 0; i < native_lib_count; i++) {
        if (native_libs[i]->contains(address)) {
            return native_libs[i];
        }
    }
    return NULL;
}

const char* findNativeMethod(const void* address) {
    CodeCache* lib = findLibraryByAddress(address);
    return lib == NULL ? NULL : lib->binarySearch(address);
}

static inline bool aligned(uintptr_t ptr) {
    return (ptr & (sizeof(uintptr_t) - 1)) == 0;
}

static inline bool inDeadZone(const void* ptr) {
    return ptr < (const void*)DEAD_ZONE || ptr > (const void*)-DEAD_ZONE;
}

// AArch64: on Linux, frame link is stored at the top of the frame,
// while on macOS, frame link is at the bottom.
static inline uintptr_t defaultSenderSP(uintptr_t sp, uintptr_t fp) {
#ifdef __APPLE__
    return sp + 2 * sizeof(void*);
#else
    return fp;
#endif
}

static u32 get_native_stack_trace(NativeFrame* frames, int max_nframes,
                                  void* ucontext, const void* pc, uintptr_t sp,
                                  uintptr_t fp) {
    StackFrame frame(ucontext);
    uintptr_t bottom = (uintptr_t)&frame + MAX_WALK_SIZE;

    int nframes = 0;
    do {
        uintptr_t prev_sp = sp;

        CodeCache* cc = findLibraryByAddress(pc);
        const char* library = cc == NULL ? NULL : cc->name();
        const char* name = cc == NULL ? NULL : cc->binarySearch(pc);
        FrameDesc* f =
            cc != NULL ? cc->findFrameDesc(pc) : &FrameDesc::default_frame;

        frames[nframes++] = {library, name};

        u8 cfa_reg = (u8)f->cfa;
        int cfa_off = f->cfa >> 8;
        if (cfa_reg == DW_REG_SP) {
            sp = sp + cfa_off;
        }
        else if (cfa_reg == DW_REG_FP) {
            sp = fp + cfa_off;
        }
        else if (cfa_reg == DW_REG_PLT) {
            sp += ((uintptr_t)pc & 15) >= 11 ? cfa_off * 2 : cfa_off;
        }
        else {
            break;
        }

        // Check if the next frame is below on the current stack
        if (sp < prev_sp || sp >= prev_sp + MAX_FRAME_SIZE || sp >= bottom) {
            break;
        }

        // Stack pointer must be word aligned
        if (!aligned(sp)) {
            break;
        }

        const void* prev_pc = pc;
        if (f->fp_off & DW_PC_OFFSET) {
            pc = (const char*)pc + (f->fp_off >> 1);
        }
        else {
            if (f->fp_off != DW_SAME_FP && f->fp_off < MAX_FRAME_SIZE &&
                f->fp_off > -MAX_FRAME_SIZE) {
                fp = *(uintptr_t*)(sp + f->fp_off);
            }

            if (EMPTY_FRAME_SIZE > 0 || f->pc_off != DW_LINK_REGISTER) {
                pc = stripPointer(*(void**)(sp + f->pc_off));
            }
            else if (nframes == 1) {
                pc = (const void*)frame.link();
            }
            else {
                break;
            }

            if (EMPTY_FRAME_SIZE == 0 && cfa_off == 0 &&
                f->fp_off != DW_SAME_FP) {
                // AArch64 default_frame
                sp = defaultSenderSP(sp, fp);
                if (sp < prev_sp || sp >= bottom || !aligned(sp)) {
                    break;
                }
            }
        }

        if (inDeadZone(pc) || (pc == prev_pc && sp == prev_sp)) {
            break;
        }

    } while (nframes < max_nframes);

    return nframes;
}

static ucontext_t empty_ucontext{};

u32 get_stack_trace(int concurrency_level_index, StackTraceArgs& args) {
    PythonFrame* python_frames = python_frame_buffer[concurrency_level_index];
    NativeFrame* native_frames = native_frame_buffer[concurrency_level_index];

    void* ucontext = args.ucontext;
    int max_nframes = args.max_nframes;
    bool native = args.native;

    u32 nof_python_frames = get_python_stack_trace(python_frames, max_nframes);
    u32 nof_native_frames = 0;

    if (native && (int)nof_python_frames < max_nframes) {
        if (ucontext != nullptr) {
            StackFrame frame(ucontext);
            nof_native_frames = get_native_stack_trace(
                native_frames, max_nframes - (int)nof_python_frames, ucontext,
                (const void*)frame.pc(), frame.sp(), frame.fp());
        }
        else {
            nof_native_frames = get_native_stack_trace(
                native_frames, max_nframes - (int)nof_python_frames,
                &empty_ucontext, args.pc, (uintptr_t)args.sp,
                (uintptr_t)args.fp);
        }
    }

    if (nof_python_frames + nof_native_frames == 0) {
        return EMPTY_STACK_TRACE_ID;
    }

    return StackTraceStorage::instance()->put(nof_python_frames, python_frames,
                                              nof_native_frames, native_frames);
}

}  // namespace pyki