#include "pyki_cupti.hpp"
#include <dlfcn.h>
#include <mutex>
#include <pybind11/pybind11.h>

#define _CUPTI_ACTIVITY_THREAD_ID_TYPE_DEFAULT 0
#define _CUPTI_ACTIVITY_THREAD_ID_TYPE_SYSTEM 1

namespace pyki {
    PyObject* cuptiSetThreadIdTypeWrapper(PyObject* module, PyObject* args) {
        const char* cupti_path;
        if (!PyArg_ParseTuple(args, "s", &cupti_path)) {
            return NULL;
        }

        void* cupti_handle = dlopen(cupti_path, RTLD_LAZY | RTLD_NOLOAD);
        if (cupti_handle == nullptr) {
            PyErr_SetString(PyExc_RuntimeError, "Failed to load CUPTI library");
            return NULL;
        }

        typedef int (*cuptiSetThreadIdTypeFunc)(int);
        cuptiSetThreadIdTypeFunc cupti_set_thread_id_type_func =
            (cuptiSetThreadIdTypeFunc)dlsym(cupti_handle, "cuptiSetThreadIdType");

        if (cupti_set_thread_id_type_func == nullptr) {
            PyErr_SetString(PyExc_RuntimeError, "Failed to find cuptiSetThreadIdType symbol");
            return NULL;
        }

        int result = 0;
        {
            pybind11::gil_scoped_release gil;
            result = cupti_set_thread_id_type_func(_CUPTI_ACTIVITY_THREAD_ID_TYPE_SYSTEM);
        }

        if (result != 0) {
            PyErr_SetString(PyExc_RuntimeError, "cuptiSetThreadIdType call failed");
            return NULL;
        }

        Py_RETURN_NONE;
    }
}
