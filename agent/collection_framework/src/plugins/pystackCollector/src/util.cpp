#include "util.h"
#include <atomic>

namespace py = pybind11;

namespace pyki::at {

// Enumerates thread ids logically;
// note: std::this_thread::get_id may return potentially
// reused thread id
std::atomic<uint64_t> next_thread_id_{0};
thread_local uint64_t current_thread_id_ = 0;

uint64_t currentThreadId() {
  if (!current_thread_id_) {
    // happens only once per thread
    current_thread_id_ = ++next_thread_id_;
  }
  return current_thread_id_;
}

} // namespace at

std::vector<std::string> pyki::pyListToStringVector(PyObject* list) {
    std::vector<std::string> result;

    if (!PyList_Check(list)) {
        PyErr_SetString(PyExc_TypeError, "Input is not a list");
        return result;
    }

    Py_ssize_t size = PyList_Size(list);
    for (Py_ssize_t i = 0; i < size; ++i) {
        PyObject* item = PyList_GetItem(list, i);
        if (!PyUnicode_Check(item)) {
            PyErr_SetString(PyExc_TypeError, "List item is not a string");
            return result;
        }

        // Convert the Python string into a C string
        const char* c_str = PyUnicode_AsUTF8(item);
        if (c_str == nullptr) {
            PyErr_SetString(PyExc_UnicodeDecodeError, "Failed to decode string");
            return result;
        }
        result.emplace_back(c_str);
    }

    return result;
}

std::string pyki::get_current_python_thread_name() {
  py::gil_scoped_acquire gil;
  py::object threading = py::module::import("threading");
  py::object current_thread = threading.attr("current_thread")();
  std::string name = current_thread.attr("name").cast<std::string>();
  return name;
}