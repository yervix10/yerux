#!/usr/bin/env python3
"""List PE build outcomes for runtime goldens and standalone programs, without starting a VM."""
import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def corpus():
    sources = sorted((ROOT / "tests").glob("*.yx"))
    for source in sources:
        expected = ROOT / "tests/expected" / (source.stem + ".out")
        if expected.exists() and expected.with_suffix(".code").exists():
            yield "golden", source.relative_to(ROOT)
    for source in sorted((ROOT / "examples").rglob("*.yx")):
        if re.match(r"e[0-9]", source.name):
            continue  # intentional compile-error examples
        if re.search(rb"^(?:pub )?fn main\s*\(", source.read_bytes(), re.MULTILINE):
            yield "example", source.relative_to(ROOT)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--yerux", type=Path, default=ROOT / "target/release/yerux")
    parser.add_argument("-j", type=int, default=4)
    args = parser.parse_args()
    compiler = args.yerux.resolve()

    with tempfile.TemporaryDirectory(prefix="yerux-pe-audit-") as directory:
        def build(item):
            index, (suite, source) = item
            output = Path(directory) / f"{index}.exe"
            try:
                result = subprocess.run(
                    [compiler, "build", source, "--target", "windows", "-o", output],
                    cwd=ROOT, capture_output=True, timeout=90,
                )
            except subprocess.TimeoutExpired:
                return suite, source, "timeout", "build exceeded 90 seconds"
            if result.returncode == 0:
                output.unlink()
                return suite, source, "built", ""
            message = next(iter(result.stderr.decode(errors="replace").splitlines()),
                           f"exit {result.returncode}")
            match = re.match(r"error: native backend: (.*?) is not supported on", message)
            if match:
                reason = match[1].split(" (used by function", 1)[0]
                return suite, source, reason, message
            return suite, source, "other build error", message

        with ThreadPoolExecutor(args.j) as pool:
            totals = Counter()
            for suite, source, result, detail in pool.map(build, enumerate(corpus())):
                totals[suite, result] += 1
                print(f"{suite}\t{source}\t{result}\t{detail}")
            for (suite, result), count in sorted(totals.items()):
                print(f"total\t{suite}\t{result}\t{count}")
            return int(any(result in ("other build error", "timeout") for _, result in totals))


if __name__ == "__main__":
    raise SystemExit(main())
