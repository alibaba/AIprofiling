#include "chrome_trace.hpp"
#include <fstream>
#include <iomanip>
#include <sstream>
#include <cstdio>


namespace pyki {

ChromeTraceEventWritter::ChromeTraceEventWritter(const char* path)
    : out_(path), trace_event_count_(0),
    base_time_(ChromeTraceBaseTime::singleton().get()){
  out_ << "{";
  char buffer[64];
  std::snprintf(buffer, sizeof(buffer), "%lld", (int64_t)base_time_);
  out_ << "\"baseTimeNanoseconds\":" << buffer;
}


ChromeTraceEventWritter::~ChromeTraceEventWritter() {
  if (out_.is_open()) {
      // Finish traceEvents array
      out_ << "\n]\n}";
      out_.close();
  }
}

void ChromeTraceEventWritter::addMetadata(const char* key, const char* value) {
  out_ << ",\n\"" << key << "\":" << value;
}

void ChromeTraceEventWritter::startTraceEvent() {
  out_ << ",\n\"traceEvents\":[\n";
  trace_event_count_ = 0;
}

void ChromeTraceEventWritter::addTraceEvent(const char* category, const char* name,
                                           int pid, int tid, c10::time_t ts,
                                           c10::time_t duration) {
  if (trace_event_count_ > 0) {
    out_ << ",\n";
  }

  double rel_ts = convertTime(ts);
  double rel_dur = static_cast<double>(duration) / 1e3;

  char buffer[2048];
  std::snprintf(buffer, sizeof(buffer),
                "{\"ph\":\"X\",\"cat\":\"%s\",\"name\":\"%s\",\"pid\":%d,\"tid\":%d,"
                "\"ts\":%.3f,\"dur\":%.3f}",
                category, name, pid, tid, rel_ts, rel_dur);

  out_ << buffer;
  trace_event_count_++;
}

void ChromeTraceEventWritter::addProcessMetadata(const char* process_name, int pid, int64_t ts) {
    if (trace_event_count_ > 0) {
        out_ << ",\n";
    }

    double rel_ts = convertTime(ts);

    char buffer[2048];

    // Write process_name
    std::snprintf(buffer, sizeof(buffer),
                  "{\"name\":\"process_name\",\"ph\":\"M\",\"ts\":%.3f,"
                  "\"pid\":%d,\"tid\":0,\"args\":{\"name\":\"%s\"}}",
                  rel_ts, pid, process_name);
    out_ << buffer;
    trace_event_count_++;

    // Write process_labels
    out_ << ",\n";
    std::snprintf(buffer, sizeof(buffer),
                  "{\"name\":\"process_labels\",\"ph\":\"M\",\"ts\":%.3f,"
                  "\"pid\":%d,\"tid\":0,\"args\":{\"labels\":\"CPU\"}}",
                  rel_ts, pid);
    out_ << buffer;
    trace_event_count_++;

    // Write process_sort_index
    out_ << ",\n";
    std::snprintf(buffer, sizeof(buffer),
                  "{\"name\":\"process_sort_index\",\"ph\":\"M\",\"ts\":%.3f,"
                  "\"pid\":%d,\"tid\":0,\"args\":{\"sort_index\":%d}}",
                  rel_ts, pid, pid);
    out_ << buffer;
    trace_event_count_++;
}

void ChromeTraceEventWritter::addThreadMetadata(const char* thread_name, int pid, int tid, int64_t ts) {
    if (trace_event_count_ > 0) {
        out_ << ",\n";
    }

    double rel_ts = convertTime(ts);

    char buffer[2048];

    // Write thread_name
    std::snprintf(buffer, sizeof(buffer),
                  "{\"name\":\"thread_name\",\"ph\":\"M\",\"ts\":%.3f,"
                  "\"pid\":%d,\"tid\":%d,\"args\":{\"name\":\"%s\"}}",
                  rel_ts, pid, tid, thread_name);
    out_ << buffer;
    trace_event_count_++;

    // Write thread_sort_index
    out_ << ",\n";
    std::snprintf(buffer, sizeof(buffer),
                  "{\"name\":\"thread_sort_index\",\"ph\":\"M\",\"ts\":%.3f,"
                  "\"pid\":%d,\"tid\":%d,\"args\":{\"sort_index\":%d}}",
                  rel_ts, pid, tid, tid);
    out_ << buffer;
    trace_event_count_++;
}

double ChromeTraceEventWritter::convertTime(c10::time_t ts) {
  return static_cast<double>(ts - base_time_) / 1e3;
}

PyObject* getChromeTraceBaseTime(PyObject* module, PyObject* Py_UNUSED(args)) {
    int64_t base_time = ChromeTraceBaseTime::singleton().get();
    return PyLong_FromLongLong(base_time);
}

}  // namespace pyki