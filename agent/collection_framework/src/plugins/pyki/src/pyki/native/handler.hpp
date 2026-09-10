#pragma once

#include <Python.h>
#include <utility>

namespace pyki {

template <class T>
class Handler {
   public:
    Handler()
        : ptr(nullptr) {}
    explicit Handler(T* ptr) noexcept
        : ptr(ptr) {}
    Handler(Handler&& p) noexcept
        : ptr(std::exchange(p.ptr, nullptr)) {}
    Handler(const Handler& p) = delete;
    Handler& operator=(const Handler&) = delete;

    ~Handler() { free(); }
    T* get() { return ptr; }
    const T* get() const { return ptr; }
    Handler dup() const { return dup(ptr); }
    static Handler dup(const T* ptr) {
        Py_XINCREF(ptr);
        return Handler(const_cast<T*>(ptr));
    }
    static Handler none() {
        Py_INCREF(Py_None);
        return Handler(reinterpret_cast<T*>(Py_None));
    }
    T* release() {
        T* tmp = ptr;
        ptr = nullptr;
        return tmp;
    }
    operator T*() { return ptr; }
    Handler& operator=(T* new_ptr) noexcept {
        free();
        ptr = new_ptr;
        return *this;
    }
    Handler& operator=(Handler&& p) noexcept {
        free();
        ptr = p.ptr;
        p.ptr = nullptr;
        return *this;
    }
    T* operator->() { return ptr; }
    explicit operator bool() const { return ptr != nullptr; }

   private:
    void free();
    T* ptr = nullptr;
};

using PyObjectHandler = Handler<PyObject>;

} // namespace pyki