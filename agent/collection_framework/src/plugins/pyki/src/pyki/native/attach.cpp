#define PY_SSIZE_T_CLEAN
#include <Python.h>
#include "shadow.hpp"

#ifndef Py_EXPORTED_SYMBOL
#  define Py_EXPORTED_SYMBOL __attribute__((visibility("default")))
#endif

extern "C" Py_EXPORTED_SYMBOL void load_pyki(void* ignored) {
    printf("Loading pyki module.\n");
    PyGILState_STATE gstate;
    gstate = PyGILState_Ensure();
    if (PyRun_SimpleString("from " PYKI_DOT
                           "ipc import on_attach; on_attach()")) {
        printf("Failed to load pyki module.\n");
    }
    else {
        printf("Success to load pyki module.\n");
    }
    PyGILState_Release(gstate);
}
