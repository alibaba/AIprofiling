#pragma once

#include <map>
#include "type.hpp"
#include <stddef.h>

namespace pyki {
#define ROW_BITS 7
#define ROWS (1 << ROW_BITS)
#define CELLS 3
#define TABLE_CAPACITY (ROWS * CELLS)
#define INITIAL_BASE_INDEX (1 + TABLE_CAPACITY)

struct DictTable;

struct DictRow {
    char* keys[CELLS];
    u32 lengths[CELLS];
    u8 kinds[CELLS];

    DictTable* next;
};

struct DictTable {
    DictRow rows[ROWS];
    unsigned int base_index;

    unsigned int index(int row, int col) {
        return base_index + (col << ROW_BITS) + row;
    }
};

struct StringWithLength {
    const char* string;
    u32 length;
    u8 kind;
};

// Append-only concurrent hash table based on multi-level arrays
class Dictionary {
   private:
    DictTable* _table;
    volatile unsigned int _base_index;

    static void clear(DictTable* table);

    static unsigned int hash(const char* key, size_t length, u8 kind);

    static void collect(std::map<u32, StringWithLength>& map, DictTable* table);

   public:
    Dictionary();
    ~Dictionary();

    void clear();

    unsigned int lookup(const char* key, size_t length, u8 kind);

    void collect(std::map<u32, StringWithLength>& map);
};

}  // namespace pyki