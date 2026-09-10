#pragma once

#include "pyki.hpp"
#include "mutex.hpp"

namespace pyki {

class MemoryProfiling {
   public:
    MemoryProfiling();

    PyObject* start(PyObject* args, PyObject* keywds);
    PyObject* stop();

    static MemoryProfiling* instance() { return _instance; };

   private:
    static MemoryProfiling* _instance;

    Mutex _mutex;

    bool _enabled;
    int _interval;
    int _max_depth;
    bool _native;
};

}  // namespace pyki