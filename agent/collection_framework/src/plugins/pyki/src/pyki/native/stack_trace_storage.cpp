#include "stack_trace_storage.hpp"
#include "os.hpp"

namespace pyki {

static const u32 INITIAL_CAPACITY = 65536;
static const u32 CALL_TRACE_CHUNK = 8 * 1024 * 1024;
static const u32 STRING_CHUNK = 16 * 1024 * 1024;

struct StackTraceEntry {
    StackTrace* _trace;

    StackTrace* acquire_stack_trace() {
        return __atomic_load_n(&_trace, __ATOMIC_ACQUIRE);
    }

    void set_stack_trace(StackTrace* value) {
        return __atomic_store_n(&_trace, value, __ATOMIC_RELEASE);
    }
};

class LongHashTable {
   private:
    LongHashTable* _prev;
    void* _padding0;
    u32 _capacity;
    u32 _padding1[15];
    volatile u32 _size;
    u32 _padding2[15];

    static size_t getSize(u32 capacity) {
        size_t size = sizeof(LongHashTable) +
                      (sizeof(u64) + sizeof(StackTraceEntry)) * capacity;
        return (size + OS::page_mask) & ~OS::page_mask;
    }

   public:
    LongHashTable()
        : _prev(NULL),
          _padding0(NULL),
          _capacity(0),
          _size(0) {
        memset(_padding1, 0, sizeof(_padding1));
        memset(_padding2, 0, sizeof(_padding2));
    }

    static LongHashTable* allocate(LongHashTable* prev, u32 capacity) {
        LongHashTable* table =
            (LongHashTable*)OS::safe_alloc(getSize(capacity));
        if (table != NULL) {
            table->_prev = prev;
            table->_capacity = capacity;
            table->_size = 0;
        }
        return table;
    }

    LongHashTable* destroy() {
        LongHashTable* prev = _prev;
        OS::safe_free(this, getSize(_capacity));
        return prev;
    }

    size_t usedMemory() { return getSize(_capacity); }

    LongHashTable* prev() { return _prev; }

    u32 capacity() { return _capacity; }

    u32 size() { return _size; }

    u32 incSize() { return __sync_add_and_fetch(&_size, 1); }

    u64* keys() { return (u64*)(this + 1); }

    StackTraceEntry* values() { return (StackTraceEntry*)(keys() + _capacity); }

    void clear() {
        memset(keys(), 0, (sizeof(u64) + sizeof(StackTraceEntry)) * _capacity);
        _size = 0;
    }
};

static StackTrace _empty_stack_trace = {};

StackTraceStorage::StackTraceStorage()
    : _allocator(CALL_TRACE_CHUNK),
      _string_allocator(STRING_CHUNK) {
    _current_table = LongHashTable::allocate(NULL, INITIAL_CAPACITY);
    _overflow = 0;
}

StackTraceStorage::~StackTraceStorage() {
    while (_current_table != NULL) {
        _current_table = _current_table->destroy();
    }
}

void StackTraceStorage::clear() {
    while (_current_table->prev() != NULL) {
        _current_table = _current_table->destroy();
    }
    _current_table->clear();
    _allocator.clear();
    _string_allocator.clear();
    _overflow = 0;
}

// Adaptation of MurmurHash64A by Austin Appleby
u64 StackTraceStorage::calc_hash(int nof_python_frames,
                                 PythonFrame* python_frames,
                                 int nof_native_frames,
                                 NativeFrame* native_frames) {
    const u64 M = 0xc6a4a7935bd1e995ULL;
    const int R = 47;

    int python_size = nof_python_frames * sizeof(PythonFrame);
    int native_size = nof_native_frames * sizeof(NativeFrame);

    u64 h = (python_size + native_size) * M;

    const u64* data = (const u64*)python_frames;
    const u64* end = data + python_size / 8;

    while (data != end) {
        u64 k = *data++;
        k *= M;
        k ^= k >> R;
        k *= M;
        h ^= k;
        h *= M;
    }

    data = (const u64*)native_frames;
    end = data + native_size / 8;

    while (data != end) {
        u64 k = *data++;
        k *= M;
        k ^= k >> R;
        k *= M;
        h ^= k;
        h *= M;
    }

    h ^= h >> R;
    h *= M;
    h ^= h >> R;

    return h;
}

StackTrace* StackTraceStorage::store_stack_trace(int nof_python_frames,
                                                 PythonFrame* python_frames,
                                                 int nof_native_frames,
                                                 NativeFrame* native_frames) {
    StackTrace* buf = (StackTrace*)_allocator.alloc(
        sizeof(StackTrace) + nof_python_frames * sizeof(PythonFrame) +
        nof_native_frames * sizeof(NativeFrame));

    if (buf != NULL) {
        {
            size_t length = 0;
            buf->nof_python_frames = nof_python_frames;
            PythonFrame* dest = (PythonFrame*)((u8*)buf + sizeof(StackTrace));
            for (int i = 0; i < nof_python_frames; i++) {
                dest[i] = python_frames[i];
                Meta meta = Meta(python_frames[i].meta);

                length += meta.filename_size();
                length += meta.name_size();
            }
            char* s = (char*)_string_allocator.alloc(length);
            buf->string = s;
            if (s != NULL) {
                for (int i = 0; i < nof_python_frames; i++) {
                    Meta meta = Meta(python_frames[i].meta);
                    size_t length = meta.filename_size();
                    for (size_t j = 0; j < length; j++) {
                        *s++ =
                            ((const char*)python_frames[i].filename_address)[j];
                    }

                    length = meta.name_size();
                    for (size_t j = 0; j < length; j++) {
                        *s++ = ((const char*)python_frames[i].name_address)[j];
                    }
                }
            }
        }
        {
            buf->nof_native_frames = nof_native_frames;
            NativeFrame* dest =
                (NativeFrame*)((u8*)buf + sizeof(StackTrace) +
                               nof_python_frames * sizeof(PythonFrame));
            for (int i = 0; i < nof_native_frames; i++) {
                dest[i] = native_frames[i];
            }
        }
        return buf;
    }
    return buf;
}

StackTrace* StackTraceStorage::find_stack_trace(LongHashTable* table,
                                                u64 hash) {
    u64* keys = table->keys();
    u32 capacity = table->capacity();
    u32 slot = hash & (capacity - 1);
    u32 step = 0;

    while (keys[slot] != hash) {
        if (keys[slot] == 0) {
            return NULL;
        }
        if (++step >= capacity) {
            return NULL;
        }
        slot = (slot + step) & (capacity - 1);
    }

    return table->values()[slot]._trace;
}

u32 StackTraceStorage::put(int nof_python_frames, PythonFrame* python_frames,
                           int nof_native_frames, NativeFrame* native_frames) {
    u64 hash = calc_hash(nof_python_frames, python_frames, nof_native_frames,
                         native_frames);

    LongHashTable* table = _current_table;
    u64* keys = table->keys();
    u32 capacity = table->capacity();
    u32 slot = hash & (capacity - 1);
    u32 step = 0;

    while (keys[slot] != hash) {
        if (keys[slot] == 0) {
            if (!__sync_bool_compare_and_swap(&keys[slot], 0, hash)) {
                continue;
            }

            // Increment the table size, and if the load factor exceeds 0.75,
            // reserve a new table
            if (table->incSize() == capacity * 3 / 4) {
                LongHashTable* new_table =
                    LongHashTable::allocate(table, capacity * 2);
                if (new_table != NULL) {
                    __sync_bool_compare_and_swap(&_current_table, table,
                                                 new_table);
                }
            }

            // Migrate from a previous table to save space
            StackTrace* trace = table->prev() == NULL
                                    ? NULL
                                    : find_stack_trace(table->prev(), hash);
            if (trace == NULL) {
                trace = store_stack_trace(nof_python_frames, python_frames,
                                          nof_native_frames, native_frames);
            }
            table->values()[slot].set_stack_trace(trace);
            break;
        }

        if (++step >= capacity) {
            // Very unlikely case of a table overflow
            atomic_inc(_overflow);
            return OVERFLOW_STACK_TRACE_ID;
        }
        // Improved version of linear probing
        slot = (slot + step) & (capacity - 1);
    }

    return capacity - (INITIAL_CAPACITY - 1) + slot;
}

void StackTraceStorage::collect_stack_traces(std::map<u32, StackTrace*>& map) {
    for (LongHashTable* table = _current_table; table != NULL;
         table = table->prev()) {
        u64* keys = table->keys();
        StackTraceEntry* entries = table->values();
        u32 capacity = table->capacity();

        for (u32 slot = 0; slot < capacity; slot++) {
            if (keys[slot] != 0) {
                StackTrace* trace = entries[slot].acquire_stack_trace();
                if (trace != NULL) {
                    map[capacity - (INITIAL_CAPACITY - 1) + slot] = trace;
                }
            }
        }
    }

    map[EMPTY_STACK_TRACE_ID] = &_empty_stack_trace;
    if (_overflow > 0) {
        map[OVERFLOW_STACK_TRACE_ID] = &_empty_stack_trace;
    }
}

static StackTraceStorage* _storage = new StackTraceStorage();

StackTraceStorage* StackTraceStorage::instance() { return _storage; }

}  // namespace pyki
