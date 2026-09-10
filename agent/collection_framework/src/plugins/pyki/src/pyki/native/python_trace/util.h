#pragma once

#include <string_view>
#include <stdexcept>
#include <memory>
#include <Python.h>
#include <cstdint>
#include <vector>
#include <string>
#include <pybind11/pybind11.h>

namespace pyki {
// Unpacks PyBytes (PyString) or PyUnicode as std::string_view
// PyBytes are unpacked as-is. PyUnicode is unpacked as UTF-8.
// NOTE: If `obj` is destroyed, then the non-owning std::string_view will
//   become invalid. If the string needs to be accessed at any point after
//   `obj` is destroyed, then the std::string_view should be copied into
//   a std::string, or another owning object, and kept alive. For an example,
//   look at how IValue and autograd nodes handle std::string_view arguments.
// NOTE: this method requires the GIL
inline std::string_view THPUtils_unpackStringView(PyObject* obj) {
  if (PyBytes_Check(obj)) {
    size_t size = PyBytes_GET_SIZE(obj);
    return std::string_view(PyBytes_AS_STRING(obj), size);
  }
  if (PyUnicode_Check(obj)) {
    Py_ssize_t size = 0;
    const char* data = PyUnicode_AsUTF8AndSize(obj, &size);
    if (!data) {
      throw std::runtime_error("error unpacking string as utf-8");
    }
    return std::string_view(data, (size_t)size);
  }
  throw std::runtime_error("unpackString: expected bytes or unicode object");
}

namespace at {

struct StringView {
  StringView() : StringView(nullptr) {}
  explicit StringView(const char* str_ptr)
      : owned_str_ptr_(nullptr), str_ptr_(str_ptr) {}
  explicit StringView(std::string str)
      : owned_str_ptr_(std::make_shared<std::string>(std::move(str))),
        str_ptr_(owned_str_ptr_->c_str()) {}

  const char* str() const {
    return str_ptr_;
  }

  friend bool operator==(const StringView& lhs, const StringView& rhs) {
    return strcmp(lhs.str(), rhs.str()) == 0;
  }

  friend bool operator!=(const StringView& lhs, const StringView& rhs) {
    return !(lhs == rhs);
  }

 private:
  std::shared_ptr<std::string> owned_str_ptr_;
  const char* str_ptr_;
};

// Returns logical thread_id for the current thread
uint64_t currentThreadId();

} // namespace at

std::vector<std::string> pyListToStringVector(PyObject* list);
std::string get_current_python_thread_name();

} // namespace pyki