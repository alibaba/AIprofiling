#pragma once

#include <set>
#include <string>

#include <dlfcn.h>

namespace pyki::linker {

static void
_dummy(void){};

class SymbolPatcher
{
  private:
    std::set<std::string> symbols;
    std::string self_so_name = "_pyki.cpython-";

  public:
    SymbolPatcher()
    {
        Dl_info info;
        if (dladdr((void*)&_dummy, &info)) {
            self_so_name = info.dli_fname;
        }
    }
    void overwrite_symbols() noexcept;
    void restore_symbols() noexcept;
};
}  // namespace pyki::linker
