#pragma once
#include <fstream>
#include <Python.h>
#include "ApproximateClock.h"

namespace pyki {

class ChromeTraceEventWritter {
public:
  ChromeTraceEventWritter(const char* path);
  ~ChromeTraceEventWritter();
  void addMetadata(const char* key, const char* value);
  void startTraceEvent();
  void addTraceEvent(const char* category, const char* name, int pid, int tid,
                      c10::time_t ts, c10::time_t duration);
  void addProcessMetadata(const char* process_name, int pid, c10::time_t ts);
  void addThreadMetadata(const char* thread_name, int pid, int tid, c10::time_t ts);

private:
  double convertTime(c10::time_t ts);

  std::ofstream out_;
  int trace_event_count_;
  c10::time_t base_time_;
};

// std::chrono header end

// There are tools like Chrome Trace Viewer that uses double to represent
// each element in the timeline. Double has a 53 bit mantissa to support
// up to 2^53 significant digits (up to 9007199254740992). This holds at the
// nanosecond level, about 3 months and 12 days. So, let's round base time to
// 3 months intervals, so we can still collect traces across ranks relative
// to each other.
// A month is 2629746, so 3 months is 7889238.
using _trimonths = std::chrono::duration<int64_t, std::ratio<7889238>>;

class ChromeTraceBaseTime {
 public:
  ChromeTraceBaseTime() = default;
  static ChromeTraceBaseTime& singleton() {
    static ChromeTraceBaseTime instance;
    return instance;
  }

  void init() {
    get();
  }
  int64_t get() {
    // Make all timestamps relative to 3 month intervals.
    static int64_t base_time = libkineto::timeSinceEpoch(
        std::chrono::time_point<std::chrono::system_clock>(
            std::chrono::floor<_trimonths>(std::chrono::system_clock::now())));
    return base_time;
  }
};

PyObject* getChromeTraceBaseTime(PyObject* module, PyObject* args);

}  // namespace pyki