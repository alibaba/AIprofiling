#pragma once

#include "type.hpp"
#include <cassert>
#include <endian.h>

namespace pyki {

const int BUFFER_SIZE = 65536 - sizeof(int);
const int MAX_STRING_LENGTH = 2048;

class Buffer {
   private:
    int _offset;
    char _data[BUFFER_SIZE];

   public:
    const char* data() const { return _data; }

    int offset() const { return _offset; }

    int remaining() const {
        assert(_offset <= BUFFER_SIZE);
        return BUFFER_SIZE - _offset;
    }

    void reset() { _offset = 0; }

    void put_8(u8 v) {
        assert(_offset + 1 <= BUFFER_SIZE);
        _data[_offset++] = v;
    }

    void put_16(u16 v) {
        assert(remaining() >= 2);
        *(u16*)(_data + _offset) = htole16(v);
        _offset += 2;
    }

    void put_32(u32 v) {
        assert(remaining() >= 4);
        *(u32*)(_data + _offset) = htole32(v);
        _offset += 4;
    }

    void put_var32(u32 v) {
        assert(remaining() >= 5);
        while (v > 0x7f) {
            _data[_offset++] = (u8)(v | 0x80);
            v >>= 7;
        }
        _data[_offset++] = (u8)v;
    }

    void put_64(u64 v) {
        assert(remaining() >= 8);
        *(u64*)(_data + _offset) = htole64(v);
        _offset += 8;
    }

    void put_var64(u64 v) {
        assert(remaining() >= 9);
        int iter = 0;
        while (v > 0x1fffff) {
            _data[_offset++] = (u8)v | 0x80;
            v >>= 7;
            _data[_offset++] = (u8)v | 0x80;
            v >>= 7;
            _data[_offset++] = (u8)v | 0x80;
            v >>= 7;
            if (++iter == 3)
                return;
        }
        while (v > 0x7f) {
            _data[_offset++] = (u8)v | 0x80;
            v >>= 7;
        }
        _data[_offset++] = (u8)v;
    }

    void put(const char* s, int len) {
        assert(len <= MAX_STRING_LENGTH);
        assert(remaining() >= 4 + len);
        memcpy(_data + _offset, s, len);
        _offset += (int)len;
    }

    void put_string(const char* s, int len) {
        assert(len <= MAX_STRING_LENGTH);
        assert(remaining() >= 5 + len);
        put_var32((u32)len);
        memcpy(_data + _offset, s, len);
        _offset += (int)len;
    }
};

}  // namespace pyki