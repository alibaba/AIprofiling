#pragma once

#include <stddef.h>

namespace pyki {

struct Chunk {
    Chunk* prev;
    volatile size_t offs;
    // To avoid false sharing
    char _padding[56];
};

class LinearAllocator {
   private:
    size_t _chunk_size;
    Chunk* _tail;
    Chunk* _reserve;

    Chunk* allocateChunk(Chunk* current);
    void freeChunk(Chunk* current);
    void reserveChunk(Chunk* current);
    Chunk* getNextChunk(Chunk* current);

   public:
    LinearAllocator(size_t chunk_size);
    ~LinearAllocator();

    void clear();
    size_t usedMemory();

    void* alloc(size_t size);
};

}  // namespace pyki