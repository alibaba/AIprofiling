// SPDX-License-Identifier: Apache-2.0
//
// Native regression test for AIProf's local cuprof patch 0002
// (patches/0002-align-cupti-epoch-to-clock-monotonic.patch), which converts
// CUPTI's CLOCK_REALTIME nanosecond timestamps to CLOCK_MONOTONIC so kernel
// events share a Perfetto timeline with pyki / torch.profiler.
//
// trace_writer.cc has no CUDA/CUPTI dependency, so this builds and runs on any
// Linux box with a C++11 compiler — no GPU, no CUDA toolkit, no container:
//
//     make -C test/native && ./test/native/test_cuprof_trace_writer
//
// It is the only automated coverage of the vendored-patch behaviour; the rest
// of cuprof needs a GPU to exercise.

#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>

// Resolved via -I$(CUPROF)/src in the sibling Makefile.
#include "trace_writer.h"

namespace {

int g_failures = 0;

void Check(bool ok, const std::string& what) {
    std::cout << (ok ? "  ok   " : "  FAIL ") << what << "\n";
    if (!ok) ++g_failures;
}

std::string ReadFile(const std::string& path) {
    std::ifstream f(path.c_str());
    std::stringstream ss;
    ss << f.rdbuf();
    return ss.str();
}

cuprof::Event MakeEvent(const std::string& name, uint64_t start_ns, uint64_t end_ns,
                        uint32_t stream) {
    cuprof::Event e;
    e.name = name;
    e.category = "kernel";
    e.start_ns = start_ns;
    e.end_ns = end_ns;
    e.stream_id = stream;
    e.device_id = 0;
    e.correlation_id = 7;
    return e;
}

// Pull the nth occurrence of `"ts":<number>` (or `"dur":`) out of the JSON.
// The writer emits one metadata ("ph":"M") record per stream first, then the
// "ph":"X" events in input order, so occurrences line up with event order.
bool NthDouble(const std::string& json, const std::string& key, size_t n, double* out) {
    const std::string needle = "\"" + key + "\":";
    size_t pos = 0, seen = 0;
    while ((pos = json.find(needle, pos)) != std::string::npos) {
        pos += needle.size();
        if (seen++ == n) {
            *out = strtod(json.c_str() + pos, NULL);
            return true;
        }
    }
    return false;
}

// A realistic epoch/CLOCK_REALTIME nanosecond timestamp: 2026-09-08-ish.
const uint64_t kEpochNs = 1788853810ULL * 1000000000ULL;
// (CLOCK_REALTIME - CLOCK_MONOTONIC) as LoadConfig() would compute it on a
// machine with ~4 days of uptime.
const uint64_t kOffsetNs = kEpochNs - 345600ULL * 1000000000ULL;

void TestOffsetApplied() {
    std::cout << "TestOffsetApplied: CUPTI epoch ns are shifted to CLOCK_MONOTONIC\n";
    const std::string path = "/tmp/aiprof_cuprof_tw_offset.json";
    std::vector<cuprof::Event> events;
    events.push_back(MakeEvent("nvjet_gemm", kEpochNs + 1000000, kEpochNs + 1500000, 7));
    events.push_back(MakeEvent("nccl_allreduce", kEpochNs + 2000000, kEpochNs + 3000000, 20));

    Check(cuprof::WriteChromeTrace(path, events, kOffsetNs), "WriteChromeTrace returns true");

    const std::string json = ReadFile(path);
    double ts0 = -1, dur0 = -1, ts1 = -1, dur1 = -1;
    Check(NthDouble(json, "ts", 0, &ts0) && NthDouble(json, "dur", 0, &dur0), "event 0 has ts/dur");
    Check(NthDouble(json, "ts", 1, &ts1) && NthDouble(json, "dur", 1, &dur1), "event 1 has ts/dur");

    // Expected values are derived from the offset rather than hardcoded, so the
    // assertion cannot drift out of sync with the fixture. With kOffsetNs set
    // to (epoch - 4 days of uptime), an event at epoch+1ms lands 4 days + 1ms
    // into the monotonic clock.
    const double kUptimeUs = 345600.0 * 1e6;  // 4 days, in microseconds
    Check(ts0 == kUptimeUs + 1000.0,
          "event 0 ts == uptime+1000 us (got " + std::to_string(ts0) + ")");
    Check(dur0 == 500.0, "event 0 dur == 500.000 us (got " + std::to_string(dur0) + ")");
    Check(ts1 == kUptimeUs + 2000.0,
          "event 1 ts == uptime+2000 us (got " + std::to_string(ts1) + ")");
    Check(dur1 == 1000.0, "event 1 dur == 1000.000 us (got " + std::to_string(dur1) + ")");

    // The whole point of the patch: an unadjusted CUPTI trace would sit at
    // ~1.788e15 us (year 2026 on the epoch clock) while pyki lanes sit at
    // ~3.4e11 us (seconds-since-boot), so the two never overlap in Perfetto.
    const double unadjusted = static_cast<double>(kEpochNs + 1000000) / 1000.0;
    Check(unadjusted > 1e15, "fixture really is an epoch-scale timestamp");
    Check(ts0 < unadjusted / 1000.0, "ts moved onto the monotonic scale (~1000x smaller)");
    Check(ts0 < 1e12, "ts is in monotonic range, not epoch range");
    std::remove(path.c_str());
}

void TestSaturationNeverWraps() {
    std::cout << "TestSaturationNeverWraps: timestamps below the offset clamp to themselves\n";
    const std::string path = "/tmp/aiprof_cuprof_tw_wrap.json";
    std::vector<cuprof::Event> events;
    // start_ns < offset: naive subtraction would wrap uint64 to ~1.8e19 ns and
    // produce an absurd ts. The patch guards with a ternary instead.
    events.push_back(MakeEvent("early_kernel", 500, 900, 1));

    Check(cuprof::WriteChromeTrace(path, events, kOffsetNs), "WriteChromeTrace returns true");
    const std::string json = ReadFile(path);
    double ts = -1, dur = -1;
    Check(NthDouble(json, "ts", 0, &ts) && NthDouble(json, "dur", 0, &dur), "event has ts/dur");
    Check(ts == 0.5, "pre-offset ts stays 0.500 us, no uint64 wrap (got " + std::to_string(ts) + ")");
    Check(dur == 0.4, "pre-offset dur stays 0.400 us (got " + std::to_string(dur) + ")");
    Check(json.find("18446744") == std::string::npos, "no wrapped-uint64 digits in output");
    std::remove(path.c_str());
}

void TestZeroOffsetIsUpstreamBehaviour() {
    std::cout << "TestZeroOffsetIsUpstreamBehaviour: offset 0 is a pure passthrough\n";
    const std::string path = "/tmp/aiprof_cuprof_tw_zero.json";
    std::vector<cuprof::Event> events;
    events.push_back(MakeEvent("memcpy_h2d", 123456789, 123999789, 3));

    Check(cuprof::WriteChromeTrace(path, events, 0), "WriteChromeTrace returns true");
    const std::string json = ReadFile(path);
    double ts = -1, dur = -1;
    Check(NthDouble(json, "ts", 0, &ts) && NthDouble(json, "dur", 0, &dur), "event has ts/dur");
    Check(ts == 123456.789, "ts == 123456.789 us (got " + std::to_string(ts) + ")");
    Check(dur == 543.0, "dur == 543.000 us (got " + std::to_string(dur) + ")");
    std::remove(path.c_str());
}

void TestStreamLanesAndStructure() {
    std::cout << "TestStreamLanesAndStructure: one metadata lane per stream, JSON well formed\n";
    const std::string path = "/tmp/aiprof_cuprof_tw_lanes.json";
    std::vector<cuprof::Event> events;
    events.push_back(MakeEvent("k1", kEpochNs, kEpochNs + 1000, 7));
    events.push_back(MakeEvent("k2", kEpochNs + 1000, kEpochNs + 2000, 7));
    events.push_back(MakeEvent("k3", kEpochNs + 2000, kEpochNs + 3000, 20));

    Check(cuprof::WriteChromeTrace(path, events, kOffsetNs), "WriteChromeTrace returns true");
    const std::string json = ReadFile(path);
    Check(json.compare(0, 36, "{\"displayTimeUnit\":\"ms\",\"traceEvents") == 0,
          "starts with the chrome-trace envelope");
    Check(json.find("\"ph\":\"M\"") != std::string::npos, "emits thread_name metadata records");
    size_t lanes = 0, pos = 0;
    while ((pos = json.find("\"ph\":\"M\"", pos)) != std::string::npos) { ++lanes; ++pos; }
    Check(lanes == 2, "exactly 2 stream lanes for streams {7,20} (got " + std::to_string(lanes) + ")");
    Check(json[json.find_last_not_of(" \n\t")] == '}', "ends with a closing brace");
    std::remove(path.c_str());
}

}  // namespace

int main() {
    TestOffsetApplied();
    TestSaturationNeverWraps();
    TestZeroOffsetIsUpstreamBehaviour();
    TestStreamLanesAndStructure();
    std::cout << (g_failures == 0 ? "\nALL PASS\n" : "\nFAILURES: ")
              << (g_failures == 0 ? "" : std::to_string(g_failures)) << "\n";
    return g_failures == 0 ? 0 : 1;
}
