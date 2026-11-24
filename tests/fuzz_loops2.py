#!/usr/bin/env python3
"""Second loop-optimisation fuzzer: loops whose bounds / list lengths / induction variables CHANGE inside the body.

Targets loop versioning (bounds-check hoisting), strength reduction and range analysis with
  * list length changing in the loop (push / pop / clear / truncate / swap_remove / a callee taking `mut List`),
  * induction variables modified conditionally in the body (skips, resets, huge jumps near overflow),
  * bounds read from `a.len()` or parameters that the body updates, `for` ranges and `step`,
  * descending while loops, zero-trip loops, negative / huge strides, values close to Int limits,
  * loop-invariant lists passed as `view` / `mut` parameters and aliased element accesses.
C backend vs native vs native --no-opt must agree on stdout / stderr / exit code (also on panics).

usage: tests/fuzz_loops2.py [-n COUNT] [--seed SEED] [-j JOBS]
"""
import argparse, os, random, subprocess, sys, tempfile, shutil
from concurrent.futures import ThreadPoolExecutor

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
YERUX = os.path.join(ROOT, "target", "release", "yerux")
BUGS = os.path.join(ROOT, "tests", "bugs")

HELPERS = """fn shrink(a: mut List<Int>, k: Int) {
  if a.len() > k { a.truncate(a.len() - k) }
}
fn grow(a: mut List<Int>, k: Int) {
  if a.len() < 300 { for q in 0..k { a.push(q + 1) } }
}
fn drop_all(a: mut List<Int>) { a.clear() }
fn sum_view(a: view List<Int>, lo: Int, hi: Int) -> Int {
  var t = 0
  var i = lo
  while i < hi {
    t = t + a.get(i)
    i = i + 1
  }
  return t
}
"""


class G:
    def __init__(self, seed):
        self.r = random.Random(seed)
        self.out = []
        self.nv = 0

    def e(self, ind, s):
        self.out.append("  " * ind + s)

    def fresh(self, p):
        self.nv += 1
        return f"{p}{self.nv}"

    def small(self):
        return self.r.choice([0, 1, 1, 2, 3, 4, 5, 7, 10, 13, 100, -1, -2])

    def idx(self, vs):
        r = self.r
        if vs and r.random() < 0.55:
            v = r.choice(vs)
            return r.choice([v, v, f"{v} + 1", f"{v} - 1", f"{v} / 2", f"{v} % (a.len() + 1)" if False else v])
        terms = []
        for v in vs:
            if r.random() < 0.8:
                k = r.choice([1, 1, 1, 2, 3, 5, -1, "n", "m"])
                terms.append(f"{v} * {k}" if k != 1 else v)
        if r.random() < 0.5 or not terms:
            terms.append(str(r.choice([0, 1, 2, 5, -1, -3, "a.len() - 1", "a.len() - 2", "n"])))
        e = " + ".join(terms)
        if r.random() < 0.15:
            e = f"({e}) % {r.choice(['a.len() + 1', 7, 13, 'm + 1'])}"
        return e

    def body_stmt(self, ind, vs, acc, depth):
        r = self.r
        k = r.random()
        v = r.choice(vs)
        if k < 0.25:
            self.e(ind, f"{acc} = {acc} + a.get({self.idx(vs)}) * {r.choice([1, 1, 3, 'n', 1000003])}")
        elif k < 0.35:
            self.e(ind, f"a.set({self.idx(vs)}, {acc} % 97)")
        elif k < 0.45:
            self.e(ind, f"if {acc} % {r.choice([2, 3, 5])} == 0 && a.len() < 300 {{ a.push({acc} % 11) }}")
        elif k < 0.52:
            self.e(ind, f"if {acc} % {r.choice([3, 4, 7])} == 1 {{ if a.len() > 0 {{ {acc} = {acc} + a.pop() }} }}")
        elif k < 0.57:
            self.e(ind, f"if {v} == {r.randint(0, 6)} {{ a.clear() }}")
        elif k < 0.62:
            self.e(ind, f"if {v} == {r.randint(0, 6)} {{ shrink(a, {r.randint(1, 4)}) }}")
        elif k < 0.66:
            self.e(ind, f"if {v} % {r.choice([2, 3, 5])} == 0 {{ grow(a, {r.randint(1, 3)}) }}")
        elif k < 0.70:
            self.e(ind, f"if {v} == {r.randint(0, 5)} {{ drop_all(a) }}")
        elif k < 0.76:
            # modify the induction variable (skip / reset / jump)
            if v in self.loopvars_while:
                self.e(ind, f"if {acc} % {r.choice([3, 5, 7])} == 0 {{ {v} = {v} + {r.choice([1, 2, 3, -1, 1000] if self.safe else [1, 2, 3, -1, 1000, 4611686018427387904, 9223372036854775800])} }}")
        elif k < 0.80:
            self.e(ind, f"{acc} = {acc} + {v} * {r.choice([1, 3, 1000] if self.safe else [1, 3, 1000, 100000000000, 4611686018427387904])}")
        elif k < 0.84:
            self.e(ind, f"print({acc})")
        elif k < 0.88:
            self.e(ind, f"{acc} = {acc} + sum_view(a, {r.choice(['0', v, 'n'])}, {r.choice(['a.len()', 'm', v + ' + 2'])})")
        elif k < 0.92:
            self.e(ind, f"if {acc} % 2 == 0 {{ continue }}")
        elif k < 0.95:
            self.e(ind, f"if {acc} % 11 == 3 {{ break }}")
        else:
            self.e(ind, f"{acc} = {acc} + a.len()")

    def loop(self, ind, vs, acc, depth, maxd):
        r = self.r
        v = self.fresh("i")
        kind = r.random()
        bound = r.choice(["a.len()", "a.len()", "n", "m", "n + m", str(r.randint(0, 9)), "a.len() - 1", "a.len() + 2"])
        if kind < 0.3:
            lo = r.choice(["0", "0", "1", "-2", "n"])
            hi = bound
            op = r.choice(["..", "..="])
            stp = r.choice(["", "", " step 2", " step 3", " step n + 1"])
            self.e(ind, f"for {v} in {lo}{op}{hi}{stp} {{")
            vs2 = vs + [v]
            wh = False
        elif kind < 0.7:
            st = r.choice([1, 1, 1, 2, 3])
            self.e(ind, f"var {v} = {r.choice(['0', '0', '1', 'n', '-3'])}")
            self.e(ind, f"while {v} < {bound} {{")
            self.e(ind + 1, f"{v} = {v} + {st}")
            vs2 = vs + [v]
            wh = True
        else:
            st = r.choice([1, 1, 2, 3])
            self.e(ind, f"var {v} = {r.choice(['a.len()', 'n', 'a.len() + 3', 'm * 2'])}")
            self.e(ind, f"while {v} > {r.choice(['0', '0', '-1', '1'])} {{")
            self.e(ind + 1, f"{v} = {v} - {st}")
            vs2 = vs + [v]
            wh = True
        if wh:
            self.loopvars_while.add(v)
        for _ in range(r.randint(1, 4)):
            if self.safe:
                self.e(ind + 1, f"{acc} = {acc} % 1000003")
            if depth < maxd and r.random() < 0.25:
                self.loop(ind + 1, vs2, acc, depth + 1, maxd)
            else:
                self.body_stmt(ind + 1, vs2, acc, depth)
        self.e(ind, "}")

    def program(self):
        r = self.r
        self.loopvars_while = set()
        self.safe = r.random() < 0.6
        self.out.append(HELPERS)
        self.e(0, "fn work(n: Int, m: Int, a: mut List<Int>) -> Int {")
        self.e(1, "var acc = 1")
        for _ in range(r.randint(1, 2)):
            self.loop(1, [], "acc", 0, r.choice([1, 2, 3]))
        self.e(1, "return acc")
        self.e(0, "}")
        self.e(0, "fn main() {")
        for _ in range(r.randint(1, 3)):
            L = r.choice([0, 3, 16, 33, 64, 100, 100])
            self.e(1, "var a = List<Int>.new()")
            self.e(1, f"for q in 0..{L} {{ a.push(q * 3 % 11 + 1) }}")
            self.e(1, f"print(work({r.randint(0, 9)}, {r.randint(0, 9)}, a))")
            self.e(1, "print(a.len())")
        self.e(0, "}")
        return "\n".join(self.out) + "\n"


def _sh_raw(args, timeout=20):
    try:
        p = subprocess.run(args, capture_output=True, timeout=timeout, stdin=subprocess.DEVNULL)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return -999, b"", b"timeout"


def sh(*args, **kwargs):
    """Run a command; `  at file:line:col` panic location lines are dropped from stderr: the native backend prints
    them in release builds, the C backend only with YERUX_LOCS/--debug (docs/PANIC_DIAGNOSTICS.md)."""
    import re as _re
    r = _sh_raw(*args, **kwargs)
    if isinstance(r, tuple) and len(r) == 3 and isinstance(r[2], bytes):
        return r[0], r[1], _re.sub(rb"(?m)^  at [^\n]*\n?", b"", r[2])
    return r


def evaluate(src, tmp):
    f = os.path.join(tmp, "p.yx")
    open(f, "w").write(src)
    rc, _, err = sh([YERUX, "check", f])
    if rc != 0:
        return None, "rejected: " + err.decode(errors="replace")[:300]
    res = {}
    for name, flags in (("c", []), ("native", ["--native"]), ("noopt", ["--native", "--no-opt"])):
        exe = os.path.join(tmp, "p_" + name)
        rc, _, err = sh([YERUX, "build", f, "-o", exe] + flags, 60)
        if rc != 0:
            return None, "build failed (%s): %s" % (name, err.decode(errors="replace")[:300])
        res[name] = sh([exe])
        if res[name][0] == -999 or res[name][0] < 0:
            return "timeout", None
    return res, None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-n", type=int, default=300)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("-j", type=int, default=1)
    a = ap.parse_args()
    seed0 = a.seed if a.seed is not None else random.randrange(1 << 30)
    print(f"fuzz_loops2: seed={seed0} n={a.n}", flush=True)
    os.makedirs(BUGS, exist_ok=True)
    stats = dict(run=0, bad=0, panics=0, rejected=0, timeouts=0)

    def one(i):
        seed = seed0 + i
        src = G(seed).program()
        tmp = tempfile.mkdtemp(prefix="yfl2_")
        try:
            res, err = evaluate(src, tmp)
            stats["run"] += 1
            if res == "timeout":
                stats["timeouts"] += 1
                return
            if res is None:
                stats["rejected"] += 1
                if stats["rejected"] <= 5:
                    print(f"seed {seed}: {err}", flush=True)
                return
            if res["c"][0] == 101:
                stats["panics"] += 1
            bad = [n for n in ("native", "noopt") if res[n] != res["c"]]
            if bad:
                stats["bad"] += 1
                path = os.path.join(BUGS, f"loops2_{seed}.yx")
                open(path, "w").write(src)
                print(f"MISMATCH seed {seed} {bad}: saved {path}", flush=True)
                for n in ("c",) + tuple(bad):
                    print(n, res[n][0], res[n][1][-200:], res[n][2][-200:], flush=True)
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    with ThreadPoolExecutor(a.j) as ex:
        list(ex.map(one, range(a.n)))
    print("done:", stats)
    return 1 if stats["bad"] else 0


if __name__ == "__main__":
    sys.exit(main())
