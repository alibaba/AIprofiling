#include "mutex.hpp"

namespace pyki {

Mutex::Mutex() {
    pthread_mutexattr_t attr;
    pthread_mutexattr_init(&attr);
    pthread_mutexattr_settype(&attr, PTHREAD_MUTEX_RECURSIVE);
    pthread_mutex_init(&_mutex, &attr);
}

void Mutex::lock() { pthread_mutex_lock(&_mutex); }

void Mutex::unlock() { pthread_mutex_unlock(&_mutex); }

Monitor::Monitor()
    : Mutex() {
    pthread_cond_init(&_cond, nullptr);
}

void Monitor::wait() { pthread_cond_wait(&_cond, &_mutex); }

void Monitor::notify() { pthread_cond_signal(&_cond); }

void Monitor::notify_all() { pthread_cond_broadcast(&_cond); }

}  // namespace pyki