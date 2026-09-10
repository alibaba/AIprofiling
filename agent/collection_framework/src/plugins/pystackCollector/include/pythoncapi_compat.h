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