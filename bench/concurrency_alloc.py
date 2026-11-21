#!/usr/bin/env python3
"""Measure allocation-heavy callbacks in serial and on a persistent pool."""
import argparse
import os
import resource
import statistics
import subprocess
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
YERUX = os.path.join(ROOT, "target", "release", "yerux")


def build(source, output):
    subprocess.run([YERUX, "build", os.path.join(ROOT, source), "--native", "-o", output],
                   cwd=ROOT, check=True, stdout=subprocess.DEVNULL)


def measure(binary, expected, runs):
    wall, user, system = [], [], []
    for _ in range(runs + 1):
        before = resource.getrusage(resource.RUSAGE_CHILDREN)
        start = time.perf_counter()
        result = subprocess.run([binary], capture_output=True, text=True, check=True)
        wall.append(time.perf_counter() - start)
        after = resource.getrusage(resource.RUSAGE_CHILDREN)
        if result.stdout != expected:
            raise SystemExit(f"output mismatch: {result.stdout!r} != {expected!r}")
        user.append(after.ru_utime - before.ru_utime)
        system.append(after.ru_stime - before.ru_stime)
    return tuple(statistics.median(samples[1:]) for samples in (wall, user, system))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--runs", type=int, default=5)
    args = parser.parse_args()
    if not os.path.isfile(YERUX):
        subprocess.run(["cargo", "build", "--release", "--bin", "yerux"], cwd=ROOT, check=True)
    with tempfile.TemporaryDirectory(prefix="yerux-alloc-concurrency-") as temp:
        serial = os.path.join(temp, "serial")
        pooled = os.path.join(temp, "pool")
        build("bench/src/concurrency_alloc_serial.yx", serial)
        build("bench/src/concurrency_alloc_pool.yx", pooled)
        expected = subprocess.run([serial], capture_output=True, text=True, check=True).stdout
        serial_s = measure(serial, expected, args.runs)
        pool_s = measure(pooled, expected, args.runs)
        print(f"4 batches x 5000 tasks x 100 allocations, workers: 4, runs: {args.runs}")
        print(f"serial: wall {serial_s[0]:.6f}s, user {serial_s[1]:.6f}s, sys {serial_s[2]:.6f}s")
        print(f"pool: wall {pool_s[0]:.6f}s, user {pool_s[1]:.6f}s, sys {pool_s[2]:.6f}s "
              f"({pool_s[0] / serial_s[0]:.2f}x serial wall)")


if __name__ == "__main__":
    main()
