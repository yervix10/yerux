#!/usr/bin/env python3
"""Mutate real Yerux sources and measure how far each mutation travels in the compiler."""
import argparse
import concurrent.futures
import os
import random
import re
import subprocess
import tempfile
import time
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
YERUX = ROOT / "target/release/yerux"
TOKEN = re.compile(r'//[^\n]*|"(?:\\.|[^"\\])*"|\s+|[A-Za-z_][A-Za-z_0-9]*|[0-9][0-9_.]*|.', re.S)
TYPES = ("Int", "Bool", "Text", "String", "Char", "Float", "List<Int>", "Option<Int>")
LITERALS = ("0", "1", "-1", "255", "999999999999999999999999", "true", "false", '"x"', "'a'")
PROFILE = re.compile(r"^yerux profile: (.+?)(?: result: (accepted|rejected))?(?::|$)", re.M)


def corpus():
    out = []
    for root in (ROOT / "tests", ROOT / "examples"):
        for path in sorted(root.rglob("*.yx")):
            try:
                if path.stat().st_size > 30_000:
                    continue
                src = path.read_text(encoding="utf-8")
            except (OSError, UnicodeError):
                continue
            # Keep the corpus self-contained when moved to a temporary directory.
            if re.search(r"(?m)^\s*mod\s+[A-Za-z_]", src):
                continue
            out.append((path, src))
    return out


def mutate(rng, src):
    how = rng.randrange(8)
    if how < 3:
        toks = TOKEN.findall(src)
        if not toks:
            return src + "!"
        i = rng.randrange(len(toks))
        if how == 0:  # delete token
            del toks[i]
        elif how == 1:  # duplicate token
            toks.insert(i, toks[i])
        else:  # swap tokens
            j = rng.randrange(len(toks))
            toks[i], toks[j] = toks[j], toks[i]
        return "".join(toks)
    lines = src.splitlines(keepends=True)
    if how == 3 and lines:
        i = rng.randrange(len(lines))
        del lines[i]
        return "".join(lines)
    if how == 4 and lines:
        i = rng.randrange(len(lines))
        lines.insert(i, lines[i])
        return "".join(lines)
    if how == 5 and len(lines) > 1:
        i, j = rng.sample(range(len(lines)), 2)
        lines[i], lines[j] = lines[j], lines[i]
        return "".join(lines)
    if how == 7 and src:
        return src[:rng.randrange(len(src))]  # truncate
    toks = TOKEN.findall(src)
    choices = [(i, t) for i, t in enumerate(toks) if t.strip()]
    if choices:
        i, old = rng.choice(choices)
        if old in TYPES:
            toks[i] = rng.choice(TYPES)
        elif old[0].isdigit() or old in ("true", "false") or old.startswith(('"', "'")):
            toks[i] = rng.choice(LITERALS)
        else:
            toks[i] = rng.choice(TYPES + LITERALS)
        return "".join(toks)
    return src + "!"


def make_mutant(rng, source):
    # One to three edits per case; enough variety without losing the source shape every time.
    for _ in range(rng.randint(1, 3)):
        source = mutate(rng, source)
        if len(source) > 50_000:
            source = source[:50_000]
    return source


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cases", type=int, default=20_000)
    ap.add_argument("--seed", type=int, default=0xC0DEC3)
    ap.add_argument("-j", type=int, default=min(12, os.cpu_count() or 4))
    ap.add_argument("--timeout", type=float, default=4.0)
    args = ap.parse_args()
    sources = corpus()
    if not sources:
        print("mutate_compile: no corpus sources")
        return 2
    rng = random.Random(args.seed)
    mutants = []
    for i in range(args.cases):
        path, src = sources[rng.randrange(len(sources))]
        mutants.append((path, make_mutant(rng, src)))
    stats = Counter()
    failures = []
    elapsed = time.monotonic()

    with tempfile.TemporaryDirectory(prefix="yerux_mutfuzz_") as td:
        def run(item):
            i, (origin, src) = item
            path = Path(td) / f"mutant_{i:05d}.yx"
            path.write_text(src, encoding="utf-8")
            env = dict(os.environ, YERUX_PROFILE="1")
            try:
                p = subprocess.run([str(YERUX), "c", str(path)], capture_output=True,
                                   text=True, timeout=args.timeout, env=env)
            except subprocess.TimeoutExpired:
                return i, origin, src, str(path), "timeout", b""
            return i, origin, src, str(path), p.returncode, p.stderr

        with concurrent.futures.ThreadPoolExecutor(max_workers=args.j) as pool:
            for i, origin, src, filepath, rc, err in pool.map(run, enumerate(mutants)):
                if rc == "timeout":
                    stats["timeouts"] += 1
                    failures.append((i, origin, src, "timeout", ""))
                else:
                    text = err if isinstance(err, str) else err.decode("utf-8", "replace")
                    markers = PROFILE.findall(text)
                    results = {}
                    for name, result in markers:
                        if result:
                            results.setdefault(name, []).append(result)
                    for stage in ("lexer", "module resolution", "parse", "type and ownership check"):
                        if stage in results:
                            stats[f"{stage}:reached"] += 1
                            if all(result == "accepted" for result in results[stage]):
                                stats[f"{stage}:accepted"] += 1
                    if any(name == "C generation" for name, _ in markers):
                        stats["codegen:reached"] += 1
                        stats["codegen:accepted"] += 1
                    if results.get("parse", [None])[-1] == "rejected":
                        stats["parser_rejected"] += 1
                    panic = rc < 0 or rc == 101 or "panicked at" in text.lower() or "internal compiler error" in text.lower()
                    if panic:
                        why = "panic/signal"
                    elif rc != 0 and ("error:" not in text.lower() or re.search(re.escape(filepath) + r":\d+", text) is None):
                        why = "missing clean file:line diagnostic"
                    else:
                        why = None
                    if why:
                        stats["failures"] += 1
                        failures.append((i, origin, src, why, text[:500]))
                if (i + 1) % 5000 == 0:
                    print(f"mutate_compile: completed {i + 1}/{args.cases}")

    print(f"mutate_compile: seed={args.seed}, corpus={len(sources)}, cases={len(mutants)}, workers={args.j}, elapsed={time.monotonic()-elapsed:.1f}s")
    for stage in ("lexer", "parse", "module resolution", "type and ownership check", "codegen"):
        print(f"  {stage}: reached={stats[stage + ':reached']}, accepted={stats[stage + ':accepted']}")
    checked = stats["type and ownership check:reached"]
    passed = stats["type and ownership check:accepted"]
    print(f"  types: reached={checked}, accepted={passed} (same integrated checker as ownership)")
    print(f"  ownership: reached={checked}, accepted={passed} (same integrated checker as types)")
    print(f"  parser_rejected={stats['parser_rejected']}, fraction={stats['parser_rejected']/max(1, stats['parse:reached']):.1%} of parser-reached, {stats['parser_rejected']/max(1, len(mutants)):.1%} of all cases")
    print(f"  timeouts={stats['timeouts']}, failures={stats['failures']}")
    for i, origin, src, why, details in failures[:10]:
        digest = f"{args.seed}_{i}"
        bug = ROOT / "tests/bugs" / f"mutant_{digest}.yx"
        bug.parent.mkdir(parents=True, exist_ok=True)
        bug.write_text(src, encoding="utf-8")
        print(f"FAIL #{i} from {origin}: {why}; saved {bug.relative_to(ROOT)}\n{details}")
    return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(main())
