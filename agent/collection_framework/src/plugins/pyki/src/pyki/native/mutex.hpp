#pragma once

#include <pthread.h>

namespace pyki {

class Mutex {
   public:
    Mutex();

    void lock();
    void unlock();

   protected:
    pthread_mutex_t _mutex;
};

class Monitor : public Mutex {
   public:
    Monitor();
    void wait();
    void notify();
    void notify_all();

   private:
    pthread_cond_t _cond;
};

class MutexLocker {
   public:
    MutexLocker(Mutex& mutex)
        : _mutex(mutex) {
        _mutex.lock();
    }

    ~MutexLocker() { _mutex.unlock(); }

   private:
    Mutex& _mutex;
};

class MonitorLocker {
   public:
    MonitorLocker(Monitor& monitor)
        : _monitor(monitor) {
        _monitor.lock();
    }

    ~MonitorLocker() { _monitor.unlock(); }

    void wait() { _monitor.wait(); }
    void notify() { _monitor.notify(); }
    void notify_all() { _monitor.notify_all(); }

   private:
    Monitor& _monitor;
};
}  // namespace pyki