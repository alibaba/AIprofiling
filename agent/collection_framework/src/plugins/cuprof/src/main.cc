// cuprof CLI: sets up the injection environment, then execs the target.
//
// The profiler itself lives in libcuprof.so; this binary only decides where
// that library is and which CUPROF_* variables to export.

#include <libgen.h>
#include <limits.h>
#include <unistd.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cerrno>
#include <string>
#include <vector>

namespace {

const char* kUsage =
    "cuprof - minimal CUDA kernel timeline profiler\n"
    "\n"
    "Usage:\n"
    "  cuprof run [options] -- <program> [args...]\n"
    "\n"
    "Options:\n"
    "  -o, --output PATH    trace output file (default ./cuprof_<pid>.json)\n"
    "  -d, --duration SEC   stop collecting after SEC seconds (default: at exit)\n"
    "  -q, --quiet          suppress profiler logging\n"
    "  -v, --verbose        log profiler activity to stderr\n"
    "  -h, --help           show this message\n"
    "\n"
    "Environment:\n"
    "  CUPROF_LIB           override the path to libcuprof.so\n"
    "\n"
    "Example:\n"
    "  cuprof run -o trace.json -- python train.py\n";

// Looks for libcuprof.so next to this executable first (build tree / relocated
// install), then falls back to the loader's search path.
std::string LocateLibrary() {
    const char* override_path = getenv("CUPROF_LIB");
    if (override_path && *override_path) return override_path;

    char exe[PATH_MAX];
    ssize_t n = readlink("/proc/self/exe", exe, sizeof(exe) - 1);
    if (n > 0) {
        exe[n] = '\0';
        std::string dir(dirname(exe));
        const char* candidates[] = {"/libcuprof.so", "/../lib/libcuprof.so"};
        for (size_t i = 0; i < sizeof(candidates) / sizeof(candidates[0]); ++i) {
            std::string p = dir + candidates[i];
            if (access(p.c_str(), R_OK) == 0) return p;
        }
    }
    return "libcuprof.so";
}

}  // namespace

int main(int argc, char** argv) {
    if (argc < 2 || strcmp(argv[1], "-h") == 0 || strcmp(argv[1], "--help") == 0) {
        fputs(kUsage, stdout);
        return argc < 2 ? 2 : 0;
    }
    if (strcmp(argv[1], "run") != 0) {
        fprintf(stderr, "cuprof: unknown command '%s'\n\n%s", argv[1], kUsage);
        return 2;
    }

    std::string output;
    std::string duration;
    const char* verbose = "0";
    int i = 2;
    for (; i < argc; ++i) {
        const char* a = argv[i];
        if (strcmp(a, "--") == 0) {
            ++i;
            break;
        } else if ((strcmp(a, "-o") == 0 || strcmp(a, "--output") == 0) && i + 1 < argc) {
            output = argv[++i];
        } else if ((strcmp(a, "-d") == 0 || strcmp(a, "--duration") == 0) && i + 1 < argc) {
            duration = argv[++i];
        } else if (strcmp(a, "-q") == 0 || strcmp(a, "--quiet") == 0) {
            verbose = "0";
        } else if (strcmp(a, "-v") == 0 || strcmp(a, "--verbose") == 0) {
            verbose = "1";
        } else if (a[0] != '-') {
            // First non-flag argument starts the target command; '--' is only
            // needed when the target itself begins with '-'.
            break;
        } else {
            fprintf(stderr, "cuprof: unknown option '%s'\n\n%s", a, kUsage);
            return 2;
        }
    }

    if (i >= argc) {
        fprintf(stderr, "cuprof: no program given (did you forget '--'?)\n\n%s", kUsage);
        return 2;
    }

    std::string lib = LocateLibrary();
    setenv("CUDA_INJECTION64_PATH", lib.c_str(), 1);
    setenv("CUPROF_VERBOSE", verbose, 1);
    if (!output.empty()) setenv("CUPROF_OUTPUT", output.c_str(), 1);
    if (!duration.empty()) setenv("CUPROF_DURATION", duration.c_str(), 1);

    execvp(argv[i], &argv[i]);
    fprintf(stderr, "cuprof: cannot execute '%s': %s\n", argv[i], strerror(errno));
    return 127;
}
