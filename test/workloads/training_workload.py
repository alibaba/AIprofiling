# SPDX-License-Identifier: Apache-2.0
"""CPU-only training workload — a stand-in for a GPU model training loop.

The point of this file is *not* to train anything useful. It is to produce
a call graph that resembles a real training step (forward → loss → backward
→ optimizer step) so that AIProf, running perf on the pid, captures a
non-trivial folded flamegraph on a machine with no GPU.

Layout of the flamegraph, roughly:

    main
      train_epoch
        forward_pass
          matmul_layer      <-- dominates
          relu
        compute_loss
        backward_pass
          matmul_grad       <-- dominates
        optimizer_step

Run:
    python3 training_workload.py --steps 200 --hidden 256

Prints its pid on stdout so an out-of-process profiler can attach.
"""

from __future__ import annotations

import argparse
import os
import sys
import time

import numpy as np


def matmul_layer(x: np.ndarray, w: np.ndarray) -> np.ndarray:
    # Named function so perf can resolve it in the folded stacks.
    return x @ w


def relu(x: np.ndarray) -> np.ndarray:
    return np.maximum(x, 0.0)


def forward_pass(x, w1, w2):
    h = relu(matmul_layer(x, w1))
    y = matmul_layer(h, w2)
    return h, y


def compute_loss(y, target):
    diff = y - target
    return float(np.mean(diff * diff)), diff


def matmul_grad(a, b):
    return a.T @ b


def backward_pass(x, h, diff, w2):
    # dL/dy = 2 * diff / N
    n = diff.shape[0]
    grad_y = (2.0 / n) * diff
    grad_w2 = matmul_grad(h, grad_y)
    grad_h = grad_y @ w2.T
    grad_h[h <= 0] = 0.0
    grad_w1 = matmul_grad(x, grad_h)
    return grad_w1, grad_w2


def optimizer_step(w, grad, lr):
    w -= lr * grad
    return w


def train_epoch(x, target, w1, w2, lr, steps):
    for _ in range(steps):
        h, y = forward_pass(x, w1, w2)
        _, diff = compute_loss(y, target)
        gw1, gw2 = backward_pass(x, h, diff, w2)
        w1 = optimizer_step(w1, gw1, lr)
        w2 = optimizer_step(w2, gw2, lr)
    return w1, w2


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--steps", type=int, default=200)
    ap.add_argument("--batch", type=int, default=256)
    ap.add_argument("--features", type=int, default=128)
    ap.add_argument("--hidden", type=int, default=256)
    ap.add_argument("--out", type=int, default=64)
    ap.add_argument("--lr", type=float, default=1e-3)
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--announce-pid", action="store_true",
                    help="print 'PID=<pid>' on the first line for the profiler")
    args = ap.parse_args()

    if args.announce_pid:
        print(f"PID={os.getpid()}", flush=True)

    rng = np.random.default_rng(args.seed)
    x = rng.standard_normal((args.batch, args.features), dtype=np.float32)
    target = rng.standard_normal((args.batch, args.out), dtype=np.float32)
    w1 = rng.standard_normal((args.features, args.hidden), dtype=np.float32) * 0.1
    w2 = rng.standard_normal((args.hidden, args.out), dtype=np.float32) * 0.1

    t0 = time.time()
    w1, w2 = train_epoch(x, target, w1, w2, args.lr, args.steps)
    dt = time.time() - t0
    # Final loss so the caller can sanity-check the run did something.
    _, y = forward_pass(x, w1, w2)
    final_loss, _ = compute_loss(y, target)
    print(f"training_done steps={args.steps} elapsed_s={dt:.2f} final_loss={final_loss:.4f}",
          file=sys.stderr, flush=True)


if __name__ == "__main__":
    main()
