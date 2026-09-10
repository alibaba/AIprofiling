#include "Wraper.h"

std::unique_ptr<torch::profiler::impl::RecordQueue> g_record_queue = nullptr;


extern "C" int start_python_tracer_c(int max_depth, int64_t threshold_ns, bool ignore_c_functions) {
    if (g_record_queue != NULL) {
        return -1; // Profiler already running
    }
    
    ProfilerConfig config = {
        max_depth,
        threshold_ns,
        ignore_c_functions
    };
    
    g_record_queue = std::make_unique<RecordQueue>(config);    
    return 0;
}

extern "C" int stop_python_tracer_c(void) {
    if (!g_record_queue) {
        return -1;
    }
    
    g_record_queue->stop();
    return 0;
}

extern "C" int suspend_python_tracer_for_current_thread_c(void) {
    if (!g_record_queue) {
        return -1;
    }
    
    g_record_queue->suspendPythonTracerForCurrentThread();
    return 0;
}

extern "C" int resume_python_tracer_for_current_thread_c(void) {
    if (!g_record_queue) {
        return -1;
    }
    
    g_record_queue->resumePythonTracerForCurrentThread();
    return 0;
}

extern "C" int reset_python_tracer_c(void) {
    if (g_record_queue) {
        g_record_queue.reset();
        g_record_queue = NULL;
    }
    return 0;
}

extern "C" int export_python_tracer_data_c(const char* path) {
    if (!g_record_queue) {
        return -1;
    }
    
    g_record_queue->export_data(path);
    return 0;
}