#!/usr/bin/env python3
"""Differential fuzzer for loop optimisations (strength reduction, bounds-check hoisting/versioning).

Generates nested while-loops over a List<Int> with affine index expressions (i*stride + j + c, parameters,
negative steps, conditional access, stores, prints) so that bounds panics and overflow panics fire at random
iterations; compares C backend / native / native --no-opt (stdout, stderr, exit code).

usage: tests/fuzz_loops.py [-n COUNT] [--seed SEED] [-j JOBS]
"""
import argparse, os, random, subprocess, sys, tempfile, shutil
from concurrent.futures import ThreadPoolExecutor

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
YERUX = os.path.join(ROOT, "target", "release", "yerux")
BUGS = os.path.join(ROOT, "tests", "bugs")


def gen(seed):
    r = random.Random(seed)
    L = r.choice([1, 5, 16, 50, 100, 120, 300])
    params = ["n", "m", "s"]
    pv = {p: r.randint(0, 12) for p in params}

    def small():
        return r.choice([0, 1, 1, 2, 3, 4, 5, 7, 10, 13, 100])

    def bound(depth):
        c = r.random()
        if c < 0.45:
            return str(r.randint(0, 14))
        if c < 0.8:
            return r.choice(params)
        return f"{r.choice(params)} + {r.randint(0, 5)}"

    vars_ = []

    def idx_expr(vs):
        terms = []
        for v in vs:
            if r.random() < 0.8:
                k = r.choice([1, 1, 1, 2, 3, 5, 7, 10, 12, 20, -1, "n", "m", "s"])
                terms.append(f"{v} * {k}" if k != 1 else v)
        if r.random() < 0.4 or not terms:
            terms.append(str(r.choice([0, 1, 2, 5, -1, -3])))
        e = " + ".join(terms)
        if r.random() < 0.15:
            e = f"({e}) % {r.choice([L, L + 1, 7, 13])}"
        return e

    out = []
    out.append("fn work(n: Int, m: Int, s: Int) -> Int {")
    out.append("  var a = List<Int>.new()")
    out.append("  var q = 0")
    out.append(f"  while q < {L} {{")
    out.append("    a.push(q * 3 % 11 + 1)")
    out.append("    q = q + 1")
    out.append("  }")
    out.append("  var acc = 0")
    depth = r.choice([1, 2, 2, 3])
    names = ["i", "j", "k"][:depth]
    ind = "  "
    closers = []
    for d, v in enumerate(names):
        down = r.random() < 0.15
        step = r.choice([1, 1, 1, 2, 3])
        b = bound(d)
        if down:
            out.append(f"{ind}var {v} = {b}")
            out.append(f"{ind}while {v} > 0 {{")
            closers.append((ind, f"{v} = {v} - {step}"))
        else:
            out.append(f"{ind}var {v} = {r.choice([0, 0, 0, 1, 2])}")
            out.append(f"{ind}while {v} < {b} {{")
            closers.append((ind, f"{v} = {v} + {step}"))
        ind += "  "
        if d < depth - 1 and r.random() < 0.3:
            out.append(f"{ind}acc = acc + {v} * {r.choice([3, 1000, 100000000000, s_big(r)])}")
    for _ in range(r.randint(1, 3)):
        kind = r.random()
        e = idx_expr(names)
        if kind < 0.5:
            mul = r.choice(["1", "1", "n", names[-1], "3", "1000000007", "100000000000000"])
            out.append(f"{ind}acc = acc + a.get({e}) * {mul}")
        elif kind < 0.7:
            out.append(f"{ind}a.set({e}, acc % 97)")
        elif kind < 0.85:
            out.append(f"{ind}if acc % 3 != 1 {{")
            out.append(f"{ind}  acc = acc + a.get({e})")
            out.append(f"{ind}}}")
        else:
            out.append(f"{ind}acc = acc + {names[-1]} * {r.choice([2, 5, 1000])} + {names[0]} * n")
        if r.random() < 0.2:
            out.append(f"{ind}print(acc)")
    for ind2, inc in reversed(closers):
        out.append(f"{ind2}  {inc}")
        out.append(f"{ind2}}}")
    out.append("  return acc")
    out.append("}")
    out.append("fn main() {")
    for _ in range(r.randint(1, 3)):
        out.append(f"  print(work({r.randint(0, 12)}, {r.randint(0, 12)}, {r.randint(0, 12)}))")
    out.append("}")
    return "\n".join(out) + "\n"


def s_big(r):
    return r.choice([7, 9223372036854775807, 4611686018427387904])


def sh(args, timeout=20):
    try:
        p = subprocess.run(args, capture_output=True, timeout=timeout)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return -999, b"", b"timeout"


def evaluate(src, tmp):
    f = os.path.join(tmp, "p.yx")
    open(f, "w").write(src)
    res = {}
    for name, flags in (("c", []), ("native", ["--native"]), ("noopt", ["--native", "--no-opt"])):
        exe = os.path.join(tmp, "p_" + name)
        rc, _, err = sh([YERUX, "build", f, "-o", exe] + flags, 60)
        if rc != 0:
            return None, "build failed (%s): %s" % (name, err.decode(errors="replace")[:300])
        res[name] = sh([exe])
    return res, None


def one(seed):
    tmp = tempfile.mkdtemp(prefix="fl_")
    try:
        src = gen(seed)
        res, err = evaluate(src, tmp)
        if res is None:
            return seed, src, err, False
        ref = res["c"]
        panicked = ref[0] != 0
        for name in ("native", "noopt"):
            if res[name] != ref:
                return seed, src, "mismatch %s: c=%r %s=%r" % (name, ref, name, res[name]), panicked
        return seed, src, None, panicked
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-n", type=int, default=500)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("-j", type=int, default=os.cpu_count() or 2)
    a = ap.parse_args()
    base = a.seed if a.seed is not None else random.SystemRandom().randrange(1 << 30)
    print(f"fuzz_loops: seed={base} n={a.n}")
    bad = 0
    pan = 0
    with ThreadPoolExecutor(a.j) as ex:
        for seed, src, err, panicked in ex.map(one, range(base, base + a.n)):
            pan += panicked
            if err:
                bad += 1
                os.makedirs(BUGS, exist_ok=True)
                path = os.path.join(BUGS, f"loops_{seed}.yx")
                open(path, "w").write(src)
                print(f"MISMATCH seed={seed}: {err}\n  saved {path}")
    print(f"done: run={a.n} mismatches={bad} panicking={pan}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
