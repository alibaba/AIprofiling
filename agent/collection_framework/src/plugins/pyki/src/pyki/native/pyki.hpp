#pragma once

#define PY_SSIZE_T_CLEAN
#include <Python.h>

#include <errno.h>

#include "type.hpp"

namespace pyki {

#define CONCURRENCY_LEVEL 16

extern PyObject* Error;

extern int Pid;

extern const char* TmpDir;

static inline int get_thread_concurrency_level_index(int thread_id) {
    u32 v = (u32)thread_id;
    v ^= v >> 8;
    v ^= v >> 4;
    return v % CONCURRENCY_LEVEL;
}

static inline u64 atomic_inc(volatile u64& var, u64 increment = 1) {
    return __sync_fetch_and_add(&var, increment);
}

static inline u64 load_acquire(u64& var) {
    return __atomic_load_n(&var, __ATOMIC_ACQUIRE);
}

}  // namespace pyki

#define SET_ERROR_MESSAGE(MESSAGE) PyErr_SetString(Error, MESSAGE)

#define BEGIN_PYTHON_FUNC_TRY try {
#define END_PYTHON_FUNC_CATCH \
    } catch (const std::exception& e) { \
        std::string msg = std::string("C++ Exception occurred: ") + e.what(); \
        PyErr_SetString(PyExc_RuntimeError, msg.c_str()); \
        return nullptr; \
    } catch (...) { \
        PyErr_SetString(PyExc_RuntimeError, "Unknown C++ exception occurred"); \
        return nullptr; \
    }


#define RESTARTABLE(_cmd, _result) \
    do {                           \
        _result = _cmd;            \
    } while (((int)_result == -1) && (errno == EINTR))
