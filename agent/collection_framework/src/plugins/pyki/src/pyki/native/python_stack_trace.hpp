#pragma once

#include "pyki.hpp"
#include "stack_trace.hpp"

namespace pyki {

u32 get_python_stack_trace(PythonFrame* frames, int max_nframes);

}  // namespace pyki