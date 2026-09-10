#pragma once

namespace pyki {
// gh-108014 added Py_IsFinalizing() to Python 3.13.0a1
// bpo-1856 added _Py_Finalizing to Python 3.2.1b1.
// _Py_IsFinalizing() was added to PyPy 7.3.0.
#if (0x030201B1 <= PY_VERSION_HEX && PY_VERSION_HEX < 0x030D00A1) \
        && (!defined(PYPY_VERSION_NUM) || PYPY_VERSION_NUM >= 0x7030000)
static inline int Py_IsFinalizing(void)
{
#if PY_VERSION_HEX >= 0x030700A1
    // _Py_IsFinalizing() was added to Python 3.7.0a1.
    return _Py_IsFinalizing();
#else
    return (_Py_Finalizing != NULL);
#endif
}
#endif
}

// The following APIs were added in CPython 3.9 (PEP 620). Provide inline
// fallbacks so the extension still builds against Python 3.6/3.7/3.8.
#if PY_VERSION_HEX < 0x03090000
#include <frameobject.h>

static inline PyCodeObject* PyFrame_GetCode(PyFrameObject* frame) {
    Py_INCREF(frame->f_code);
    return frame->f_code;
}

static inline PyFrameObject* PyFrame_GetBack(PyFrameObject* frame) {
    PyFrameObject* back = frame->f_back;
    Py_XINCREF(back);
    return back;
}

static inline PyFrameObject* PyThreadState_GetFrame(PyThreadState* ts) {
    PyFrameObject* f = ts->frame;
    Py_XINCREF(f);
    return f;
}

static inline PyInterpreterState* PyInterpreterState_Get(void) {
    PyThreadState* ts = PyThreadState_Get();
    return ts->interp;
}
#endif
