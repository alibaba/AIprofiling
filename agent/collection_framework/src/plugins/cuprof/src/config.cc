// AIProf-local modification (Apache-2.0 4(b)): this file differs from
// upstream cuprof. See VENDOR.md and patches/0002-align-cupti-epoch-to-clock-monotonic.patch.
#include "config.h"

#include <time.h>
#include <unistd.h>

#include <cstdlib>
#include <fstream>
#include <map>
#include <sstream>

namespace cuprof {
namespace {

std::string DefaultOutput() {
    std::ostringstream os;
    os << "cuprof_" << getpid() << ".json";
    return os.str();
}

// KEY=VALUE per line; '#' starts a comment; whitespace-trimmed.
std::map<std::string, std::string> ReadCfgFile(const std::string& path) {
    std::map<std::string, std::string> kv;
    std::ifstream f(path.c_str());
    std::string line;
    while (std::getline(f, line)) {
        size_t hash = line.find('#');
        if (hash != std::string::npos) line.erase(hash);
        size_t eq = line.find('=');
        if (eq == std::string::npos) continue;
        std::string k = line.substr(0, eq);
        std::string v = line.substr(eq + 1);
        k.erase(0, k.find_first_not_of(" \t"));
        k.erase(k.find_last_not_of(" \t\r") + 1);
        v.erase(0, v.find_first_not_of(" \t"));
        v.erase(v.find_last_not_of(" \t\r") + 1);
        if (!k.empty()) kv[k] = v;
    }
    return kv;
}

std::map<std::string, std::string> LoadCfgFile() {
    const char* p = getenv("CUPROF_CONFIG");
    if (p && *p) return ReadCfgFile(p);

    std::ostringstream os;
    os << "/tmp/cuprof_" << getpid() << ".cfg";
    std::map<std::string, std::string> kv = ReadCfgFile(os.str());
    // An external tool may not know the in-namespace pid; a pid-less path
    // serves the one-process-per-container case.
    if (kv.empty()) kv = ReadCfgFile("/tmp/cuprof.cfg");
    return kv;
}

}  // namespace

Config LoadConfig() {
    std::map<std::string, std::string> kv = LoadCfgFile();

    // Environment wins over the file; the file covers processes whose
    // environment the launcher could not touch.
    const char* keys[] = {"CUPROF_OUTPUT", "CUPROF_DURATION", "CUPROF_VERBOSE",
                          "CUPROF_SOCKET"};
    for (size_t i = 0; i < sizeof(keys) / sizeof(keys[0]); ++i) {
        const char* v = getenv(keys[i]);
        if (v && *v) kv[keys[i]] = v;
    }

    Config c;
    c.output = kv.count("CUPROF_OUTPUT") ? kv["CUPROF_OUTPUT"] : DefaultOutput();
    c.duration_sec = kv.count("CUPROF_DURATION")
                         ? static_cast<unsigned>(strtoul(kv["CUPROF_DURATION"].c_str(), NULL, 10))
                         : 0;
    c.verbose = kv.count("CUPROF_VERBOSE") && strtol(kv["CUPROF_VERBOSE"].c_str(), NULL, 10) != 0;
    c.socket_path = kv.count("CUPROF_SOCKET") ? kv["CUPROF_SOCKET"] : "";

    // CUPTI timestamps are epoch (CLOCK_REALTIME) nanoseconds. PyTorch
    // profiler (pyki) uses CLOCK_MONOTONIC. To align them on the same
    // timeline in Perfetto/chrome-trace viewers, subtract the difference.
    struct timespec rt, mt;
    clock_gettime(CLOCK_REALTIME, &rt);
    clock_gettime(CLOCK_MONOTONIC, &mt);
    uint64_t rt_ns = static_cast<uint64_t>(rt.tv_sec) * 1000000000ULL + rt.tv_nsec;
    uint64_t mt_ns = static_cast<uint64_t>(mt.tv_sec) * 1000000000ULL + mt.tv_nsec;
    c.epoch_to_mono_offset_ns = rt_ns - mt_ns;

    return c;
}

}  // namespace cuprof
