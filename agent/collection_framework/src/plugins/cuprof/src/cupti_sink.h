#ifndef CUPROF_CUPTI_SINK_H
#define CUPROF_CUPTI_SINK_H

#include "config.h"
#include "trace_writer.h"

#include <pthread.h>

#include <vector>

namespace cuprof {

// Owns the CUPTI activity subscription and the collected events.
//
// Lifetime: created once from cuprof_start(), flushed once at process
// exit (or after Config::duration_sec). Activity callbacks arrive on
// CUPTI-owned threads, so Append() must stay thread-safe.
class CuptiSink {
  public:
    static CuptiSink& Instance();

    // Enables activity collection. Calling it while already running is a
    // no-op that returns true. After a Stop(), Start() may be called again
    // to collect a fresh window (same loaded instance, new output file).
    bool Start(const Config& cfg);

    // Disables collection, drains CUPTI buffers, writes the trace file.
    // Idempotent; concurrent callers are serialized and only the first one
    // performs the stop sequence.
    void Stop();

    void Append(const Event& e);

    const Config& config() const { return cfg_; }
    bool running() const { return running_; }

  private:
    CuptiSink();
    ~CuptiSink();
    CuptiSink(const CuptiSink&);
    CuptiSink& operator=(const CuptiSink&);

    Config cfg_;
    bool running_;
    bool stopped_;
    pthread_mutex_t mu_;
    std::vector<Event> events_;
};

}  // namespace cuprof

#endif  // CUPROF_CUPTI_SINK_H
