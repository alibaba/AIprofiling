#include "object_ptr.h"
#include "Macros.h"
#include <Python.h>

namespace pyki {
template <>
void THPPointer<PyObject>::free() {
  if (ptr && C10_LIKELY(Py_IsInitialized()))
    Py_DECREF(ptr);
}

template class THPPointer<PyObject>;

template <>
void THPPointer<PyCodeObject>::free() {
  if (ptr && C10_LIKELY(Py_IsInitialized()))
    Py_DECREF(ptr);
}

template class THPPointer<PyCodeObject>;

template <>
void THPPointer<PyFrameObject>::free() {
  if (ptr && C10_LIKELY(Py_IsInitialized()))
    Py_DECREF(ptr);
}

template class THPPointer<PyFrameObject>;
} // namespace pyki