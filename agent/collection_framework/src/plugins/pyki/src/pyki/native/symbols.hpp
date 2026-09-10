/*
 * Copyright The async-profiler authors
 * SPDX-License-Identifier: Apache-2.0
 */

#ifndef _SYMBOLS_H
#define _SYMBOLS_H

#include "codeCache.hpp"
#include "mutex.hpp"

namespace pyki {

class Symbols {
  private:
    static Mutex _parse_lock;
    static bool _have_kernel_symbols;
    static bool _libs_limit_reported;

    static CodeCacheArray _native_libs;

  public:
    static void parseKernelSymbols(CodeCache* cc);
    static void parseLibraries(bool kernel_symbols);

    static bool haveKernelSymbols() {
        return _have_kernel_symbols;
    }

    static CodeCacheArray& nativeLibs() {
        return _native_libs;
    }
};

class UnloadProtection {
  private:
    void* _lib_handle;
    bool _valid;

  public:
    UnloadProtection(const CodeCache *cc);
    ~UnloadProtection();

    UnloadProtection& operator=(const UnloadProtection& other) = delete;

    bool isValid() const { return _valid; }
};

} // namespace pyki

#endif // _SYMBOLS_H
