// AIProf-local modification (Apache-2.0 4(b)): this file differs from
// upstream cuprof. See VENDOR.md and patches/0002-align-cupti-epoch-to-clock-monotonic.patch
// and patches/0004-duration-from-raw-endpoint-pair.patch.
#include "trace_writer.h"

#include <unistd.h>

#include <cstdio>
#include <fstream>
#include <set>

namespace cuprof {
namespace {

// Minimal JSON string escaping. Kernel names can contain quotes and
// backslashes after demangling.
std::string Escape(const std::string& in) {
    std::string out;
    out.reserve(in.size() + 8);
    for (size_t i = 0; i < in.size(); ++i) {
        char c = in[i];
        switch (c) {
            case '"': out += "\\\""; break;
            case '\\': out += "\\\\"; break;
            case '\n': out += "\\n"; break;
            case '\r': out += "\\r"; break;
            case '\t': out += "\\t"; break;
            default:
                if (static_cast<unsigned char>(c) < 0x20) {
                    char buf[8];
                    snprintf(buf, sizeof(buf), "\\u%04x", c);
                    out += buf;
                } else {
                    out += c;
                }
        }
    }
    return out;
}

// CUPTI reports nanoseconds; the Chrome trace format expects microseconds.
double ToMicros(uint64_t ns) { return static_cast<double>(ns) / 1000.0; }

}  // namespace

bool WriteChromeTrace(const std::string& path, const std::vector<Event>& events,
                      uint64_t epoch_to_mono_offset_ns) {
    std::ofstream f(path.c_str());
    if (!f.is_open()) return false;

    const long pid = static_cast<long>(getpid());
    f.precision(3);
    f << std::fixed;
    f << "{\"displayTimeUnit\":\"ms\",\"traceEvents\":[\n";

    // Name each stream lane so the timeline is readable.
    std::set<uint32_t> streams;
    for (size_t i = 0; i < events.size(); ++i) streams.insert(events[i].stream_id);
    bool first = true;
    for (std::set<uint32_t>::const_iterator it = streams.begin(); it != streams.end(); ++it) {
        if (!first) f << ",\n";
        first = false;
        f << "{\"ph\":\"M\",\"name\":\"thread_name\",\"pid\":" << pid << ",\"tid\":" << *it
          << ",\"args\":{\"name\":\"stream " << *it << "\"}}";
    }

    for (size_t i = 0; i < events.size(); ++i) {
        const Event& e = events[i];
        if (!first) f << ",\n";
        first = false;
        // Shift the start timestamp only. A value at or below the offset is
        // left unchanged rather than subtracted, so it can never wrap uint64.
        uint64_t adj_start = e.start_ns > epoch_to_mono_offset_ns
                                 ? e.start_ns - epoch_to_mono_offset_ns
                                 : e.start_ns;
        // A duration is invariant under a uniform shift, so take it from the
        // raw pair. Shifting each endpoint separately and subtracting is not:
        // an event that straddles the offset, or one whose end_ns is still
        // unset (0), gets one endpoint shifted and the other left alone, and
        // the subtraction then underflows uint64 to ~1.8e19 ns - a single such
        // record corrupts the whole trace while WriteChromeTrace() still
        // reports success.
        uint64_t dur_ns = e.end_ns >= e.start_ns ? e.end_ns - e.start_ns : 0;
        f << "{\"ph\":\"X\",\"name\":\"" << Escape(e.name) << "\",\"cat\":\"" << e.category
          << "\",\"ts\":" << ToMicros(adj_start)
          << ",\"dur\":" << ToMicros(dur_ns) << ",\"pid\":" << pid
          << ",\"tid\":" << e.stream_id << ",\"args\":{\"device\":" << e.device_id
          << ",\"correlation\":" << e.correlation_id;
        for (size_t a = 0; a < e.args.size(); ++a) {
            f << ",\"" << e.args[a].first << "\":\"" << Escape(e.args[a].second) << "\"";
        }
        f << "}}";
    }

    f << "\n]}\n";
    f.close();
    return f.good();
}

}  // namespace cuprof
