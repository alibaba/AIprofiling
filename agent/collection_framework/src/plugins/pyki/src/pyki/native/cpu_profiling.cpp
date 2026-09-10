#include "cpu_profiling.hpp"
#include "crash_protection.hpp"
#include "lock.hpp"
#include "recorder.hpp"
#include "stack_trace.hpp"

#include <sys/syscall.h>
#include <sys/time.h>

#include <stdio.h>

#include "logging.hpp"
namespace pyki {

void CPUProfiling::prof_handler(int signo, siginfo_t* siginfo, void* ucontext) {
    if (!_instance->_enabled) {
        return;
    }
    ThreadCritical tc;
    if (tc.try_enter()) {
        int level_index = tc.get_index();
        u32 thread_id = tc.get_thread_id();
        CrashProtection::call(tc, [=] {
            StackTraceArgs args = {
                .ucontext = ucontext,
                .max_nframes = _instance->_max_depth,
                .native = _instance->_native,
            };
            u32 stack_trace_id = get_stack_trace(level_index, args);
            Recorder::instance()->record_cpu_event(level_index, thread_id,
                                                   stack_trace_id);
        });
    }
}

CPUProfiling* CPUProfiling::_instance = new CPUProfiling();

CPUProfiling::CPUProfiling()
    : _mutex(),
      _enabled(false),
      _interval(0),
      _max_depth(0),
      _native(false) {}

PyObject* CPUProfiling::start(PyObject* args, PyObject* keywds) {
    MutexLocker locker(_mutex);
    if (_enabled) {
        SET_ERROR_MESSAGE("CPU profiling is already enabled");
        return NULL;
    }

    int interval_ms = 10;
    int max_depth = 64;
    int native = 0;

    static const char* kwlist[] = {"interval_ms", "max_depth", "native", NULL};

    if (!PyArg_ParseTupleAndKeywords(args, keywds, "|$iib", (char**)kwlist,
                                     &interval_ms, &max_depth, &native)) {
        return NULL;
    }

    this->_interval = interval_ms;
    this->_max_depth = max_depth;
    this->_native = native;

    struct sigaction sa;
    struct sigaction oldsa;
    sigemptyset(&sa.sa_mask);
    sa.sa_sigaction = prof_handler;
    sa.sa_flags = SA_SIGINFO | SA_RESTART;
    sigaction(SIGPROF, &sa, &oldsa);

    int interval_ns = interval_ms * 1000 * 1000;

    time_t sec = interval_ns / 1000000000;
    suseconds_t usec = (interval_ns % 1000000000) / 1000;
    struct itimerval tv = {{sec, usec}, {sec, usec}};

    if (setitimer(ITIMER_PROF, &tv, NULL) != 0) {
        SET_ERROR_MESSAGE("ITIMER_PROF is not supported on this system");
        return NULL;
    }

    _enabled = true;
    Py_RETURN_NONE;
}

PyObject* CPUProfiling::stop() {
    MutexLocker locker(_mutex);
    if (!_enabled) {
        SET_ERROR_MESSAGE("CPU profiling is not enabled");
        return NULL;
    }
    struct itimerval tv = {{0, 0}, {0, 0}};
    setitimer(ITIMER_PROF, &tv, NULL);
    _enabled = false;
    Py_RETURN_NONE;
}
}  // namespace pyki