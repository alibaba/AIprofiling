// AIProf-local modification (Apache-2.0 4(b)): this file differs from
// upstream cuprof. See VENDOR.md and patches/0002-align-cupti-epoch-to-clock-monotonic.patch.
#ifndef CUPROF_TRACE_WRITER_H
#define CUPROF_TRACE_WRITER_H

#include <stdint.h>

#include <string>
#include <vector>

namespace cuprof {

// One Chrome-trace "X" (complete) event.
struct Event {
    std::string name;
    std::string category;
    uint64_t start_ns;
    uint64_t end_ns;
    uint32_t stream_id;
    uint32_t device_id;
    uint32_t correlation_id;
    // Free-form extras rendered under "args"; already JSON-escaped keys.
    std::vector<std::pair<std::string, std::string> > args;
};

// Writes events as Chrome Trace Event JSON. Returns false if the file cannot
// be opened. epoch_to_mono_offset_ns is subtracted from each event timestamp
// to convert CUPTI epoch nanoseconds to CLOCK_MONOTONIC (matching pyki).
bool WriteChromeTrace(const std::string& path, const std::vector<Event>& events,
                      uint64_t epoch_to_mono_offset_ns);

}  // namespace cuprof

#endif  // CUPROF_TRACE_WRITER_H
