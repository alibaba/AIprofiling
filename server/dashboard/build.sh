#!/bin/bash

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUTPUT_DIR="${SCRIPT_DIR}/dist"

echo "========================================="
echo "        Building analysis tool           "
echo "========================================="

if ! command -v pyinstaller &> /dev/null; then
    echo "PyInstaller not found, installing..."
    pip install pyinstaller -i https://mirrors.aliyun.com/pypi/simple/
fi

echo "Cleaning previous build artifacts..."
rm -rf "${SCRIPT_DIR}/build"
rm -rf "${OUTPUT_DIR}"

echo "Running PyInstaller..."
cd "${SCRIPT_DIR}"

pyinstaller --clean \
    --onefile \
    --name analysis_summary \
    --add-data "gmem.py:." \
    --hidden-import ijson \
    --hidden-import pandas \
    --hidden-import numpy \
    --hidden-import concurrent.futures \
    --hidden-import multiprocessing \
    analysis.py

if [ -f "${OUTPUT_DIR}/analysis_summary" ]; then
    echo "Moving executable to parent directory..."
    mv "${OUTPUT_DIR}/analysis_summary" "${SCRIPT_DIR}/analysis_summary"

    echo "Cleaning up build residue..."
    rm -rf "${SCRIPT_DIR}/build"
    rm -rf "${OUTPUT_DIR}"

    echo "========================================="
    echo "              Build succeeded!           "
    echo "========================================="

    echo "Executable path: ${SCRIPT_DIR}/analysis_summary"
    ls -lh "${SCRIPT_DIR}/analysis_summary"
    echo ""
    echo "Usage:"
    echo "  ./analysis_summary -d <data directory>"
    echo ""
else
    echo "Build failed. Check the error log."
    exit 1
fi
