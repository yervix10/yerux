#!/usr/bin/env python3
"""Generate small valid and invalid Yerux programs and check compiler diagnostics."""
import argparse
import random
import re
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
YERUX = ROOT / "target/release/yerux"


def expression(rng, depth=0):
    if depth >= 3 or rng.random() < 0.4:
        return str(rng.randrange(0, 100))
    op = rng.choice(("+", "-", "*", "%"))
    rhs = "1" if op == "%" else expression(rng, depth + 1)
    return f"({expression(rng, depth + 1)} {op} {rhs})"


def program(rng, valid):
    if valid:
        return f"fn main() {{\n  let answer = {expression(rng)}\n  print(answer)\n}}\n"
    kind = rng.randrange(3)
    if kind == 0:
        return "fn main() {\n  let answer: Int = true\n}\n"
    if kind == 1:
        return "fn main() {\n  print(missing_name)\n}\n"
    return "fn main( {\n  print(1)\n}\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-n", "--cases", type=int, default=2000)
    ap.add_argument("--seed", type=int, default=0x59455255)
    ap.add_argument("--timeout", type=float, default=2.0)
    args = ap.parse_args()
    rng = random.Random(args.seed)
    failures = 0
    deadline = time.monotonic() + 55
    with tempfile.TemporaryDirectory(prefix="yerux_genfuzz_") as td:
        for i in range(args.cases):
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                print(f"fuzz_compile: stopped at 55s budget after {i} cases")
                break
            valid = i % 2 == 0
            path = Path(td) / f"case_{i:05d}.yx"
            path.write_text(program(rng, valid), encoding="utf-8")
            try:
                proc = subprocess.run([str(YERUX), "check", str(path)], capture_output=True,
                                      text=True, timeout=min(args.timeout, remaining))
                err = proc.stderr
                panic = proc.returncode < 0 or proc.returncode == 101 or "panicked at" in err.lower()
                has_location = re.search(re.escape(str(path)) + r":\d+", err) is not None
                bad = panic or (proc.returncode == 0 if not valid else proc.returncode != 0)
                if not valid and ("error:" not in err.lower() or not has_location):
                    bad = True
                if bad:
                    failures += 1
                    print(f"FAIL case={i} valid={valid} rc={proc.returncode} stderr={err[:240]!r}")
                    if failures >= 10:
                        break
            except subprocess.TimeoutExpired:
                failures += 1
                print(f"FAIL case={i} timeout after {args.timeout}s")
                if failures >= 10:
                    break
    print(f"fuzz_compile: seed={args.seed}, cases={i + 1}, failures={failures}")
    return int(failures != 0)


if __name__ == "__main__":
    raise SystemExit(main())
