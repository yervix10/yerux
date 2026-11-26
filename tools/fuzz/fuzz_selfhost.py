#!/usr/bin/env python3
"""Compare diagnostics and executable behavior from the Rust and selfhost compilers."""

import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from functools import partial
import json
import os
from pathlib import Path
import random
import re
import shutil
import signal
import subprocess
import tempfile

import fuzz_backends
import fuzz_syntax
import fuzz_valid


ROOT = Path(__file__).resolve().parents[2]
ERROR = re.compile(rb"^error(?:\[[^\]]+\])?: .*", re.MULTILINE)
LEX_ERROR = re.compile(rb"^Error:([0-9a-f]+):\d+:\d+:\d+$", re.MULTILINE)
STAGE_CRASH = re.compile(rb"^yerux-self: .*: signal \d+$", re.MULTILINE)


@dataclass(frozen=True)
class Outcome:
    status: int | None
    stdout: bytes
    stderr: bytes


def invoke(command, cwd, timeout):
    with subprocess.Popen(
        command, cwd=cwd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, start_new_session=True,
        env=dict(os.environ, LC_ALL="C", NO_COLOR="1", TMPDIR=str(cwd)),
    ) as child:
        try:
            stdout, stderr = child.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            stdout, stderr = child.communicate()
            return Outcome(None, stdout, stderr)
        return Outcome(child.returncode, stdout, stderr)


def first_error(outcome):
    for stream in (outcome.stderr, outcome.stdout):
        match = ERROR.search(stream)
        if match:
            return match[0].decode("utf-8", errors="replace")
        match = LEX_ERROR.search(stream)
        if match:
            return "error: " + bytes.fromhex(match[1].decode("ascii")).decode("utf-8", errors="replace")
    for line in outcome.stderr.splitlines() + outcome.stdout.splitlines():
        if line.strip():
            return line.decode("utf-8", errors="replace")
    return ""


def inspect_case(source, rust, selfhost, cc, timeout, runtime_timeout=3):
    with tempfile.TemporaryDirectory(prefix="yerux-fuzz-selfhost-") as directory:
        work = Path(directory)
        path = work / "case.yx"
        path.write_text(source, encoding="utf-8")
        rust_c = invoke([rust, "c", path], work, timeout)
        self_c = invoke([selfhost, "c", path], work, timeout)
        results = {"rust": rust_c, "selfhost": self_c}
        rust_ok, self_ok = rust_c.status == 0, self_c.status == 0
        messages = [first_error(rust_c) if not rust_ok else "",
                    first_error(self_c) if not self_ok else ""]
        if None in (rust_c.status, self_c.status):
            return "compiler-timeout", messages, results
        if (any(status < 0 or status >= 101 for status in (rust_c.status, self_c.status))
                or STAGE_CRASH.search(self_c.stderr)):
            if crash := STAGE_CRASH.search(self_c.stderr):
                messages[1] = crash[0].decode("utf-8", errors="replace")
            return "compiler-crash", messages, results
        if rust_ok != self_ok:
            return ("selfhost-rejects" if rust_ok else "selfhost-accepts"), messages, results
        if not rust_ok:
            if rust_c.status != self_c.status:
                return "diagnostic-status", messages, results
            return ("matched-rejection" if messages[0] == messages[1]
                    else "diagnostic-message"), messages, results

        programs = []
        for label, compiler in results.copy().items():
            # Identical translations can share a build. They still run separately.
            if label == "selfhost" and rust_c.stdout == self_c.stdout:
                programs.append(programs[0])
                break
            c_file = work / f"{label}.c"
            executable = work / label
            c_file.write_bytes(compiler.stdout)
            build = invoke([cc, "-std=gnu11", "-O0", c_file, "-o", executable,
                            "-pthread", "-lm"], work, timeout)
            if build.status != 0:
                results[f"{label}-cc"] = build
                return f"{label}-cc", [first_error(build), ""], results
            programs.append(executable)
        rust_run, self_run = (invoke([program], work, runtime_timeout) for program in programs)
        results.update({"rust-run": rust_run, "selfhost-run": self_run})
        if rust_run.status is None and self_run.status is None:
            return "shared-runtime-timeout", messages, results
        if None in (rust_run.status, self_run.status):
            return "runtime-timeout", messages, results
        if (rust_run.status, rust_run.stdout) != (self_run.status, self_run.stdout):
            return "runtime-difference", messages, results
        if rust_run.status < 0:
            return "runtime-signal", messages, results
        return "matched-execution", messages, results


def corpus(rust):
    seeds = {"tests": [], "examples": []}
    for origin in seeds:
        for file in sorted((ROOT / origin).glob("*.yx")):
            source = file.read_text(encoding="utf-8")
            if (re.match(r"e\d", file.name) or len(source) > 4000 or "fn main" not in source
                    or fuzz_valid.BAD.search(source) or "import " in source
                    or re.search(r"\b(extern|unsafe|unlink|write_file)\b", source)):
                continue
            if invoke([rust, "check", file], ROOT, 5).status == 0:
                seeds[origin].append((str(file.relative_to(ROOT)), source))
    return seeds


def generate(rust, seed, count):
    seeds = corpus(rust)
    if not all(seeds.values()):
        raise ValueError("no valid standalone seeds in tests/ or examples/")
    profiles = tuple(fuzz_backends.GENERATORS)
    mutators = (fuzz_valid.mutate, fuzz_syntax.mutate)
    index = seed
    seen = set()
    while len(seen) < count:
        rng = random.Random(index)
        origin = ("tests", "examples", "generated")[index % 3]
        if origin == "generated":
            profile = profiles[(index // 3) % len(profiles)]
            name = f"generated/{profile}"
            base = fuzz_backends.GENERATORS[profile](rng)
        else:
            name, base = rng.choice(seeds[origin])
        mutator = mutators[(index // 3) % len(mutators)]
        source = mutator(base, rng)
        for _ in range(5):
            if source != base:
                break
            source = mutator(base, rng)
        if source != base and source not in seen:
            seen.add(source)
            yield index, name, source
        index += 1


def run_case(case, *, out, rust, selfhost, cc, timeout, runtime_timeout):
    index, name, source = case
    kind, messages, results = inspect_case(source, rust, selfhost, cc, timeout, runtime_timeout)
    record = {"case": index, "origin": name, "kind": kind, "messages": messages,
              "statuses": {label: value.status for label, value in results.items()}}
    if not kind.startswith("matched-"):
        saved = out / f"case-{index}"
        saved.mkdir()
        (saved / "case.yx").write_text(source, encoding="utf-8")
        (saved / "result.json").write_text(json.dumps(record, ensure_ascii=False, indent=2))
        for label, value in results.items():
            # For accepted compilers the stdout is C and can be regenerated.
            if label not in ("rust", "selfhost") or value.status != 0:
                (saved / f"{label}.out").write_bytes(value.stdout)
            (saved / f"{label}.err").write_bytes(value.stderr)
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--yerux", type=Path, default=os.environ.get("YERUX", ROOT / "target/release/yerux"))
    parser.add_argument("--selfhost", type=Path, default=os.environ.get("YERUX_SELFHOST", ROOT / "target/selfhost/yerux-self"))
    parser.add_argument("--cc", default=os.environ.get("YERUX_CC", "cc"))
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--count", type=int, default=2000)
    parser.add_argument("--jobs", "-j", type=int, default=4)
    parser.add_argument("--timeout", type=float, default=20, help="seconds per compiler invocation")
    parser.add_argument("--runtime-timeout", type=float, default=3, help="seconds per generated executable")
    parser.add_argument("--out", required=True, type=Path, help="new directory for results")
    parser.add_argument("files", nargs="*", type=Path, help="compare these files instead of generating cases")
    options = parser.parse_args()
    if min(options.jobs, options.count, options.timeout, options.runtime_timeout) <= 0:
        parser.error("jobs, count and timeout must be positive")
    rust, selfhost = (path.resolve() for path in (options.yerux, options.selfhost))
    for executable in (rust, selfhost):
        if not os.access(executable, os.X_OK):
            parser.error(f"compiler is not executable: {executable}")
    cc = shutil.which(options.cc)
    if cc is None:
        parser.error(f"C compiler is not executable: {options.cc}")
    options.out.mkdir(parents=True, exist_ok=False)
    cases = ((index, str(file), file.read_text(encoding="utf-8"))
             for index, file in enumerate(options.files)) if options.files else generate(rust, options.seed, options.count)
    checker = partial(run_case, out=options.out, rust=rust, selfhost=selfhost,
                      cc=Path(cc).resolve(), timeout=options.timeout, runtime_timeout=options.runtime_timeout)
    counts = Counter()
    with (options.out / "results.jsonl").open("w", encoding="utf-8") as log:
        with ThreadPoolExecutor(max_workers=options.jobs) as pool:
            for record in pool.map(checker, cases):
                counts[record["kind"]] += 1
                log.write(json.dumps(record, ensure_ascii=False) + "\n")
                log.flush()
    print(json.dumps(dict(sorted(counts.items())), indent=2))
    total = sum(counts.values())
    matched = counts["matched-execution"] + counts["matched-rejection"]
    inconclusive = counts["shared-runtime-timeout"]
    flagged = total - matched - inconclusive
    print(f"cases={total} matched={matched} flagged={flagged} inconclusive={inconclusive}")
    return int(flagged != 0 or inconclusive != 0)


if __name__ == "__main__":
    raise SystemExit(main())
