# Makefile for compiling pyki loader as a shared library (.so)

TARGET = libloader.so
SRC = loader.c
CC = gcc

# Python version detection and adaptation
PYTHON_VERSIONS = python3.12 python3.11 python3.10 python3.9 python3
DETECTED_PYTHON_CONFIG := $(shell \
    for py in $(PYTHON_VERSIONS); do \
        if command -v $${py}-config >/dev/null 2>&1; then \
            echo $${py}-config; \
            break; \
        fi; \
    done; \
    if [ -z "$$(which python3-config 2>/dev/null)" ]; then \
        echo "python3-config"; \
    fi \
)

# Pull the Python configuration into variables
PYTHON_CONFIG ?= $(DETECTED_PYTHON_CONFIG)

# Compile options
CFLAGS = -fPIC -O2 -Wall -Wextra -std=c99
CPPFLAGS = $(shell $(PYTHON_CONFIG) --includes 2>/dev/null || echo "-I/usr/include/python3.9") -I.
LDFLAGS = $(shell $(PYTHON_CONFIG) --ldflags 2>/dev/null || echo "-lpython3.9") -shared -pthread

# Default target
all: check-python $(TARGET)

# Verify the Python configuration
check-python:
	@echo "Using Python config: $(PYTHON_CONFIG)"
	@if ! command -v $(PYTHON_CONFIG) >/dev/null 2>&1; then \
		echo "Warning: $(PYTHON_CONFIG) not found. Will try to use default Python 3.9 settings."; \
	else \
		if [ -z "$$($(PYTHON_CONFIG) --includes 2>/dev/null)" ]; then \
			echo "Warning: Failed to get Python includes. Will try to use default Python 3.9 settings."; \
		else \
			echo "Python configuration detected successfully."; \
		fi; \
	fi

# Build rule
$(TARGET): $(SRC)
	$(CC) $(CFLAGS) $(CPPFLAGS) $(SRC) $(LDFLAGS) -o $(TARGET)

# Clean object files
clean:
	rm -f $(TARGET)

# Rebuild
re: clean all

# Print Python configuration (for debugging)
print-config:
	@echo "Detected Python config: $(DETECTED_PYTHON_CONFIG)"
	@echo "Using Python config: $(PYTHON_CONFIG)"
	@echo "Available Python versions: $(PYTHON_VERSIONS)"
	@echo "Python includes: $$($(PYTHON_CONFIG) --includes 2>/dev/null || echo "Not available")"
	@echo "Python ldflags: $$($(PYTHON_CONFIG) --ldflags 2>/dev/null || echo "Not available")"
	@echo "Using CFLAGS: $(CFLAGS)"
	@echo "Using CPPFLAGS: $(CPPFLAGS)"
	@echo "Using LDFLAGS: $(LDFLAGS)"

# Install target
install: $(TARGET)
	install -m 755 $(TARGET) /usr/local/lib/

# Test build against a specific Python version
test-python3.9:
	$(MAKE) PYTHON_CONFIG=python3.9-config

test-python3.10:
	$(MAKE) PYTHON_CONFIG=python3.10-config

test-python3.11:
	$(MAKE) PYTHON_CONFIG=python3.11-config

test-python3.12:
	$(MAKE) PYTHON_CONFIG=python3.12-config

.PHONY: all clean re print-config check-python install \
	test-python3.9 test-python3.10 test-python3.11 test-python3.12