# Makefile for injector_probe. Point LIBPROFILER_DIR at the dir that
# contains BOTH profiler.h and libprofiler.a.
#
# Example:
#   make LIBPROFILER_DIR=/path/to/libprofiler
#
# libprofiler.a pulls in binutils / dwarf. On glibc systems the runtime
# resolves those from libbfd / libdw. Adjust EXTRA_LIBS for build
# layouts if these aren't dynamically available.

LIBPROFILER_DIR ?= ../../agent/collection_framework/src/third_party/profiler
INCLUDE_DIR     ?= $(LIBPROFILER_DIR)/include
LIB_DIR         ?= $(LIBPROFILER_DIR)/src
LINK_SCRIPT     ?= $(LIBPROFILER_DIR)/link.ld

CC       ?= gcc
CFLAGS   ?= -O2 -g -Wall -Wextra -std=gnu11 -I$(INCLUDE_DIR)
# libprofiler.a defines a custom .thread_func_sec section (see link.ld);
# the injector loop reads __start_thread_func / __end_thread_func to know
# which function bodies to copy into the target's cloned thread. We must
# link with the same linker script or those symbols stay unresolved.
LDFLAGS  ?= -Wl,-T$(LINK_SCRIPT)
# libprofiler.a bundles its own elf/dwarf/unwind objects. In the default
# build only libpthread/libdl are needed at link time.
# If a given libprofiler.a strips those bundled objects and instead
# links dynamically against libbfd/libdw, extend EXTRA_LIBS accordingly.
EXTRA_LIBS ?= -lpthread -ldl

probe: probe.c $(LIB_DIR)/libprofiler.a
	$(CC) $(CFLAGS) $(LDFLAGS) probe.c $(LIB_DIR)/libprofiler.a $(EXTRA_LIBS) -o $@

clean:
	rm -f probe

.PHONY: clean
