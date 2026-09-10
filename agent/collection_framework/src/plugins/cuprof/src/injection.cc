#include "config.h"
#include "cupti_sink.h"

#include <pthread.h>
#include <unistd.h>

#include <cstdlib>

namespace {

void FlushAtExit() { cuprof::CuptiSink::Instance().Stop(); }

void* StopAfterDuration(void* arg) {
    unsigned secs = *static_cast<unsigned*>(arg);
    delete static_cast<unsigned*>(arg);
    sleep(secs);
    cuprof::CuptiSink::Instance().Stop();
    return NULL;
}

}  // namespace

// Starts collection. Returns 1 on success, 0 on failure.
//
// Runs on whichever thread triggers initialization, before any user kernel
// launches in the launch-mode path. External orchestrators call this directly
// after loading the library; see docs/embedding.md.
extern "C" int cuprof_start() {
    bool was_running = cuprof::CuptiSink::Instance().running();
    cuprof::Config cfg = cuprof::LoadConfig();
    if (!cuprof::CuptiSink::Instance().Start(cfg)) return 0;

    // Process-lifetime hooks are installed exactly once: when the same
    // loaded instance serves several collection windows in a row, repeated
    // atexit registrations would stack up and repeated duration timers
    // would stop later windows early. (Under -z nodelete the instance is
    // process-lifetime anyway; see docs/embedding.md.)
    static bool hooks_installed = false;
    if (!hooks_installed) {
        atexit(FlushAtExit);
        hooks_installed = true;
    }

    if (!was_running && cfg.duration_sec > 0) {
        pthread_t tid;
        unsigned* secs = new unsigned(cfg.duration_sec);
        if (pthread_create(&tid, NULL, StopAfterDuration, secs) == 0) {
            pthread_detach(tid);
        } else {
            delete secs;
        }
    }
    return 1;
}

// The CUDA driver looks up this exact symbol by name in the library named by
// CUDA_INJECTION64_PATH. Renaming it makes launch mode silently do nothing:
// the driver finds no entry point and reports no error. Keep it as a shim.
extern "C" int InitializeInjection() { return cuprof_start(); }
