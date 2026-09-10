#pragma once

#include "collection.h"
#include "profiler_python.h"

#include "python_tracer.hpp"

namespace pyki::torch::profiler::impl {
PyObject* start_python_tracer(PyObject* Py_UNUSED(self), PyObject* args,
                              PyObject* keywds);

PyObject* stop_python_tracer(PyObject* Py_UNUSED(self),
                             PyObject* Py_UNUSED(args));

PyObject* syspend_python_tracer_for_current_thread(PyObject* Py_UNUSED(self),
                                                   PyObject* Py_UNUSED(args));

PyObject* resume_python_tracer_for_current_thread(PyObject* Py_UNUSED(self),
                                                  PyObject* Py_UNUSED(args));

PyObject* reset_python_tracer(PyObject* Py_UNUSED(self),
                              PyObject* Py_UNUSED(args));

PyObject* export_python_tracer_data(PyObject* Py_UNUSED(self), PyObject* args);
}  // namespace pyki::torch