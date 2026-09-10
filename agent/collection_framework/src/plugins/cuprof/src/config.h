// AIProf-local modification (Apache-2.0 4(b)): this file differs from
// upstream cuprof. See VENDOR.md and patches/0002-align-cupti-epoch-to-clock-monotonic.patch.
#ifndef CUPROF_CONFIG_H
#define CUPROF_CONFIG_H

#include <stdint.h>

#include <string>

namespace cuprof {

struct Config {
    std::string output;
    unsigned duration_sec;
    bool verbose;
    // Optional unix socket to notify about lifecycle transitions. Empty
    // disables notification. See docs/embedding.md.
    std::string socket_path;
    // Offset (ns) to subtract from CUPTI epoch timestamps to align with
    // CLOCK_MONOTONIC (which PyTorch profiler / pyki uses). Computed once
    // at start as (CLOCK_REALTIME - CLOCK_MONOTONIC).
    uint64_t epoch_to_mono_offset_ns;
};

// Configuration sources, in priority order:
//   1. CUPROF_* environment variables (the `cuprof run` path)
//   2. /tmp/cuprof_<pid>.cfg with KEY=VALUE lines (the embedded path, where
//      an external tool injects libcuprof.so into a process whose
//      environment it cannot change)
// Never fails: unset or malformed values fall back to defaults.
Config LoadConfig();

}  // namespace cuprof

#endif  // CUPROF_CONFIG_H
