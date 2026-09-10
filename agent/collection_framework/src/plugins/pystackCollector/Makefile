# Makefile for Python Function Collector Static Library

# Compilers and tools
CXX := g++
AR := ar
RANLIB := ranlib

# Directory layout
SRC_DIR := src
INCLUDE_DIR := include
BUILD_DIR := build

# Source files
CPP_SOURCES := $(SRC_DIR)/profiler_python.cpp

CONDA_ENV ?=
ifdef CONDA_ENV
    PYTHON_BIN := $(CONDA_ENV)/bin/python3
    PYTHON_CONFIG := $(CONDA_ENV)/bin/python3-config
    PYTHON_INCLUDES := $(shell $(PYTHON_CONFIG) --includes 2>/dev/null || \
                        echo "-I$(CONDA_ENV)/include/python3.10")
    PYBIND11_INCLUDES := $(shell $(PYTHON_BIN) -c "import pybind11; print('-I' + pybind11.get_include())" 2>/dev/null)
else
    # Original auto-detection logic, kept as-is
    PYTHON_INCLUDES := $(shell python3-config --includes 2>/dev/null || \
                        python3.12-config --includes 2>/dev/null || \
                        python3.11-config --includes 2>/dev/null || \
                        python3.10-config --includes 2>/dev/null || \
                        python3.9-config --includes 2>/dev/null || \
                        echo "-I/usr/include/python3.8 -I/usr/include/python3.8")

    PYBIND11_INCLUDES := $(shell python3 -c "import pybind11; print('-I' + pybind11.get_include())" 2>/dev/null || \
                         echo "-I/usr/include/pybind11")
endif

# Include directories
INCLUDES := -I$(INCLUDE_DIR) $(PYTHON_INCLUDES) $(PYBIND11_INCLUDES)

# Compile flags
CXXFLAGS := -std=c++17 -fPIC -O2 -Wall -Wextra -DNDEBUG
CPPFLAGS := $(INCLUDES)

# Output static library
STATIC_LIB := libpystackcollector.a
TARGET := $(BUILD_DIR)/$(STATIC_LIB)

# Object files
OBJECTS := $(BUILD_DIR)/profiler_python.o

# Default target
.PHONY: all clean install

all: $(TARGET)

# Create build directory
$(BUILD_DIR):
	mkdir -p $(BUILD_DIR)

# Compile sources
$(BUILD_DIR)/%.o: $(SRC_DIR)/%.cpp | $(BUILD_DIR)
	$(CXX) $(CXXFLAGS) $(CPPFLAGS) -c $< -o $@

# Build the static library
$(TARGET): $(OBJECTS)
	$(AR) rcs $@ $^
	$(RANLIB) $@

# Clean build artifacts
clean:
	rm -rf $(BUILD_DIR)

# Install the static library and headers (optional)
install: $(TARGET)
	install -d /usr/local/lib
	install -d /usr/local/include/pystackcollector
	install $(TARGET) /usr/local/lib/
	cp -r $(INCLUDE_DIR)/* /usr/local/include/pystackcollector/

# Print variables (for debugging)
.PHONY: info
info:
	@echo "Sources: $(CPP_SOURCES)"
	@echo "Objects: $(OBJECTS)"
	@echo "Target: $(TARGET)"
	@echo "Includes: $(INCLUDES)"
	@echo "Python includes: $(PYTHON_INCLUDES)"

# Dependency check
.PHONY: check-deps
check-deps:
	@echo "Checking for required dependencies..."
	@which $(CXX) >/dev/null || (echo "Error: C++ compiler not found"; exit 1)
	@which python3-config >/dev/null && echo "Python config found: $$(python3-config --includes)" || echo "Warning: python3-config not found"
	@python3 -c "import pybind11" 2>/dev/null && echo "pybind11 found" || echo "Warning: pybind11 not found"

# Debug build
debug: CXXFLAGS += -g -DDEBUG -UNDEBUG
debug: $(TARGET)

# Optimized release build
release: CXXFLAGS += -O3 -DNDEBUG
release: $(TARGET)