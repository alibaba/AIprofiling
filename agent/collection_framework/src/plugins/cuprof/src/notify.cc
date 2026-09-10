#include "notify.h"

#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <cstring>

namespace cuprof {

bool Notify(const std::string& socket_path, const char* message) {
    if (socket_path.empty()) return false;

    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) return false;

    sockaddr_un addr;
    memset(&addr, 0, sizeof(addr));
    addr.sun_family = AF_UNIX;
    if (socket_path.size() >= sizeof(addr.sun_path)) {
        close(fd);
        return false;
    }
    strncpy(addr.sun_path, socket_path.c_str(), sizeof(addr.sun_path) - 1);

    bool ok = false;
    if (connect(fd, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) == 0) {
        size_t len = strlen(message);
        ok = write(fd, message, len) == static_cast<ssize_t>(len);
    }
    close(fd);
    return ok;
}

}  // namespace cuprof
