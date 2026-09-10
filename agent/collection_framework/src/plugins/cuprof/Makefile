# AIProf-local modification (Apache-2.0 4(b)): this file differs from
# upstream cuprof. See VENDOR.md and patches/0001-rpath-origin-for-ptrace-injection.patch.
# cuprof - minimal CUPTI kernel timeline profiler
#
# Build artifacts:
#   build/libcuprof.so   injected library (CUDA_INJECTION64_PATH target)
#   build/cuprof         CLI launcher

CUDA_HOME ?= /usr/local/cuda
CUPTI_HOME ?= $(CUDA_HOME)/extras/CUPTI
PREFIX ?= /usr/local

CXX ?= g++
CXXFLAGS ?= -O2 -g -std=c++11 -Wall -Wextra -fPIC
CPPFLAGS += -I$(CUPTI_HOME)/include -I$(CUDA_HOME)/include

BUILD := build
LIB := $(BUILD)/libcuprof.so
CLI := $(BUILD)/cuprof

LIB_SRCS := src/injection.cc src/cupti_sink.cc src/trace_writer.cc src/config.cc src/notify.cc
LIB_OBJS := $(LIB_SRCS:src/%.cc=$(BUILD)/%.o)

# Only InitializeInjection is exported; everything else stays local so we can
# never clash with symbols in the target process.
#
# RPATH matters: the CUDA driver dlopen()s this library, and a failed dlopen is
# completely silent — no error, no collection.
#
# $ORIGIN comes first so an injected copy resolves libcupti.so.12 from its own
# directory: the injector ptrace-dlopen()s this library inside the *target*
# process, where the build-time CUPTI path may not exist (different CUDA
# version, or a containerised build vs. bare-metal target). The absolute
# CUPTI_HOME path stays as a fallback for the normal
# CUDA_INJECTION64_PATH flow on the build host.
#
# -z nodelete: CUPTI keeps holding pointers into this library's code for the
# life of the process (activity buffer callbacks; there is no per-callback
# unregister API), so unloading it is never safe. Some orchestrators still
# dlclose() after collection; nodelete turns that dlclose into a refcount
# decrement that never unmaps, keeping those pointers valid. This is defense
# in depth against the dlclose-unmap variant only — it does nothing about a
# truncated or overwritten backing file, which the contract in
# docs/embedding.md forbids instead.
LDFLAGS_LIB := -shared -Wl,--version-script=export.map -Wl,--no-undefined \
               -Wl,-rpath,'$$ORIGIN' -Wl,-rpath,$(CUPTI_HOME)/lib64 \
               -Wl,-z,nodelete
LDLIBS_LIB := -L$(CUPTI_HOME)/lib64 -lcupti -lpthread

.PHONY: all clean install examples

all: $(LIB) $(CLI)

$(BUILD):
	@mkdir -p $(BUILD)

$(BUILD)/%.o: src/%.cc | $(BUILD)
	$(CXX) $(CXXFLAGS) $(CPPFLAGS) -c $< -o $@

$(LIB): $(LIB_OBJS) export.map
	$(CXX) $(CXXFLAGS) $(LDFLAGS_LIB) $(LIB_OBJS) -o $@ $(LDLIBS_LIB)

$(CLI): src/main.cc | $(BUILD)
	$(CXX) $(CXXFLAGS) $< -o $@

examples: $(BUILD)/vector_add

$(BUILD)/vector_add: examples/vector_add.cu | $(BUILD)
	$(CUDA_HOME)/bin/nvcc -O2 -o $@ $<

install: all
	install -d $(DESTDIR)$(PREFIX)/bin $(DESTDIR)$(PREFIX)/lib
	install -m 755 $(CLI) $(DESTDIR)$(PREFIX)/bin/cuprof
	install -m 644 $(LIB) $(DESTDIR)$(PREFIX)/lib/libcuprof.so

clean:
	rm -rf $(BUILD)
