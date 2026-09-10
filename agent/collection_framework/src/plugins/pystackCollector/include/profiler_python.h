#pragma once

#include <Python.h>

#include "util.h"
#include "strong_type.h"
#include "collection.h"

namespace pyki::torch::profiler::impl {

class RecordQueue;

namespace python_tracer {

struct PythonTracerBase {
  static std::unique_ptr<PythonTracerBase> make(RecordQueue* queue);
  virtual ~PythonTracerBase() = default;

  virtual void stop() = 0;
  virtual void suspend() = 0;
  virtual void resume() = 0;
  virtual void export_data(const char* path) = 0;
};


} // namespace python_tracer
} // namespace torch::profiler::impl