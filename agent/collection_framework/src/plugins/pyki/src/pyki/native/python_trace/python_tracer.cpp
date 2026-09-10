#include "collection.h"
#include "python_tracer.hpp"
#include "profiler_python.h"
#include <Python.h>
#include <pybind11/pybind11.h>
#include "pythoncapi_compat.h"
#include "../pyki.hpp"

namespace pyki {
namespace {

std::unique_ptr<torch::profiler::impl::RecordQueue> g_record_queue = nullptr;

} // namespace

namespace torch::profiler::impl {

PyObject* start_python_tracer(PyObject* Py_UNUSED(self),
                              PyObject* args,
                              PyObject* keywds) {
    BEGIN_PYTHON_FUNC_TRY

    int python_tracer_max_depth = 0;
    int python_tracer_threshold_ns = 0;
    int python_tracer_ignore_c_functions = 0;

    static const char* kwlist[] = {
        "max_depth", "threshold_ns", "ignore_c_functions", NULL};

    if (!PyArg_ParseTupleAndKeywords(
            args, keywds, "iLp", const_cast<char**>(kwlist),
            &python_tracer_max_depth,
            &python_tracer_threshold_ns,
            &python_tracer_ignore_c_functions)) {
        return nullptr;
    }

    ProfilerConfig config = {
        python_tracer_max_depth,
        python_tracer_threshold_ns,
        python_tracer_ignore_c_functions != 0};

    if (g_record_queue != nullptr) {
        PyErr_SetString(PyExc_RuntimeError, "Profiler is already running.");
        return nullptr;
    }
    g_record_queue = std::make_unique<RecordQueue>(config);
    Py_RETURN_NONE;

    END_PYTHON_FUNC_CATCH
}

PyObject* stop_python_tracer(PyObject* Py_UNUSED(self),
                             PyObject* Py_UNUSED(args)) {
    BEGIN_PYTHON_FUNC_TRY

    if (!g_record_queue) {
        PyErr_SetString(PyExc_RuntimeError, "Profiler is not running.");
        return nullptr;
    }

    g_record_queue->stop();
    Py_RETURN_NONE;

    END_PYTHON_FUNC_CATCH
}

PyObject* syspend_python_tracer_for_current_thread(PyObject* Py_UNUSED(self),
                                                    PyObject* Py_UNUSED(args)) {
    BEGIN_PYTHON_FUNC_TRY

    if (!g_record_queue) {
        PyErr_SetString(PyExc_RuntimeError, "Profiler is not running.");
        return nullptr;
    }

    g_record_queue->suspendPythonTracerForCurrentThread();
    Py_RETURN_NONE;

    END_PYTHON_FUNC_CATCH
}

PyObject* resume_python_tracer_for_current_thread(PyObject* Py_UNUSED(self),
                                                  PyObject* Py_UNUSED(args)) {
    BEGIN_PYTHON_FUNC_TRY

    if (!g_record_queue) {
        PyErr_SetString(PyExc_RuntimeError, "Profiler is not running.");
        return nullptr;
    }

    g_record_queue->resumePythonTracerForCurrentThread();
    Py_RETURN_NONE;

    END_PYTHON_FUNC_CATCH
}

PyObject* reset_python_tracer(PyObject* Py_UNUSED(self),
                              PyObject* Py_UNUSED(args)) {
    BEGIN_PYTHON_FUNC_TRY

    if (g_record_queue) {
        g_record_queue.reset();
    }
    Py_RETURN_NONE;

    END_PYTHON_FUNC_CATCH
}

PyObject* export_python_tracer_data(PyObject* Py_UNUSED(self),
                                   PyObject* args) {
    BEGIN_PYTHON_FUNC_TRY

    const char* path = nullptr;
    if (!PyArg_ParseTuple(args, "s", &path)) {
        return nullptr;
    }

    if (!g_record_queue) {
        PyErr_SetString(PyExc_RuntimeError, "Profiler is not running.");
        return nullptr;
    }
    {
        pybind11::gil_scoped_release gil_release;
        g_record_queue->export_data(path);
        // prevent crash, see https://github.com/pytorch/pytorch/issues/38228
        if (Py_IsFinalizing()) {
          gil_release.disarm();
        }
    }
    Py_RETURN_NONE;

    END_PYTHON_FUNC_CATCH
}

} // namespace torch::profiler::impl
} // namespace pyki