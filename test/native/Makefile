# Native (GPU-free) regression test for the vendored cuprof patch series.
#
# Only trace_writer.cc is compiled in: it has no CUDA/CUPTI dependency, so the
# timestamp-alignment patch (patches/0002-*.patch) can be verified on any Linux
# host with a C++11 compiler. The rest of cuprof needs a GPU.
#
#   make            # build + run
#   make clean

CXX ?= g++
CXXFLAGS ?= -O2 -g -std=c++11 -Wall -Wextra

CUPROF := ../../agent/collection_framework/src/plugins/cuprof
BIN := test_cuprof_trace_writer

.PHONY: all run clean
all: run

$(BIN): test_cuprof_trace_writer.cc $(CUPROF)/src/trace_writer.cc $(CUPROF)/src/trace_writer.h
	$(CXX) $(CXXFLAGS) -I$(CUPROF)/src test_cuprof_trace_writer.cc $(CUPROF)/src/trace_writer.cc -o $@

run: $(BIN)
	./$(BIN)

clean:
	rm -f $(BIN)
