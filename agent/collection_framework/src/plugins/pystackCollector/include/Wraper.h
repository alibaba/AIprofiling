#ifndef WRAPER_H
#define WRAPER_H

#include "collection.h"
#include "profiler_python.h"

#ifdef __cplusplus
extern "C" {
#endif

int start_python_tracer_c(int max_depth, 
                          int64_t threshold_ns, 
                          bool ignore_c_functions);

int stop_python_tracer_c(void);

int suspend_python_tracer_for_current_thread_c(void);

int resume_python_tracer_for_current_thread_c(void);

int reset_python_tracer_c(void);

int export_python_tracer_data_c(const char* path);

#ifdef __cplusplus
}
#endif

#endif // WRAPER_H