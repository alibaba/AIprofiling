# SPDX-License-Identifier: Apache-2.0
"""CPU-only inference workload — a stand-in for a batched inference server.

A pretend "model" (two dense layers + softmax) is applied to a stream of
random requests. The workload spends its time in a small number of hot
functions so the AIProf flamegraph is easy to read:

    main
      inference_loop
        run_batch
          dense_layer_1      <-- matmul
          gelu               <-- pure-python-ish activation
          dense_layer_2      <-- matmul
          softmax            <-- exp + normalize

Run:
    python3 inference_workload.py --batches 200 --batch 32

Prints its pid on stdout with --announce-pid, so a profiler can attach.
"""

from __future__ import annotations

import argparse
import math
import os
import sys
import time

import numpy as np


def dense_layer_1(x: np.ndarray, w: np.ndarray, b: np.ndarray) -> np.ndarray:
    return x @ w + b


def gelu(x: np.ndarray) -> np.ndarray:
    # Approximate GELU (Hendrycks) — deliberately expressed with a mix of
    # np ops so it shows up in the profile as its own frame.
    c = math.sqrt(2.0 / math.pi)
    return 0.5 * x * (1.0 + np.tanh(c * (x + 0.044715 * x * x * x)))


def dense_layer_2(x: np.ndarray, w: np.ndarray, b: np.ndarray) -> np.ndarray:
    return x @ w + b


def softmax(x: np.ndarray) -> np.ndarray:
    m = np.max(x, axis=-1, keepdims=True)
    e = np.exp(x - m)
    return e / np.sum(e, axis=-1, keepdims=True)


def run_batch(x, w1, b1, w2, b2):
    h = gelu(dense_layer_1(x, w1, b1))
    logits = dense_layer_2(h, w2, b2)
    return softmax(logits)


def inference_loop(rng, batches, batch, features, hidden, classes):
    w1 = rng.standard_normal((features, hidden), dtype=np.float32) * 0.05
    b1 = np.zeros((hidden,), dtype=np.float32)
    w2 = rng.standard_normal((hidden, classes), dtype=np.float32) * 0.05
    b2 = np.zeros((classes,), dtype=np.float32)

    n_correct = 0
    for i in range(batches):
        x = rng.standard_normal((batch, features), dtype=np.float32)
        probs = run_batch(x, w1, b1, w2, b2)
        # Toy accuracy metric so the loop can't be optimised away.
        n_correct += int(np.sum(np.argmax(probs, axis=-1) == (i % classes)))
    return n_correct


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--batches", type=int, default=200)
    ap.add_argument("--batch", type=int, default=32)
    ap.add_argument("--features", type=int, default=128)
    ap.add_argument("--hidden", type=int, default=256)
    ap.add_argument("--classes", type=int, default=16)
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--announce-pid", action="store_true",
                    help="print 'PID=<pid>' on the first line for the profiler")
    args = ap.parse_args()

    if args.announce_pid:
        print(f"PID={os.getpid()}", flush=True)

    rng = np.random.default_rng(args.seed)
    t0 = time.time()
    n = inference_loop(rng, args.batches, args.batch, args.features,
                       args.hidden, args.classes)
    dt = time.time() - t0
    print(f"inference_done batches={args.batches} elapsed_s={dt:.2f} matches={n}",
          file=sys.stderr, flush=True)


if __name__ == "__main__":
    main()
