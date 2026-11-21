#!/usr/bin/env python3
"""Compare wall-clock builds of the first ten runnable examples."""
import argparse
from pathlib import Path
import statistics
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("selfhost", type=Path, help="installed yerux-self binary")
    parser.add_argument("--rust", type=Path, default=Path("target/release/yerux"))
    parser.add_argument("--samples", type=int, default=5)
    args = parser.parse_args()
    if args.samples < 1:
        parser.error("--samples must be positive")
    compilers = (args.rust.resolve(), args.selfhost.resolve())
    root = Path(__file__).resolve().parents[1]
    examples = [next((root / "examples").glob(f"{n:02d}_*.yx")) for n in range(1, 11)]
    totals = [0.0, 0.0]
    print("| Programa | Rust (ms) | Autoalojado (ms) |")
    print("| --- | ---: | ---: |")
    with tempfile.TemporaryDirectory() as directory:
        output = Path(directory) / "example"
        for source in examples:
            samples = [[], []]
            for sample in range(args.samples + 1):
                order = (0, 1) if sample % 2 == 0 else (1, 0)
                for index in order:
                    start = time.perf_counter()
                    subprocess.run(
                        [compilers[index], "build", source, "-o", output],
                        cwd=root,
                        stdout=subprocess.DEVNULL,
                        check=True,
                    )
                    elapsed = time.perf_counter() - start
                    if sample:
                        samples[index].append(elapsed * 1000)
            rust, selfhost = map(statistics.median, samples)
            totals[0] += rust
            totals[1] += selfhost
            print(f"| `{source.name}` | {rust:.1f} | {selfhost:.1f} |")
    print(f"| **Suma** | **{totals[0]:.1f}** | **{totals[1]:.1f}** |")


if __name__ == "__main__":
    main()
