#include "handler.hpp"

#include <stddef.h>
#include <dlfcn.h>
#include <sys/mman.h>
#include <unistd.h>
#include <pybind11/pybind11.h>
#include "python_trace/pythoncapi_compat.h"
#include "codeCache.hpp"
#include "symbols.hpp"

namespace pyki::torch {

#ifdef __x86_64__
class CallSiteHook {
   private:
    static const int JUMP_INSTRUCTION_LENGTH = 5;
    static const size_t PAGE_SIZE;  // 静态页面大小变量
    
    void* call_instruction_address = nullptr;
    void* original_function_address = nullptr;
    void* allocated_trampoline = nullptr;
    uint8_t original_instruction[JUMP_INSTRUCTION_LENGTH];

    struct MemoryRegion {
        uintptr_t start;
        uintptr_t end;
    };

    std::vector<MemoryRegion> parse_memory_maps() {
        std::vector<MemoryRegion> regions;
        FILE* maps_file = fopen("/proc/self/maps", "r");
        if (!maps_file) {
            return regions;
        }

        char line[2048];
        while (fgets(line, sizeof(line), maps_file)) {
            MemoryRegion region = {};
            unsigned long start, end;

            int parsed = sscanf(line, "%lx-%lx", &start, &end);

            if (parsed >= 2) {
                region.start = start;
                region.end = end;
                regions.push_back(region);
            }
        }

        fclose(maps_file);
        return regions;
    }

    // 查找指定地址附近的大空隙
    void* find_nearby_hole(uintptr_t target_addr, size_t required_size, size_t search_range) {
        auto regions = parse_memory_maps();

        std::sort(regions.begin(), regions.end(),
                  [](const MemoryRegion& a, const MemoryRegion& b) {
                      return a.start < b.start;
                  });

        uintptr_t search_start = target_addr - search_range;
        uintptr_t search_end = target_addr + search_range;

        // 查找在搜索范围内的空隙
        uintptr_t prev_end = 0;
        for (const auto& region : regions) {
            if (region.start > prev_end) {
                // 发现一个空隙
                uintptr_t hole_start = prev_end;
                uintptr_t hole_end = region.start;

                // 检查空隙是否在目标范围内且足够大
                if (hole_start >= search_start && hole_end <= search_end) {
                    uintptr_t aligned_start = (hole_start + PAGE_SIZE - 1) & ~(PAGE_SIZE - 1);

                    // 关键：还要检查对齐后的地址是否仍在32位范围内
                    ptrdiff_t offset =
                        aligned_start - (target_addr + JUMP_INSTRUCTION_LENGTH);
                    if (aligned_start + required_size <= hole_end &&  // 足够大
                        offset >= INT32_MIN &&
                        offset <= INT32_MAX) {  // 在32位范围内
                        return (void*)aligned_start;
                    }
                }
            }
            prev_end = region.end;
        }

        // 检查最后一个区域之后是否有空隙
        if (prev_end < search_end) {
            uintptr_t aligned_start = (prev_end + PAGE_SIZE - 1) & ~(PAGE_SIZE - 1);

            // 同样检查对齐后的地址是否在32位范围内
            ptrdiff_t offset =
                aligned_start - (target_addr + JUMP_INSTRUCTION_LENGTH);
            if (aligned_start + required_size <= search_end &&  // 足够大
                offset >= INT32_MIN && offset <= INT32_MAX) {   // 在32位范围内
                return (void*)aligned_start;
            }
        }

        return nullptr;  // 未找到合适的空隙
    }

    CodeBlob* find_function(const char* so_name, const char* function) {
        CodeCacheArray& native_libs = Symbols::nativeLibs();
        const int native_lib_count = native_libs.count();
        for (int i = 0; i < native_lib_count; i++) {
            CodeCache* cc = native_libs[i];
            if (strstr(cc->name(), so_name) != nullptr) {
                CodeBlob* blob = cc->findBlob(function);
                if (blob) {
                    return blob;
                }
            }
        }
        return nullptr;
    }

   public:
    bool enter_hook(const char* caller_so, const char* caller_function,
                    const char* callee_so, const char* callee_function,
                    void* new_function) {
        // 获取caller函数地址
        CodeBlob* caller_func_blob = find_function(caller_so, caller_function);
        if (!caller_func_blob) {
            return false;
        }
        uint8_t* caller_func_addr = (uint8_t *)caller_func_blob->_start;
        uint8_t* caller_func_addr_end = (uint8_t *)caller_func_blob->_end;

        // 获取callee函数地址
        CodeBlob* callee_func_blob = find_function(callee_so, callee_function);
        if (!callee_func_blob) {
            return false;
        }
        uint8_t* callee_func_addr = (uint8_t *)callee_func_blob->_start;

        // 遍历caller函数的指令，查找对callee函数的调用
        uintptr_t page_start = (uintptr_t)caller_func_addr & ~(PAGE_SIZE - 1);
        uintptr_t page_end = ((uintptr_t)caller_func_addr_end + PAGE_SIZE - 1) & ~(PAGE_SIZE - 1);
        size_t total_size = page_end - page_start;

        if (::mprotect((void*)page_start, total_size,
                    PROT_READ | PROT_WRITE | PROT_EXEC) != 0) {
            return false;
        }

        for (uint8_t* instruction_addr = caller_func_addr; instruction_addr <= caller_func_addr_end - JUMP_INSTRUCTION_LENGTH; instruction_addr++) {
            // jmpq or callq
            if (instruction_addr[0] == 0xe9 || instruction_addr[0] == 0xe8) {
                int32_t rel_offset = *(int32_t*)(instruction_addr + 1);
                uint8_t* target_addr = instruction_addr + JUMP_INSTRUCTION_LENGTH + rel_offset;

                if (target_addr == callee_func_addr) {
                    // 寻找附近可用的内存空间
                    void* trampoline = nullptr;
                    for (int retry_count = 0; retry_count < 3; retry_count++) {
                        void* trampoline_addr = find_nearby_hole((uintptr_t)instruction_addr, PAGE_SIZE, 1UL << 30);  // 1GB 范围
                        if (!trampoline_addr) {
                            break;
                        }

                        trampoline = mmap(trampoline_addr, PAGE_SIZE,
                                        PROT_READ | PROT_WRITE | PROT_EXEC,
                                        MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);

                        if (trampoline != MAP_FAILED) {
                            ptrdiff_t actual_offset = (uint8_t*)trampoline - (instruction_addr + JUMP_INSTRUCTION_LENGTH);
                            if (actual_offset >= INT32_MIN && actual_offset <= INT32_MAX) {
                                allocated_trampoline = trampoline;
                                break;
                            } else {
                                munmap(trampoline, PAGE_SIZE);
                                trampoline = nullptr;
                                continue;
                            }
                        }
                    }

                    if (!allocated_trampoline) {
                        return false;
                    }

                    // 设置跳板代码
                    uint8_t* tramp_code = static_cast<uint8_t*>(trampoline);
                    tramp_code[0] = 0x48;  // REX.W
                    tramp_code[1] = 0xB8;  // MOV RAX, imm64
                    *(uint64_t*)(tramp_code + 2) =
                        reinterpret_cast<uint64_t>(new_function);
                    tramp_code[10] = 0xFF;  // JMP
                    tramp_code[11] = 0xE0;  // RAX

                    // 计算到跳板的偏移
                    ptrdiff_t tramp_offset =(uint8_t*)trampoline - (instruction_addr + JUMP_INSTRUCTION_LENGTH);

                    // 保存原始指令
                    memcpy(original_instruction, instruction_addr,JUMP_INSTRUCTION_LENGTH);
                    call_instruction_address = instruction_addr;

                    // 替换偏移值 - 跳转到跳板
                    *(int32_t*)(instruction_addr + 1) = static_cast<int32_t>(tramp_offset);

                    original_function_address = callee_func_addr;

                    return true;
                }
            }
        }
        
        return false;
    }

    bool exit_hook() {
        if (call_instruction_address != nullptr) {
            memcpy(call_instruction_address, original_instruction,
                   JUMP_INSTRUCTION_LENGTH);
        }

        // 释放跳板内存
        if (allocated_trampoline != nullptr) {
            munmap(allocated_trampoline, PAGE_SIZE);
            allocated_trampoline = nullptr;
        }

        return true;
    }

    void* get_original_function_address() {
        return original_function_address;
    }
};

// 定义静态变量
const size_t CallSiteHook::PAGE_SIZE = sysconf(_SC_PAGESIZE);

static CallSiteHook* kineto_save_hook = nullptr;

void kineto_save_no_gil(void* this_obj, const std::string& path) {
    if (kineto_save_hook != nullptr &&
        kineto_save_hook->get_original_function_address() != nullptr) {
        pybind11::gil_scoped_release gil;
        typedef void (*save_func_type)(void *, const std::string&);
        save_func_type original_save_func = reinterpret_cast<save_func_type>(
            kineto_save_hook->get_original_function_address());
        original_save_func(this_obj, path);

        if (Py_IsFinalizing()) {
            gil.disarm();
        }
    }
}

#endif
PyObject* enable_no_gil_for_torch_profile_export(PyObject* Py_UNUSED(self),
                                                 PyObject* args) {
#ifdef __x86_64__
    pybind11::gil_scoped_release gil;
    kineto_save_hook = new CallSiteHook();
    bool success = kineto_save_hook->enter_hook(
        "libtorch_cpu.so",
        "_ZN5torch8autograd8profiler14ProfilerResult4saveERKNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEE",
        "libtorch_cpu.so",
        "_ZN5torch8profiler4impl6kineto20ActivityTraceWrapper4saveERKNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEE",
        (void*)kineto_save_no_gil);
    if (Py_IsFinalizing()) {
        gil.disarm();
    }

    if (!success) {
        delete kineto_save_hook;
        kineto_save_hook = nullptr;
        Py_RETURN_FALSE;
    }

    Py_RETURN_TRUE;
#else
    Py_RETURN_FALSE;
#endif
}

PyObject* disable_no_gil_for_torch_profile_export(PyObject* Py_UNUSED(self),
                                                  PyObject* Py_UNUSED(args)) {
#ifdef __x86_64__
    if (kineto_save_hook != nullptr) {
        kineto_save_hook->exit_hook();
        delete kineto_save_hook;
        kineto_save_hook = nullptr;
    }
#endif
    Py_RETURN_NONE;
}

#ifdef __x86_64__

static CallSiteHook* pythonTraceback_gather_hook = nullptr;

extern "C" {
int Py_IsInitialized_gil_check() {
    if (pythonTraceback_gather_hook != nullptr &&
        pythonTraceback_gather_hook->get_original_function_address() != nullptr) {
        typedef int (*Py_IsInitialized_func_type)(void);
        Py_IsInitialized_func_type original_func = reinterpret_cast<Py_IsInitialized_func_type>(
            pythonTraceback_gather_hook->get_original_function_address());
        if (!original_func()) {
            return false;
        }
        // check if python thread state was ever binded
        if (PyGILState_GetThisThreadState() == nullptr) {
            return false;
        }
        return true;
    }
}

}

#endif

PyObject* enable_gil_check_for_PythonTraceback_gather(PyObject* Py_UNUSED(self), PyObject* Py_UNUSED(args)) {
#ifdef __x86_64__
    pybind11::gil_scoped_release gil;
    pythonTraceback_gather_hook = new CallSiteHook();
    bool success = pythonTraceback_gather_hook->enter_hook(
        "libtorch_python.so",
        "_ZN5torch12_GLOBAL__N_115PythonTraceback6gatherEv",
        "libtorch_python.so",
        "Py_IsInitialized@plt",
        (void*)Py_IsInitialized_gil_check);
    if (Py_IsFinalizing()) {
        gil.disarm();
    }

    if (!success) {
        delete pythonTraceback_gather_hook;
        pythonTraceback_gather_hook = nullptr;
        Py_RETURN_FALSE;
    }

    Py_RETURN_TRUE;
#else
    Py_RETURN_FALSE;
#endif
}

PyObject* disable_gil_check_for_PythonTraceback_gather(PyObject* Py_UNUSED(self), PyObject* Py_UNUSED(args)) {
#ifdef __x86_64__
    if (pythonTraceback_gather_hook != nullptr) {
        pythonTraceback_gather_hook->exit_hook();
        delete pythonTraceback_gather_hook;
        pythonTraceback_gather_hook = nullptr;
    }
#endif
    Py_RETURN_NONE;
}

#define IS_PYTHON_3_12 (PY_MAJOR_VERSION == 3 && PY_MINOR_VERSION == 12)

#if IS_PYTHON_3_12
struct _PyEventHandler {
    PyObject_HEAD vectorcallfunc vectorcall;
};

inline bool checkString(PyObject* obj) {
    return PyBytes_Check(obj) || PyUnicode_Check(obj);
}

inline const char* getString(PyObject* obj) {
    if (PyBytes_Check(obj)) {
        return PyBytes_AS_STRING(obj);
    }
    else if (PyUnicode_Check(obj)) {
        return PyUnicode_AsUTF8(obj);
    }
    return nullptr;
}

static PyTypeObject _PyEventHandler_Type = {
    PyVarObject_HEAD_INIT(&PyType_Type,
                          0) "torch.profiler.python_tracer_event_handler",
    sizeof(_PyEventHandler),
    .tp_dealloc = (destructor)PyObject_Free,
    .tp_vectorcall_offset = offsetof(_PyEventHandler, vectorcall),
    .tp_call = PyVectorcall_Call,
    .tp_flags = Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE |
                Py_TPFLAGS_HAVE_VECTORCALL | Py_TPFLAGS_DISALLOW_INSTANTIATION,
};

static PyObject* c_call_callback(_PyEventHandler* self, PyObject* const* args,
                                 size_t nargsf, PyObject* kwnames) {
    PyThreadState* tstate = PyThreadState_GET();
    if (!tstate->c_profilefunc) {
        Py_RETURN_NONE;
    }

    PyObject* callable = args[2];
    if (Py_TYPE(callable) == &PyMethod_Type) {
        PyObject* func = PyMethod_GET_FUNCTION(callable);
        if (!func) {
            return NULL;
        }
        if (PyCFunction_Check(func)) {
            PyFrameObject* frame = PyEval_GetFrame();
            if (!frame) {
                PyErr_SetString(PyExc_SystemError,
                                "Missing frame when calling profile function.");
                return NULL;
            }
            Py_INCREF(frame);
            int err = tstate->c_profilefunc(tstate->c_profileobj, frame,
                                            PyTrace_C_CALL, func);
            Py_DECREF(frame);
            if (err) {
                return NULL;
            }
        }
    }
    Py_RETURN_NONE;
}

#define PROFILER_ID 2
#define PY_MONITORING_EVENT_CALL 4

#define PY_MONITORING_EVENT_PY_START 0
#define PY_MONITORING_EVENT_PY_RESUME 1
#define PY_MONITORING_EVENT_PY_RETURN 2
#define PY_MONITORING_EVENT_PY_YIELD 3
#define PY_MONITORING_EVENT_CALL 4
#define PY_MONITORING_EVENT_LINE 5
#define PY_MONITORING_EVENT_INSTRUCTION 6
#define PY_MONITORING_EVENT_JUMP 7
#define PY_MONITORING_EVENT_BRANCH 8
#define PY_MONITORING_EVENT_STOP_ITERATION 9
#define PY_MONITORING_EVENT_RAISE 10
#define PY_MONITORING_EVENT_EXCEPTION_HANDLED 11
#define PY_MONITORING_EVENT_PY_UNWIND 12
#define PY_MONITORING_EVENT_PY_THROW 13
#define PY_MONITORING_EVENT_RERAISE 14

static void registerMonitoringCallback() {
    auto sys_module = Handler(PyImport_ImportModule("sys"));
    if (!sys_module) {
        return;
    }
    auto monitoring = Handler(PyObject_GetAttrString(sys_module, "monitoring"));
    if (!monitoring) {
        return;
    }
    auto result = Handler(PyObject_CallMethod(monitoring, "use_tool_id", "is",
                                              PROFILER_ID, "PyKi Profiler"));
    if (!result) {
        return;
    }
    auto handler = Handler(PyObject_NEW(PyObject, &_PyEventHandler_Type));
    if (!handler) {
        return;
    }
    reinterpret_cast<_PyEventHandler*>(handler.get())->vectorcall =
        (vectorcallfunc)c_call_callback;
    result = Handler(
        PyObject_CallMethod(monitoring, "register_callback", "iiO", PROFILER_ID,
                            1 << PY_MONITORING_EVENT_CALL, handler.get()));
    if (!result) {
        return;
    }
    result =
        Handler(PyObject_CallMethod(monitoring, "set_events", "ii", PROFILER_ID,
                                    1 << PY_MONITORING_EVENT_CALL));
    if (!result) {
        return;
    }
}

static void unregisterMonitoringCallback() {
    auto sys_module = Handler(PyImport_ImportModule("sys"));
    if (!sys_module) {
        return;
    }
    auto monitoring = Handler(PyObject_GetAttrString(sys_module, "monitoring"));
    if (!monitoring) {
        return;
    }
    auto tool_name =
        Handler(PyObject_CallMethod(monitoring, "get_tool", "i", PROFILER_ID));
    if (!tool_name) {
        return;
    }
    const char* str = getString(tool_name);
    if (!str || strcmp(str, "PyKi Profiler") != 0) {
        return;
    }
    auto none = Handler(Py_None);
    Py_INCREF(Py_None);
    auto result = Handler(
        PyObject_CallMethod(monitoring, "register_callback", "iiO", PROFILER_ID,
                            1 << PY_MONITORING_EVENT_CALL, none.get()));
    if (!result) {
        return;
    }
    result = Handler(
        PyObject_CallMethod(monitoring, "set_events", "ii", PROFILER_ID, 0));
    if (!result) {
        return;
    }
    result = Handler(
        PyObject_CallMethod(monitoring, "free_tool_id", "i", PROFILER_ID));
    if (!result) {
        return;
    }
}

PyObject* enable_workaround_for_3_12_0_4(PyObject* Py_UNUSED(self),
                                         PyObject* Py_UNUSED(args)) {
    registerMonitoringCallback();
    Py_RETURN_NONE;
}

PyObject* disable_workaround_for_3_12_0_4(PyObject* Py_UNUSED(self),
                                          PyObject* Py_UNUSED(args)) {
    unregisterMonitoringCallback();
    Py_RETURN_NONE;
}

#else

PyObject* enable_workaround_for_3_12_0_4(PyObject* Py_UNUSED(self),
                                         PyObject* Py_UNUSED(args)) {
    Py_RETURN_NONE;
}

PyObject* disable_workaround_for_3_12_0_4(PyObject* Py_UNUSED(self),
                                          PyObject* Py_UNUSED(args)) {
    Py_RETURN_NONE;
}

#endif // IS_PYTHON_3_12
}  // namespace pyki::torch