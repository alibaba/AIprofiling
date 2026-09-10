#pragma once

#include "pyki.hpp"
#include "os.hpp"

#include <sys/syscall.h>

namespace pyki {

class SpinLock {
   private:
    volatile int _lock;

   public:
    SpinLock()
        : _lock(0) {}

    void lock() {
        while (!try_lock()) {
            __asm__ __volatile__("pause");
        }
    }

    bool try_lock() { return __sync_bool_compare_and_swap(&_lock, 0, 1); }

    void unlock() { __sync_fetch_and_sub(&_lock, 1); }
};

class SpinLockSet {
   private:
    SpinLock _locks[CONCURRENCY_LEVEL];

   public:
    SpinLock& operator[](int index) { return _locks[index]; }
};

extern SpinLockSet TopLevelSpinLockSet;

class ThreadCritical {
   private:
    SpinLockSet& _lock_set;
    int _index;
    const int _thread_id;
    bool _all_locked;

   private:
    bool try_lock(int index_start) {
        int i = index_start;
        if (_lock_set[i].try_lock() ||
            _lock_set[i = (i + 1) % CONCURRENCY_LEVEL].try_lock() ||
            _lock_set[i = (i + 2) % CONCURRENCY_LEVEL].try_lock()) {
            _index = i;
            return true;
        }
        return false;
    }

   public:
    ThreadCritical(SpinLockSet& lock_set = TopLevelSpinLockSet)
        : _lock_set(lock_set),
          _index(-1),
          _thread_id(OS::thread_id()),
          _all_locked(false) {}

    void enter() {
        int i = get_thread_concurrency_level_index(_thread_id);
        while (!try_lock(i)) {
        }
    }

    bool try_enter() {
        return try_lock(get_thread_concurrency_level_index(_thread_id));
    }

    void lock_all() {
        for (int i = 0; i < CONCURRENCY_LEVEL; i++) {
            _lock_set[i].lock();
        }
        _all_locked = true;
    }

    int get_index() const { return _index; }

    int get_thread_id() const { return _thread_id; }

    ~ThreadCritical() {
        if (_all_locked) {
            for (int i = 0; i < CONCURRENCY_LEVEL; i++) {
                _lock_set[i].unlock();
            }
        }
        else if (_index != -1) {
            _lock_set[_index].unlock();
        }
    }
};

}  // namespace pyki