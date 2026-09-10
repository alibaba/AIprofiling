#include "pyki.hpp"
#include "pytorch.hpp"
#include "crash_protection.hpp"
#include "logging.hpp"
#include "profiler-link.h"
#include "recorder.hpp"
#include "cpu_profiling.hpp"
#include "memory_profiling.hpp"
#include "native_memory_profiling.hpp"
#include "stack_trace.hpp"
#include "symbols.hpp"
#include "python_trace/python_tracer.hpp"
#include "python_trace/chrome_trace.hpp"
#include <dlfcn.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include "handler.hpp"
#include "shadow.hpp"
#include "pyki_cupti.hpp"
#include <malloc.h>

#include <pybind11/pybind11.h>

namespace pyki {

PyObject* Error = NULL;

int Pid;

const char* TmpDir;

typedef void (*SigAction)(int, siginfo_t*, void*);

SigAction _old_crash_handler = NULL;

void crashHandler(int signo, siginfo_t* siginfo, void* ucontext) {
    pyki::CrashProtection::check_crash_protection();
    if (_old_crash_handler) {
        _old_crash_handler(signo, siginfo, ucontext);
    }
}

static PyObject* do_init(PyObject* Py_UNUSED(self), PyObject* Py_UNUSED(args)) {
    if (!Logger::init()) {
        return NULL;
    }

    init_stack_trace_buffer(128);

    Pid = ::getpid();
    TmpDir = "/tmp";

    struct sigaction sa;
    sigaction(SIGSEGV, NULL, &sa);
    _old_crash_handler = sa.sa_sigaction;
    sa.sa_sigaction = crashHandler;
    sa.sa_flags |= SA_SIGINFO | SA_RESTART;
    sigaction(SIGSEGV, &sa, NULL);

    Symbols::parseLibraries(false);
    Py_RETURN_NONE;
}

static PyObject* set_profiling_data_file(PyObject* Py_UNUSED(self),
                                         PyObject* args) {
    int fd;
    PyArg_ParseTuple(args, "i", &fd);
    Recorder::instance()->set_fd(fd);
    Py_RETURN_NONE;
}

static PyObject* rotate_to(PyObject* Py_UNUSED(self), PyObject* args) {
    int fd;
    PyArg_ParseTuple(args, "i", &fd);
    Recorder::instance()->rotate_to(fd);
    Py_RETURN_NONE;
}

static PyObject* start_memory_profiling(PyObject* Py_UNUSED(self),
                                        PyObject* args, PyObject* keywds) {
    return MemoryProfiling::instance()->start(args, keywds);
}

static PyObject* stop_memory_profiling(PyObject* Py_UNUSED(self),
                                       PyObject* Py_UNUSED(args)) {
    return MemoryProfiling::instance()->stop();
}

static PyObject* start_native_memory_profiling(PyObject* Py_UNUSED(self),
                                               PyObject* args, PyObject* keywds) {
    return NativeMemoryProfiling::instance()->start(args, keywds);
}

static PyObject* stop_native_memory_profiling(PyObject* Py_UNUSED(self),
                                              PyObject* Py_UNUSED(args)) {
    return NativeMemoryProfiling::instance()->stop();
}

static PyObject* start_cpu_profiling(PyObject* Py_UNUSED(self), PyObject* args,
                                     PyObject* keywds) {
    return CPUProfiling::instance()->start(args, keywds);
}

static PyObject* stop_cpu_profiling(PyObject* Py_UNUSED(self),
                                    PyObject* Py_UNUSED(args)) {
    return CPUProfiling::instance()->stop();
}

static PyObject* set_native_log_level(PyObject* Py_UNUSED(self),
                                      PyObject* args) {
    int level;
    if (!PyArg_ParseTuple(args, "i", &level)) {
        return NULL;
    }
    Logger::set_level(static_cast<LogLevel>(level));
    Py_RETURN_NONE;
}

static PyObject* flush_native_logs(PyObject* Py_UNUSED(self),
                                   PyObject* Py_UNUSED(args)) {
    Logger::flush();
    Py_RETURN_NONE;
}

extern "C" {

int convert_to_pprof(const char* input_path, const char* cpu_pprof_path,
                     const char* allocation_pprof_path, const char* native_memory_pprof_path,
                     const unsigned char native_memory_leaks, const unsigned char compress);
}

static PyObject* convert_to_pprof_files(PyObject* Py_UNUSED(self),
                                        PyObject* args, PyObject* keywds) {
    static const char* kwlist[] = {"input_path", "cpu_pprof_path",
                                   "allocation_pprof_path", "native_memory_pprof_path", "native_memory_leaks", "compress", NULL};
    const char* input_path = nullptr;
    const char* cpu_pprof_path = nullptr;
    const char* allocation_pprof_path = nullptr;
    const char* native_memory_pprof_path = nullptr;
    int native_memory_leaks;
    int compress = 0;
    if (!PyArg_ParseTupleAndKeywords(args, keywds, "|$ssssbb", (char**)kwlist, &input_path,
                                     &cpu_pprof_path, &allocation_pprof_path, &native_memory_pprof_path,
                                     &native_memory_leaks, &compress)) {
        return NULL;
    }

    int result = -1;

    Py_BEGIN_ALLOW_THREADS;
    result = convert_to_pprof(input_path, cpu_pprof_path, allocation_pprof_path, native_memory_pprof_path,
                              (const unsigned char)native_memory_leaks, (const unsigned char)compress);
    Py_END_ALLOW_THREADS;

    if (result == 0) {
        Py_RETURN_NONE;
    }
    return NULL;
}

static bool do_attach(pid_t pid) {
    char lib_path[2048];
    Dl_info info;
    if (dladdr((void*)do_attach, &info)) {
        int r = snprintf(lib_path, sizeof(lib_path), "%s", info.dli_fname);
        if (r < 0 || (size_t)r >= sizeof(lib_path)) {
            return false;
        }
    }
    else {
        return false;
    }

    profiler_t* profiler;

    int retry_count = 10;
    int sleep_before_retry = 100000;  // in microseconds

    while (true) {
        if (profiler_init(&profiler, pid, "load_pyki") != 0) {
            return false;
        }
        int attach_result = profiler_attach(profiler);
        if (attach_result == PROFILER_SUCCESS) {
            break;
        }
        else if (attach_result == PROFILER_ERR_CHECK_STACK) {
            if (retry_count-- == 0) {
                return false;
            }
            usleep(sleep_before_retry);
        }
        else {
            return false;
        }
    }

    bool success = true;
    void* handle = NULL;
    if (profiler_inject_in_cloned_thread(profiler, lib_path, &handle) != 0) {
        success = false;
    }

    profiler_loose(profiler);
    profiler_wait(profiler, THREAD_END);

    profiler_retach(profiler);
    profiler_detach(profiler);

    return success;
}

static PyObject* attach(PyObject* Py_UNUSED(self), PyObject* args) {
    pid_t pid;
    PyArg_ParseTuple(args, "i", &pid);
    bool success = false;
    {
        pybind11::gil_scoped_release gil;
        success = do_attach(pid);
    }
    if (success) {
        Py_RETURN_TRUE;
    }
    else {
        Py_RETURN_FALSE;
    }
}

static int call_profile_module_invoke_pending_call(void* ignore) {
    // TODO: log error
    auto module =
        Handler(PyImport_ImportModule(PYKI_DOT "profiling.torch_profile"));
    if (module) {
        Handler(PyObject_CallMethod(module, "_invoke_pending_call", NULL));
    }
    PyErr_Clear();
    return 0;
}

static PyObject* add_pending_call_for_torch_profile_module(
    PyObject* module, PyObject* Py_UNUSED(args)) {
    if (Py_AddPendingCall(call_profile_module_invoke_pending_call, NULL) == 0) {
        Py_RETURN_TRUE;
    }
    Py_RETURN_FALSE;
}

static PyObject* setns_wrapper(PyObject* module, PyObject* path) {
    int result = 0;

#ifdef __linux__
    const char* c_path = PyUnicode_AsUTF8(path);
    int newns = open(c_path, O_RDONLY);
    if (newns < 0) {
        result = 1;
    }
    else {
        result = setns(newns, 0);
        if (result == -1) {
            result = errno;
        }
        close(newns);
    }
#endif
    return PyLong_FromLong((long)result);
}

static PyObject* trim_native_memory(PyObject* module,
                                    PyObject* Py_UNUSED(args)) {
#ifdef __GLIBC__
    ::malloc_trim(0);
#endif
    Py_RETURN_NONE;
}

static PyObject* RecordFunctionFastHook(PyObject* Py_UNUSED(self),
                                        PyObject* Py_UNUSED(unused)) {
    Py_RETURN_NONE;
}

static PyObject* hook_record_function_fast(PyObject* Py_UNUSED(self),
                                           PyObject* args) {
    PyTypeObject* record_function_fast_type = nullptr;
    if (!PyArg_ParseTuple(args, "O", &record_function_fast_type)) {
        Py_RETURN_FALSE;
    }
    if (!PyType_Check(record_function_fast_type)) {
        Py_RETURN_FALSE;
    }
    auto methods = record_function_fast_type->tp_methods;
    if (methods[0].ml_name && strcmp(methods[0].ml_name, "__enter__") != 0) {
        Py_RETURN_FALSE;
    }
    if (methods[1].ml_name && strcmp(methods[1].ml_name, "__exit__") != 0) {
        Py_RETURN_FALSE;
    }
    methods[0].ml_meth = RecordFunctionFastHook;
    methods[1].ml_meth = RecordFunctionFastHook;
    Py_RETURN_TRUE;
}

static PyMethodDef methods[] = {
    {"do_init", do_init, METH_NOARGS, "Initialize PyKi Extension"},

    {"set_profiling_data_file", set_profiling_data_file, METH_VARARGS,
     "Set Proifling Data File"},

    {"rotate_to", rotate_to, METH_VARARGS, "Rotate To New Data File"},

    {"start_cpu_profiling", (PyCFunction)(void (*)(void))start_cpu_profiling,
     METH_VARARGS | METH_KEYWORDS, "Start CPU Profiling"},

    {"stop_cpu_profiling", stop_cpu_profiling, METH_NOARGS,
     "Stop CPU Profiling"},

    {"start_memory_profiling",
     (PyCFunction)(void (*)(void))start_memory_profiling,
     METH_VARARGS | METH_KEYWORDS, "Start Memory Profiling"},

    {"stop_memory_profiling", stop_memory_profiling, METH_NOARGS,
     "Stop Memory Profiling"},

    {"start_native_memory_profiling",
     (PyCFunction)(void (*)(void))start_native_memory_profiling,
     METH_VARARGS | METH_KEYWORDS, "Start Native Memory Profiling"},

    {"stop_native_memory_profiling", stop_native_memory_profiling, METH_NOARGS,
     "Stop Native Memory Profiling"},

    {"convert_to_pprof_files",
     (PyCFunction)(void (*)(void))convert_to_pprof_files,
     METH_VARARGS | METH_KEYWORDS, "Convert to pprof files"},

    {"set_native_log_level", set_native_log_level, METH_VARARGS,
     "Set Native Log Level"},

    {"flush_native_logs", flush_native_logs, METH_NOARGS, "Flush Native Logs"},

    {"setns_wrapper", (PyCFunction)setns_wrapper, METH_O, "setns Wrapper"},

    {"attach", attach, METH_VARARGS, "Attach To Target Process"},

    {"enable_workaround_for_3_12_0_4",
     (PyCFunction)torch::enable_workaround_for_3_12_0_4, METH_NOARGS,
     "Enable Workaround For CPython 3.12.0-4"},

    {"disable_workaround_for_3_12_0_4",
     (PyCFunction)torch::disable_workaround_for_3_12_0_4, METH_NOARGS,
     "Dnable Workaround For CPython 3.12.0-4"},

    {"start_python_tracer",
     (PyCFunction)torch::profiler::impl::start_python_tracer,
     METH_VARARGS | METH_KEYWORDS, "Start Python Tracer"},

    {"stop_python_tracer",
     (PyCFunction)torch::profiler::impl::stop_python_tracer, METH_NOARGS,
     "Stop Python Tracer"},

    {"syspend_python_tracer_for_current_thread",
     (PyCFunction)
         torch::profiler::impl::syspend_python_tracer_for_current_thread,
     METH_NOARGS, "Suspend Tracer for Current Thread"},

    {"resume_python_tracer_for_current_thread",
     (PyCFunction)
         torch::profiler::impl::resume_python_tracer_for_current_thread,
     METH_NOARGS, "Resume Tracer for Current Thread"},

    {"reset_python_tracer",
     (PyCFunction)torch::profiler::impl::reset_python_tracer, METH_NOARGS,
     "Reset Python Tracer"},

    {"export_python_tracer_data",
     (PyCFunction)torch::profiler::impl::export_python_tracer_data,
     METH_VARARGS, "Export Python Tracer Data to File"},

    {"add_pending_call_for_torch_profile_module",
     add_pending_call_for_torch_profile_module, METH_NOARGS,
     "Add Pending Call for Torch Profile Module"},

    {"get_chrome_trace_base_time", (PyCFunction)getChromeTraceBaseTime,
     METH_NOARGS, "Get Chrome Trace Base Time"},

    {"trim_native_memory", (PyCFunction)trim_native_memory, METH_NOARGS,
     "Trim Native Memory"},

    {"cuptiSetThreadIdType_wrapper", (PyCFunction)cuptiSetThreadIdTypeWrapper, METH_VARARGS,
     "cuptiSetThreadIdType wrapper"},

    {"hook_record_function_fast", (PyCFunction)hook_record_function_fast,
     METH_VARARGS, "hook record function fast"},

       
    {"enable_no_gil_for_torch_profile_export", (PyCFunction)pyki::torch::enable_no_gil_for_torch_profile_export, METH_NOARGS,
         "Enable GIL release for torch profiler export"},
    {"disable_no_gil_for_torch_profile_export", (PyCFunction)pyki::torch::disable_no_gil_for_torch_profile_export, METH_NOARGS,
         "Disable GIL release for torch profiler export"},

    {"enable_gil_check_for_PythonTraceback_gather", (PyCFunction)pyki::torch::enable_gil_check_for_PythonTraceback_gather, METH_NOARGS,
         "Enable GIL check for PythonTraceback gather"},
    {"disable_gil_check_for_PythonTraceback_gather", (PyCFunction)pyki::torch::disable_gil_check_for_PythonTraceback_gather, METH_NOARGS,
         "Enable GIL check for PythonTraceback gather"},

    {NULL, NULL, 0, NULL}};

static struct PyModuleDef mod = PyModuleDef{.m_base = PyModuleDef_HEAD_INIT,
                                            .m_name = "pyki_extension",
                                            .m_doc = "PyKi Extension",
                                            .m_size = -1,
                                            .m_methods = methods};

}  // namespace pyki

#if PY_VERSION_HEX < 0x03090000
// PyMODINIT_FUNC on Python < 3.9 does not include default visibility, so
// -fvisibility=hidden would strip PyInit_pyki_extension. Force export.
extern "C" __attribute__((visibility("default"))) PyObject* PyInit_pyki_extension(void);
#endif

PyMODINIT_FUNC PyInit_pyki_extension(void) {
    using namespace pyki;

    auto m = PyModule_Create(&::pyki::mod);
    if (m == NULL) {
        return NULL;
    }

    Error = PyErr_NewException("pyki_extension.error", NULL, NULL);
#if PY_MINOR_VERSION >= 10
    if (PyModule_AddObjectRef(m, "error", Error) < 0) {
        Py_CLEAR(Error);
        Py_DECREF(m);
        return NULL;
    }
#else
    Py_INCREF(Error);
    if (PyModule_AddObject(m, "error", Error) < 0) {
        Py_DECREF(Error);
        Py_CLEAR(Error);
        Py_DECREF(m);
        return NULL;
    }
#endif

#ifdef Py_GIL_DISABLED
    // Required for free-threaded Python to not re-enable the GIL on import.
    PyUnstable_Module_SetGIL(m, Py_MOD_GIL_NOT_USED);
#endif

    return m;
}
