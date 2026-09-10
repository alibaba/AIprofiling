#!/usr/bin/env python3
"""
Synthetic target for injector_probe.

Runs a controlled workload so ptrace-attach lands on specific glibc paths
that libprofiler.a's stack blacklist rejects:

  tls   -> repeated dlopen/dlclose to keep __tls_get_addr on the stack
  heap  -> multi-thread malloc/free churn, keeps _int_malloc/_int_free hot
  mixed -> both at once
  torch -> minimal torch loop, `record_memory_history` enabled; useful
           when combining injector stress with a snapshot-shape probe

The process runs until killed. Print its pid on start so `probe` can
attach.
"""

import argparse
import ctypes
import ctypes.util
import os
import sys
import threading
import time


def worker_tls(candidates):
    libc = ctypes.CDLL(None)
    libc.dlopen.restype = ctypes.c_void_p
    libc.dlopen.argtypes = [ctypes.c_char_p, ctypes.c_int]
    libc.dlclose.restype = ctypes.c_int
    libc.dlclose.argtypes = [ctypes.c_void_p]
    RTLD_LAZY = 1
    i = 0
    while True:
        name = candidates[i % len(candidates)]
        h = libc.dlopen(name.encode(), RTLD_LAZY)
        if h:
            libc.dlclose(h)
        i += 1


def worker_heap(block_bytes):
    while True:
        # Force allocator to walk arenas: mix of small and large blocks.
        bufs = []
        for _ in range(64):
            bufs.append(bytearray(block_bytes))
        # Drop half, keep half — forces fragmentation.
        del bufs[::2]
        del bufs


def resolve_shared_libs():
    """Pick libraries the target host is guaranteed to have."""
    candidates = []
    for name in ("m", "rt", "dl", "crypt", "resolv"):
        path = ctypes.util.find_library(name)
        if path:
            candidates.append(path)
    if not candidates:
        candidates = ["libm.so.6", "libdl.so.2", "librt.so.1"]
    return candidates


def run_torch(record_history):
    try:
        import torch
    except ImportError:
        print("torch not installed; --mode torch unavailable", file=sys.stderr)
        sys.exit(2)

    if not torch.cuda.is_available():
        print("CUDA not available; --mode torch unavailable", file=sys.stderr)
        sys.exit(2)

    if record_history:
        torch.cuda.memory._record_memory_history(max_entries=100_000)

    device = torch.device("cuda:0")
    x = torch.randn(1024, 1024, device=device)
    step = 0
    while True:
        # Burst then release, to make snapshot event count high.
        buf = [torch.randn(2048, 2048, device=device) for _ in range(4)]
        y = x @ x
        del buf, y
        torch.cuda.synchronize()
        step += 1
        if step % 100 == 0:
            print(f"torch step {step}", flush=True)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--mode", choices=["tls", "heap", "mixed", "torch"],
                   default="mixed")
    p.add_argument("--threads", type=int, default=4,
                   help="worker threads for heap/tls/mixed")
    p.add_argument("--heap-block", type=int, default=64 * 1024,
                   help="bytes per allocation in heap workers")
    p.add_argument("--record-memory-history", action="store_true",
                   help="only meaningful with --mode torch")
    args = p.parse_args()

    print(f"target pid = {os.getpid()}  mode = {args.mode}", flush=True)

    if args.mode == "torch":
        run_torch(args.record_memory_history)
        return

    libs = resolve_shared_libs()
    threads = []
    for _ in range(args.threads):
        if args.mode in ("tls", "mixed"):
            t = threading.Thread(target=worker_tls, args=(libs,), daemon=True)
            t.start()
            threads.append(t)
        if args.mode in ("heap", "mixed"):
            t = threading.Thread(target=worker_heap, args=(args.heap_block,),
                                 daemon=True)
            t.start()
            threads.append(t)

    while True:
        time.sleep(3600)


if __name__ == "__main__":
    main()
