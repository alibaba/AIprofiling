#ifndef CUPROF_NOTIFY_H
#define CUPROF_NOTIFY_H

#include <string>

namespace cuprof {

// Best-effort lifecycle notification: connects to a unix stream socket,
// writes `message`, closes. One connection per message so the receiver can
// match messages by equality without framing. Returns false on any failure;
// the profiler never depends on delivery.
bool Notify(const std::string& socket_path, const char* message);

}  // namespace cuprof

#endif  // CUPROF_NOTIFY_H
