#include "handler.hpp"

namespace pyki {

template <>
void Handler<PyObject>::free() {
    if (ptr && Py_IsInitialized())
        Py_DECREF(ptr);
}

} // namespace pyki