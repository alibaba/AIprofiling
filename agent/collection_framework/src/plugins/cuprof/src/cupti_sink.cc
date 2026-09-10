// AIProf-local modification (Apache-2.0 4(b)): this file differs from
// upstream cuprof. See VENDOR.md and patches/0002-align-cupti-epoch-to-clock-monotonic.patch.
#include "cupti_sink.h"

#include "notify.h"

#include <cupti.h>
#include <cxxabi.h>

#include <cstdio>
#include <cstdarg>
#include <cstdlib>
#include <cstring>
#include <sstream>

namespace cuprof {
namespace {

// CUPTI hands us raw buffers to fill; 8-byte alignment is required.
const size_t kBufferSize = 8 * 1024 * 1024;
const size_t kAlign = 8;

bool Verbose() { return CuptiSink::Instance().config().verbose; }

void Logf(const char* fmt, ...) {
    if (!Verbose()) return;
    va_list ap;
    va_start(ap, fmt);
    fputs("[cuprof] ", stderr);
    vfprintf(stderr, fmt, ap);
    fputc('\n', stderr);
    va_end(ap);
}

std::string Demangle(const char* name) {
    if (!name) return "<null>";
    int status = 0;
    char* out = abi::__cxa_demangle(name, NULL, NULL, &status);
    if (status == 0 && out) {
        std::string s(out);
        free(out);
        return s;
    }
    if (out) free(out);
    return name;
}

std::string Dims(uint32_t x, uint32_t y, uint32_t z) {
    std::ostringstream os;
    os << x << "," << y << "," << z;
    return os.str();
}

const char* MemcpyKindName(uint8_t kind) {
    switch (kind) {
        case CUPTI_ACTIVITY_MEMCPY_KIND_HTOD: return "Memcpy HtoD";
        case CUPTI_ACTIVITY_MEMCPY_KIND_DTOH: return "Memcpy DtoH";
        case CUPTI_ACTIVITY_MEMCPY_KIND_DTOD: return "Memcpy DtoD";
        case CUPTI_ACTIVITY_MEMCPY_KIND_HTOH: return "Memcpy HtoH";
        case CUPTI_ACTIVITY_MEMCPY_KIND_ATOA: return "Memcpy AtoA";
        case CUPTI_ACTIVITY_MEMCPY_KIND_ATOH: return "Memcpy AtoH";
        case CUPTI_ACTIVITY_MEMCPY_KIND_ATOD: return "Memcpy AtoD";
        case CUPTI_ACTIVITY_MEMCPY_KIND_DTOA: return "Memcpy DtoA";
        case CUPTI_ACTIVITY_MEMCPY_KIND_HTOA: return "Memcpy HtoA";
        default: return "Memcpy";
    }
}

std::string ToStr(uint64_t v) {
    std::ostringstream os;
    os << v;
    return os.str();
}

void IngestRecord(CUpti_Activity* rec) {
    switch (rec->kind) {
        case CUPTI_ACTIVITY_KIND_CONCURRENT_KERNEL:
        case CUPTI_ACTIVITY_KIND_KERNEL: {
            CUpti_ActivityKernel9* k = reinterpret_cast<CUpti_ActivityKernel9*>(rec);
            // The forced flush at Stop() can deliver records for work that
            // was still running; those have no end timestamp. Drop them
            // instead of letting the unsigned duration wrap into a
            // multi-century event in the trace.
            if (k->end == 0 || k->end < k->start) break;
            Event e;
            e.name = Demangle(k->name);
            e.category = "kernel";
            e.start_ns = k->start;
            e.end_ns = k->end;
            e.stream_id = k->streamId;
            e.device_id = k->deviceId;
            e.correlation_id = k->correlationId;
            e.args.push_back(std::make_pair("grid", Dims(k->gridX, k->gridY, k->gridZ)));
            e.args.push_back(std::make_pair("block", Dims(k->blockX, k->blockY, k->blockZ)));
            e.args.push_back(
                std::make_pair("registers_per_thread", ToStr(k->registersPerThread)));
            e.args.push_back(
                std::make_pair("static_shared_mem", ToStr(k->staticSharedMemory)));
            e.args.push_back(
                std::make_pair("dynamic_shared_mem", ToStr(k->dynamicSharedMemory)));
            CuptiSink::Instance().Append(e);
            break;
        }
        case CUPTI_ACTIVITY_KIND_MEMCPY: {
            CUpti_ActivityMemcpy5* m = reinterpret_cast<CUpti_ActivityMemcpy5*>(rec);
            if (m->end == 0 || m->end < m->start) break;
            Event e;
            e.name = MemcpyKindName(m->copyKind);
            e.category = "memcpy";
            e.start_ns = m->start;
            e.end_ns = m->end;
            e.stream_id = m->streamId;
            e.device_id = m->deviceId;
            e.correlation_id = m->correlationId;
            e.args.push_back(std::make_pair("bytes", ToStr(m->bytes)));
            CuptiSink::Instance().Append(e);
            break;
        }
        case CUPTI_ACTIVITY_KIND_MEMSET: {
            CUpti_ActivityMemset4* m = reinterpret_cast<CUpti_ActivityMemset4*>(rec);
            if (m->end == 0 || m->end < m->start) break;
            Event e;
            e.name = "Memset";
            e.category = "memset";
            e.start_ns = m->start;
            e.end_ns = m->end;
            e.stream_id = m->streamId;
            e.device_id = m->deviceId;
            e.correlation_id = m->correlationId;
            e.args.push_back(std::make_pair("bytes", ToStr(m->bytes)));
            CuptiSink::Instance().Append(e);
            break;
        }
        default:
            break;
    }
}

void CUPTIAPI OnBufferRequest(uint8_t** buffer, size_t* size, size_t* max_num_records) {
    void* p = NULL;
    if (posix_memalign(&p, kAlign, kBufferSize) != 0) {
        *buffer = NULL;
        *size = 0;
        *max_num_records = 0;
        return;
    }
    *buffer = static_cast<uint8_t*>(p);
    *size = kBufferSize;
    // 0 means "fill the buffer as full as possible".
    *max_num_records = 0;
}

void CUPTIAPI OnBufferFilled(CUcontext ctx, uint32_t streamId, uint8_t* buffer, size_t, size_t valid_size) {
    if (valid_size > 0) {
        CUpti_Activity* rec = NULL;
        for (;;) {
            CUptiResult r = cuptiActivityGetNextRecord(buffer, valid_size, &rec);
            if (r == CUPTI_SUCCESS) {
                IngestRecord(rec);
            } else if (r == CUPTI_ERROR_MAX_LIMIT_REACHED) {
                break;
            } else {
                Logf("cuptiActivityGetNextRecord failed: %d", static_cast<int>(r));
                break;
            }
        }
    }

    size_t dropped = 0;
    if (cuptiActivityGetNumDroppedRecords(ctx, streamId, &dropped) == CUPTI_SUCCESS && dropped > 0) {
        Logf("dropped %zu records (buffer pressure)", dropped);
    }
    free(buffer);
}

}  // namespace

CuptiSink& CuptiSink::Instance() {
    static CuptiSink instance;
    return instance;
}

CuptiSink::CuptiSink() : running_(false), stopped_(false) {
    pthread_mutex_init(&mu_, NULL);
}

CuptiSink::~CuptiSink() { pthread_mutex_destroy(&mu_); }

void CuptiSink::Append(const Event& e) {
    pthread_mutex_lock(&mu_);
    events_.push_back(e);
    pthread_mutex_unlock(&mu_);
}

bool CuptiSink::Start(const Config& cfg) {
    pthread_mutex_lock(&mu_);
    if (running_) {
        pthread_mutex_unlock(&mu_);
        return true;
    }
    cfg_ = cfg;
    stopped_ = false;

    // CUPTI holds these buffer-callback pointers for the life of the process
    // (there is no per-callback unregister API), and they never change
    // between collection windows, so register them exactly once even when
    // the same loaded instance serves several windows in a row.
    static bool registered = false;
    if (!registered) {
        CUptiResult r = cuptiActivityRegisterCallbacks(OnBufferRequest, OnBufferFilled);
        if (r != CUPTI_SUCCESS) {
            pthread_mutex_unlock(&mu_);
            Logf("cuptiActivityRegisterCallbacks failed: %d", static_cast<int>(r));
            return false;
        }
        registered = true;
    }

    const CUpti_ActivityKind kinds[] = {
        CUPTI_ACTIVITY_KIND_CONCURRENT_KERNEL,
        CUPTI_ACTIVITY_KIND_MEMCPY,
        CUPTI_ACTIVITY_KIND_MEMSET,
    };
    bool any = false;
    for (size_t i = 0; i < sizeof(kinds) / sizeof(kinds[0]); ++i) {
        CUptiResult er = cuptiActivityEnable(kinds[i]);
        if (er == CUPTI_SUCCESS) {
            any = true;
        } else {
            Logf("cuptiActivityEnable(%d) failed: %d", static_cast<int>(kinds[i]),
                 static_cast<int>(er));
        }
    }
    if (!any) {
        pthread_mutex_unlock(&mu_);
        return false;
    }

    running_ = true;
    pthread_mutex_unlock(&mu_);
    Notify(cfg.socket_path, "CUPTIProfilingStart");
    Logf("collecting -> %s", cfg.output.c_str());
    return true;
}

void CuptiSink::Stop() {
    pthread_mutex_lock(&mu_);
    if (!running_ || stopped_) {
        pthread_mutex_unlock(&mu_);
        return;
    }
    stopped_ = true;
    running_ = false;
    pthread_mutex_unlock(&mu_);

    cuptiActivityDisable(CUPTI_ACTIVITY_KIND_CONCURRENT_KERNEL);
    cuptiActivityDisable(CUPTI_ACTIVITY_KIND_MEMCPY);
    cuptiActivityDisable(CUPTI_ACTIVITY_KIND_MEMSET);
    // Forced flush: without this, records still sitting in CUPTI's internal
    // buffers never reach OnBufferFilled. The flush may include records for
    // still-running work; IngestRecord drops those.
    cuptiActivityFlushAll(1);

    Notify(cfg_.socket_path, "CUPTIProfilingStop");

    pthread_mutex_lock(&mu_);
    std::vector<Event> snapshot;
    snapshot.swap(events_);  // leave the sink empty for the next window
    pthread_mutex_unlock(&mu_);

    if (!WriteChromeTrace(cfg_.output, snapshot, cfg_.epoch_to_mono_offset_ns)) {
        fprintf(stderr, "[cuprof] failed to write %s\n", cfg_.output.c_str());
        // Without this, an orchestrator waiting for WriterOver would block
        // forever; Failed is the only terminal message that can replace it.
        Notify(cfg_.socket_path, "CUPTIProfilingFailed");
        return;
    }
    // "WriterOver" is the consumable-file signal; anything watching the
    // socket must key off this, not off Stop.
    Notify(cfg_.socket_path, "CUPTIProfilingWriterOver");
    fprintf(stderr, "[cuprof] wrote %zu events to %s\n", snapshot.size(), cfg_.output.c_str());
}

}  // namespace cuprof
