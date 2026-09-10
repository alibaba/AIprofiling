#pragma once

#include "mutex.hpp"
#include "type.hpp"
#include <queue>

namespace pyki {

enum LogLevel { DEBUG, INFO, WARNING, ERROR, OFF };

struct LogRecord {
    LogLevel level;
    bool rust_log;
    u64 timestamp;
    const char* filename;
    const char* function;
    u32 lineno;
    const char* message;
};

#define MAX_LOG_RECORD_QUEUE_SIZE 1024

class Logger {
   public:
    static bool init();
    static void set_level(LogLevel level);
    static LogLevel get_level() { return _level; }
    static void log(LogLevel level, bool rust_log, const char* filename,
                    const char* function, u32 lineno, const char* fmt, ...);
    static void flush();

   private:
    static Monitor _monitor;
    static LogLevel _level;
    static std::queue<LogRecord> _queue;
};

}  // namespace pyki

#define LOG(level, fmt, ...)                                             \
    ::pyki::Logger::log(level, false, __FILE__, __func__, __LINE__, fmt, \
                        __VA_ARGS__)
#define DEBUG(fmt, ...) LOG(::pyki::DEBUG, fmt, __VA_ARGS__)
#define INFO(fmt, ...) LOG(::pyki::INFO, fmt, __VA_ARGS__)
#define WARNING(fmt, ...) LOG(::pyki::WARNING, fmt, __VA_ARGS__)
#define ERROR(fmt, ...) LOG(::pyki::ERROR, fmt, __VA_ARGS__)

extern "C" {
::pyki::LogLevel get_pyki_native_log_level();
void pyki_log(::pyki::LogLevel level, const char* filename, u32 lineno,
              const char* message);
}