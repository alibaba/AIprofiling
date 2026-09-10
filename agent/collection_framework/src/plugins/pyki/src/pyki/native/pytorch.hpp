#pragma once

#include <Python.h>

namespace pyki::torch {

PyObject* enable_workaround_for_3_12_0_4(PyObject* self, PyObject* args);

PyObject* disable_workaround_for_3_12_0_4(PyObject* self, PyObject* args);

PyObject* enable_no_gil_for_torch_profile_export(PyObject* Py_UNUSED(self), PyObject* Py_UNUSED(args));

PyObject* disable_no_gil_for_torch_profile_export(PyObject* Py_UNUSED(self), PyObject* Py_UNUSED(args));

PyObject* enable_gil_check_for_PythonTraceback_gather(PyObject* Py_UNUSED(self), PyObject* Py_UNUSED(args));

PyObject* disable_gil_check_for_PythonTraceback_gather(PyObject* Py_UNUSED(self), PyObject* Py_UNUSED(args));

}  // namespace pyki::torch