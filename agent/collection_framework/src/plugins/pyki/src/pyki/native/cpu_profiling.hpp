#pragma once

#include "pyki.hpp"
#include "mutex.hpp"
#include <signal.h>

namespace pyki {

class CPUProfiling {
   public:
    CPUProfiling();

    PyObject* start(PyObject* args, PyObject* keywds);
    PyObject* stop();

    static CPUProfiling* instance() { return _instance; };

   private:
    static CPUProfiling* _instance;

    Mutex _mutex;

    bool _enabled;
    int _interval;
    int _max_depth;
    bool _native;

    static void prof_handler(int signo, siginfo_t* siginfo, void* ucontext);
};

}  // namespace pyki