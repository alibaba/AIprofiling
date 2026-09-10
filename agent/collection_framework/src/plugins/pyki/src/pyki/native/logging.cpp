#include "logging.hpp"
#include "os.hpp"
#include "pyki.hpp"
#include "python_trace/object_ptr.h"
#include "shadow.hpp"

#include <stdio.h>
#include <stdarg.h>

namespace pyki {

static PyObject* logging_module = nullptr;

Monitor Logger::_monitor;
LogLevel Logger::_level = INFO;
std::queue<LogRecord> Logger::_queue;

bool Logger::init() {
    return true;
}

void Logger::set_level(LogLevel level) {
    MonitorLocker locker(_monitor);
    _level = level;
}

void Logger::log(LogLevel level, bool rust_log, const char* filename,
                 const char* function, u32 lineno, const char* fmt, ...) {
    return;
}

void Logger::flush() {
    return;
}

}  // namespace pyki

extern "C" {
pyki::LogLevel get_pyki_native_log_level() { return pyki::Logger::get_level(); }

void pyki_log(pyki::LogLevel level, const char* filename, u32 lineno,
              const char* message) {
    pyki::Logger::log(level, true, filename, "<rust function>", lineno, "%s",
                      message);
}
}