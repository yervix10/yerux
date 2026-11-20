#!/usr/bin/env python3
"""Differentially exercise range signatures and interval arithmetic.

Each generated program is compiled to C, optimized x64 native, x64 native
without optimization and x64 native with --check-ranges. All four executables must print the same values.
"""
import argparse
import os
import random
import subprocess
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
YERUX = os.path.join(ROOT, "target", "release", "yerux")


def interval(op, a, b):
    al, ah = a
    bl, bh = b
    if op == "+":
        return al + bl, ah + bh
    if op == "-":
        return al - bh, ah - bl
    if op == "*":
        xs = [al * bl, al * bh, ah * bl, ah * bh]
        return min(xs), max(xs)
    if op == "/":
        xs = [al // bl, al // bh, ah // bl, ah // bh]
        # Yerux division truncates toward zero. Generated numerators are
        # nonnegative and denominators positive, so // is the same operation.
        return min(xs), max(xs)
    m = max(abs(bl), abs(bh)) - 1
    return 0, min(m, ah)


def build_source(rng, cases):
    funcs, calls, expected = [], [], []
    for i, op in enumerate(["+", "-", "*", "/", "%"]):
        a = (rng.randint(0, 8), rng.randint(9, 20))
        b = (rng.randint(1, 3), rng.randint(4, 8)) if op in "/%" else (rng.randint(0, 5), rng.randint(6, 12))
        out = interval(op, a, b)
        funcs.append(
            f"fn f{i}(a: Int<{a[0]}..{a[1]}>, b: Int<{b[0]}..{b[1]}>) -> Int<{out[0]}..{out[1]}> {{ return a {op} b }}"
        )
        for _ in range(cases):
            x, y = rng.randint(*a), rng.randint(*b)
            calls.append(f"  print(f{i}({x}, {y}))")
            value = x + y if op == "+" else x - y if op == "-" else x * y if op == "*" else x // y if op == "/" else x % y
            expected.append(str(value))
    source = "\n".join(funcs) + "\nfn main() {\n" + "\n".join(calls) + "\n}\n"
    return source, (0, ("\n".join(expected) + "\n").encode(), b"")


def tdiv(a, b):
    q = abs(a) // abs(b)
    return q if (a >= 0) == (b > 0) else -q


def build_flow_source(rng, cases):
    """Functions that narrow an Int by comparisons, convert with try_from / from, and loop over typed ranges.
    The oracle is computed here; every guard direction (< <= > >= == !=, &&, ||, !) is exercised."""
    funcs, calls, expected = [], [], []
    for i in range(6):
        lo = rng.randint(-20, 10)
        hi = lo + rng.randint(0, 40)
        k = rng.randint(1, 5)
        c = rng.randint(-9, 9)
        kind = rng.choice(["guard", "guard_or", "guard_not", "try", "from", "loop", "mixed"])
        calls_i = []
        if kind == "guard":
            funcs.append(
                f"fn g{i}(n: Int) -> Int {{\n  if n < {lo} {{ return -1 }}\n  if n > {hi} {{ return -2 }}\n"
                f"  let p: Int<{lo}..{hi}> = n\n  return p * {k} + {c}\n}}"
            )
            fn = lambda n: -1 if n < lo else -2 if n > hi else n * k + c
        elif kind == "guard_or":
            funcs.append(
                f"fn g{i}(n: Int) -> Int {{\n  if n < {lo} || n > {hi} {{ return -1 }}\n"
                f"  let p: Int<{lo}..{hi}> = n\n  return p - {c}\n}}"
            )
            fn = lambda n: -1 if (n < lo or n > hi) else n - c
        elif kind == "guard_not":
            funcs.append(
                f"fn g{i}(n: Int) -> Int {{\n  if !(n >= {lo} && n <= {hi}) {{ return 7 }}\n"
                f"  let p: Int<{lo}..{hi}> = n\n  return p * p\n}}"
            )
            fn = lambda n: 7 if not (lo <= n <= hi) else n * n
        elif kind == "try":
            funcs.append(
                f"fn g{i}(n: Int) -> Int {{\n  match Int<{lo}..{hi}>.try_from(n) {{\n"
                f"    Ok(b) => {{ return b * {k} + {c} }}\n    Err(v) => {{ return v - {c} }}\n  }}\n}}"
            )
            fn = lambda n: n * k + c if lo <= n <= hi else n - c
        elif kind == "from":
            funcs.append(f"fn g{i}(n: Int) -> Int {{\n  let b = Int<{lo}..{hi}>.from(n)\n  return b + {k}\n}}")
            fn = lambda n: n + k
        elif kind == "loop":
            top = rng.randint(1, 12)
            funcs.append(
                f"fn g{i}(n: Int) -> Int {{\n  var s = 0\n  for j in 0..{top} {{\n    let d: Int<0..{top - 1}> = j\n"
                f"    s = s + d * {k} + n\n  }}\n  return s\n}}"
            )
            fn = lambda n: sum(j * k + n for j in range(top))
        else:
            funcs.append(
                f"fn g{i}(n: Int) -> Int {{\n  if n >= {lo} && n <= {hi} {{\n    let p: Int<{lo}..{hi}> = n\n"
                f"    let q: Int<{lo - 3}..{hi + 3}> = p + {k % 4 - 1 if k % 4 - 1 else 0}\n    return q\n  }}\n  return n % 7\n}}"
            )
            d = k % 4 - 1 if k % 4 - 1 else 0
            fn = lambda n: n + d if lo <= n <= hi else (abs(n) % 7) * (1 if n >= 0 else -1)
        panics = False
        for _ in range(cases):
            n = rng.randint(lo - 8, hi + 8)
            if kind == "from" and not (lo <= n <= hi):
                continue
            calls.append(f"  print(g{i}({n}))")
            expected.append(str(fn(n)))
    source = "\n".join(funcs) + "\nfn main() {\n" + "\n".join(calls) + "\n}\n"
    return source, (0, ("\n".join(expected) + "\n").encode(), b"")


INF = 10 ** 18
FULL = (-INF, INF)
EMPTY = None  # no value: the code cannot run


def hull(a, b):
    if a is EMPTY:
        return b
    if b is EMPTY:
        return a
    return (min(a[0], b[0]), max(a[1], b[1]))


def meet(env, lo, hi):
    if env is EMPTY:
        return EMPTY
    l, h = max(env[0], lo), min(env[1], hi)
    return (l, h) if l <= h else EMPTY


def finite(env):
    return env is not EMPTY and abs(env[0]) < 500 and abs(env[1]) < 500


class VarGen:
    """Random `var` programs: conditions, assignments, loops, `as .. else` and `match` narrow `a`; the generator
    tracks an interval it is sure of (never tighter than what the checker proves) and uses it in annotated lets
    that the checker must accept. A Python twin of every statement is the oracle."""

    def __init__(self, rng):
        self.rng = rng
        self.n = 0

    def fresh(self, base):
        self.n += 1
        return f"{base}{self.n}"

    def assigns_a(self, stmts):
        return any(s[0] == "assign_a" or (s[0] in ("if", "loop", "match") and self.assigns_a_nested(s)) for s in stmts)

    def assigns_a_nested(self, s):
        return any(self.assigns_a(blk) for blk in s[-1])

    def block(self, depth, env, ind):
        """-> (yerux lines, python lines, env after, assigned_a)"""
        ys, ps = [], []
        assigned = False
        for _ in range(self.rng.randint(1, 3)):
            y, p, env, asg = self.stmt(depth, env, ind)
            ys += y
            ps += p
            assigned |= asg
        return ys, ps, env, assigned

    def stmt(self, depth, env, ind):
        r = self.rng
        sp = "  " * ind
        kinds = ["add_b", "assign_const", "assign_add", "use", "narrow_if", "as_else", "match"]
        if depth < 2:
            kinds += ["if", "if", "loop"]
        kind = r.choice(kinds)
        if kind == "add_b":
            k = r.randint(1, 4)
            return [f"{sp}b = b + a + {k}"], [f"{sp}b = b + a + {k}"], env, False
        if kind == "assign_const":
            k = r.randint(-9, 9)
            return [f"{sp}a = {k}"], [f"{sp}a = {k}"], (k, k), True
        if kind == "assign_add":
            k = r.randint(-3, 3)
            ne = EMPTY if env is EMPTY else (env[0] + k, env[1] + k)
            return [f"{sp}a = a + {k}"], [f"{sp}a = a + {k}"], ne, True
        if kind == "use":
            if not finite(env):
                return [f"{sp}b = b + 1"], [f"{sp}b = b + 1"], env, False
            v = self.fresh("p")
            lo, hi = env
            return ([f"{sp}let {v}: Int<{lo}..{hi}> = a", f"{sp}b = b + {v} * 2"], [f"{sp}b = b + a * 2"], env, False)
        if kind == "narrow_if":
            lo = r.randint(-12, 5)
            hi = lo + r.randint(0, 20)
            ne = meet(env, lo, hi)
            yb, pb, env2, asg = self.block(depth + 1, ne, ind + 1) if depth < 2 else ([f"{sp}  b = b + 1"], [f"{sp}  b = b + 1"], ne, False)
            ys = [f"{sp}if a >= {lo} && a <= {hi} {{"]
            ps = [f"{sp}if a >= {lo} and a <= {hi}:"]
            if finite(ne):
                v = self.fresh("p")
                ys.append(f"{sp}  let {v}: Int<{ne[0]}..{ne[1]}> = a")
                ys.append(f"{sp}  b = b + {v}")
                ps.append(f"{sp}  b = b + a")
            ys += yb
            ps += pb
            ys.append(f"{sp}}}")
            # after the `if`: either the branch ran (env2) or not (env: nothing is learnt)
            return ys, ps, hull(env2, env), asg
        if kind == "if":
            k = r.randint(-8, 8)
            op = r.choice(["<", "<=", ">", ">=", "==", "!="])
            pyop = op
            def cons(e, truth):
                o = op if truth else {"<": ">=", "<=": ">", ">": "<=", ">=": "<", "==": "!=", "!=": "=="}[op]
                if o == "<": return meet(e, -INF, k - 1)
                if o == "<=": return meet(e, -INF, k)
                if o == ">": return meet(e, k + 1, INF)
                if o == ">=": return meet(e, k, INF)
                if o == "==": return meet(e, k, k)
                # !=: only an end of the interval can be cut off
                if e is EMPTY: return EMPTY
                if e[0] == k == e[1]: return EMPTY
                if e[0] == k: return (e[0] + 1, e[1])
                if e[1] == k: return (e[0], e[1] - 1)
                return e
            te, ee = cons(env, True), cons(env, False)
            yt, pt, env_t, a1 = self.block(depth + 1, te, ind + 1)
            ye, pe, env_e, a2 = self.block(depth + 1, ee, ind + 1)
            ys = [f"{sp}if a {op} {k} {{"] + yt + [f"{sp}}} else {{"] + ye + [f"{sp}}}"]
            ps = [f"{sp}if a {pyop} {k}:"] + pt + [f"{sp}else:"] + pe
            return ys, ps, hull(env_t, env_e), a1 or a2
        if kind == "loop":
            i = self.fresh("i")
            t = r.randint(1, 4)
            kill = None
            # a loop may change a: no fact about it survives into or out of the loop when it does
            probe_env = env
            yb, pb, env_b, asg = self.block(depth + 1, probe_env, ind + 1)
            if asg:
                # regenerate the body without facts: the checker forgets them at the loop head
                yb, pb, env_b, asg = self.block(depth + 1, FULL, ind + 1)
            ys = [f"{sp}var {i} = 0", f"{sp}while {i} < {t} {{"] + yb + [f"{sp}  {i} = {i} + 1", f"{sp}}}"]
            ps = [f"{sp}for _ in range({t}):"] + (pb or [f"{sp}  pass"])
            return ys, ps, FULL if asg else env, asg
        if kind == "as_else":
            lo = r.randint(-15, 5)
            hi = lo + r.randint(0, 25)
            c = self.fresh("c")
            ne = meet(env, lo, hi)
            ys = [f"{sp}let {c} = a as Int<{lo}..{hi}> else {{ return -77 }}", f"{sp}b = b + {c} * 3"]
            ps = [f"{sp}if a < {lo} or a > {hi}:", f"{sp}  return -77", f"{sp}b = b + a * 3"]
            if finite(ne):
                v = self.fresh("p")
                ys.append(f"{sp}let {v}: Int<{ne[0]}..{ne[1]}> = a")
                ys.append(f"{sp}b = b + {v}")
                ps.append(f"{sp}b = b + a")
            return ys, ps, ne, False
        # match
        lo = r.randint(-10, 3)
        hi = lo + r.randint(0, 12)
        ne = meet(env, lo, hi)
        yb, pb, env_a, a1 = self.block(depth + 1, ne, ind + 2) if depth < 2 else ([f"{sp}    b = b + 1"], [f"{sp}    b = b + 1"], ne, False)
        ys = [f"{sp}match a {{", f"{sp}  {lo}..={hi} => {{"] + yb + [f"{sp}  }}", f"{sp}  _ => {{ b = b - 1 }}", f"{sp}}}"]
        ps = [f"{sp}if a >= {lo} and a <= {hi}:"] + (pb or [f"{sp}  pass"]) + [f"{sp}else:", f"{sp}  b = b - 1"]
        return ys, ps, FULL if a1 else hull(env_a, env), a1


def build_var_source(rng, cases):
    """Programs over two `var` integers; compares every backend with a Python model."""
    funcs, calls, expected = [], [], []
    for i in range(4):
        g = VarGen(rng)
        ys, ps, env, _ = g.block(0, FULL, 1)
        funcs.append(f"fn v{i}(n: Int) -> Int {{\n  var a = n\n  var b = 0\n" + "\n".join(ys) + "\n  return a * 7 + b\n}")
        code = f"def v{i}(n):\n  a = n\n  b = 0\n" + "\n".join(ps) + "\n  return a * 7 + b\n"
        scope = {}
        exec(code, scope)
        for _ in range(cases):
            n = rng.randint(-14, 14)
            calls.append(f"  print(v{i}({n}))")
            expected.append(str(scope[f"v{i}"](n)))
    source = "\n".join(funcs) + "\nfn main() {\n" + "\n".join(calls) + "\n}\n"
    return source, (0, ("\n".join(expected) + "\n").encode(), b"")


ADV_PRELUDE = """trait Bump { fn set(mut self, k: Int)
  fn push(mut self, k: Int) }
impl Bump for Int {
  fn set(mut self, k: Int) { self = self + k }
  fn push(mut self, k: Int) { self = self + k }
}
trait Peek { fn get(self, other: mut Int) -> Int }
impl Peek for Int { fn get(self, other: mut Int) -> Int { other = other + self
  return other } }
fn grow(v: mut Int, k: Int) { v = v + k }
fn apply(k: Int, f: view fn(Int)) { f(k) }
struct Cell { n: Int }
fn grow_cell(c: mut Cell, k: Int) { c.n = c.n + k }
"""


class AdvGen:
    """Adversarial mutations of a narrowed `var`: methods with `mut self` named like pure integer methods,
    `mut` arguments, closures, defers, tuple swaps, loops, shadowing. Every read of `a` inside a guarded
    region is a narrowed read, which `--check-ranges` verifies; the Python twin is the oracle."""

    def __init__(self, rng):
        self.rng = rng
        self.n = 0

    def fresh(self, base):
        self.n += 1
        return f"{base}{self.n}"

    def mutation(self, ind, defers):
        """-> (yerux lines, python lines)"""
        r = self.rng
        sp = "  " * ind
        k = r.randint(1, 40)
        kind = r.choice(["set", "push", "get", "grow", "closure", "swap", "loop", "defer", "shadow", "cell", "inner_if", "assign"])
        if kind == "set":
            return [f"{sp}a.set({k})"], [f"{sp}a = a + {k}"]
        if kind == "push":
            return [f"{sp}a.push({k})"], [f"{sp}a = a + {k}"]
        if kind == "get":
            # a user method taking `mut` argument: `k.get(a)` adds k to a
            kv = self.fresh("k")
            return [f"{sp}let {kv} = {k}", f"{sp}let {self.fresh('g')} = {kv}.get(a)"], [f"{sp}a = a + {k}"]
        if kind == "grow":
            return [f"{sp}grow(a, {k})"], [f"{sp}a = a + {k}"]
        if kind == "closure":
            return [f"{sp}apply({k}, |d| a = a + d)"], [f"{sp}a = a + {k}"]
        if kind == "swap":
            # (a tuple assignment cannot follow an expression statement directly: the parser would read a call)
            return [f"{sp}if b > -100000 {{", f"{sp}  (a, b) = (b, a)", f"{sp}}}"], [f"{sp}a, b = b, a"]
        if kind == "loop":
            c = self.fresh("c")
            return ([f"{sp}var {c} = 0", f"{sp}while {c} < 2 {{", f"{sp}  a = a + {k}", f"{sp}  {c} = {c} + 1", f"{sp}}}"],
                    [f"{sp}a = a + {2 * k}"])
        if kind == "defer":
            defers.append(([f"defer a = a + {k}"], [f"a = a + {k}"]))
            return [], []
        if kind == "shadow":
            # a new variable with the same name: the outer one (and its facts) is untouched
            return [f"{sp}if b > -100000 {{", f"{sp}  let a = {k * 1000}", f"{sp}  b = b + a", f"{sp}}}"], [f"{sp}b = b + {k * 1000}"]
        if kind == "cell":
            # a struct field that is also named `a` must not confuse anything
            return ([f"{sp}var cell = Cell {{ n: {k} }}", f"{sp}grow_cell(cell, {k})", f"{sp}b = b + cell.n"],
                    [f"{sp}b = b + {2 * k}"])
        if kind == "inner_if":
            return ([f"{sp}if b > -1000 {{", f"{sp}  a.set({k})", f"{sp}}}"], [f"{sp}if b > -1000:", f"{sp}  a = a + {k}"])
        return [f"{sp}a = a + {k}"], [f"{sp}a = a + {k}"]

    def use(self, ind):
        sp = "  " * ind
        v = self.fresh("u")
        return [f"{sp}let {v} = a", f"{sp}b = b + {v} * 2"], [f"{sp}b = b + a * 2"]

    def region(self, ind):
        r = self.rng
        sp = "  " * ind
        lo = r.randint(-10, 4)
        hi = lo + r.randint(2, 30)
        form = r.choice(["both", "lt", "gt", "while"])
        ys, ps = [], []
        defers = []
        body_y, body_p = [], []
        for _ in range(r.randint(2, 4)):
            if r.random() < 0.5:
                y, p = self.use(ind + 1)
            else:
                y, p = self.mutation(ind + 1, defers)
            body_y += y
            body_p += p
        body_y += self.use(ind + 1)[0]
        body_p += self.use(ind + 1)[1]
        # the deferred statements run when the block ends (last deferred first)
        dy = [f"{sp}  {d[0][0]}" for d in defers]
        dp = [f"{sp}  {d[1][0]}" for d in reversed(defers)]
        if form == "while":
            c = self.fresh("w")
            ys = [f"{sp}var {c} = 0", f"{sp}while {c} < 2 && a >= {lo} && a <= {hi} {{"] + dy + body_y + [f"{sp}  {c} = {c} + 1", f"{sp}}}"]
            ps = [f"{sp}{c} = 0", f"{sp}while {c} < 2 and a >= {lo} and a <= {hi}:"] + body_p + dp + [f"{sp}  {c} = {c} + 1"]
            if defers:
                # a deferred statement inside a loop body runs at the end of every pass: keep it simple, drop defers
                ys = [l for l in ys if "defer" not in l]
                ps = [f"{sp}{c} = 0", f"{sp}while {c} < 2 and a >= {lo} and a <= {hi}:"] + body_p + [f"{sp}  {c} = {c} + 1"]
            return ys, ps
        cond = {"both": (f"a >= {lo} && a <= {hi}", f"a >= {lo} and a <= {hi}"),
                "lt": (f"a < {hi}", f"a < {hi}"), "gt": (f"a > {lo}", f"a > {lo}")}[form]
        ys = [f"{sp}if {cond[0]} {{"] + dy + body_y + [f"{sp}}}"]
        ps = [f"{sp}if {cond[1]}:"] + body_p + dp + ([f"{sp}  pass"] if not (body_p or dp) else [])
        return ys, ps


def build_adv_source(rng, cases):
    """Adversarial mutation patterns of a narrowed `var`; every backend must agree with the Python model."""
    funcs, calls, expected = [], [], []
    for i in range(4):
        g = AdvGen(rng)
        ys, ps = [], []
        for _ in range(rng.randint(2, 4)):
            y, p = g.region(1)
            ys += y
            ps += p
            y, p = g.use(1)
            ys += y
            ps += p
        funcs.append(f"fn x{i}(n: Int) -> Int {{\n  var a = n\n  var b = 1\n" + "\n".join(ys) + "\n  return a * 7 + b\n}")
        code = f"def x{i}(n):\n  a = n\n  b = 1\n" + "\n".join(ps) + "\n  return a * 7 + b\n"
        scope = {}
        exec(code, scope)
        for _ in range(cases):
            n = rng.randint(-14, 14)
            calls.append(f"  print(x{i}({n}))")
            expected.append(str(scope[f"x{i}"](n)))
    source = ADV_PRELUDE + "\n".join(funcs) + "\nfn main() {\n" + "\n".join(calls) + "\n}\n"
    return source, (0, ("\n".join(expected) + "\n").encode(), b"")


def build_and_run(source, path, flags):
    cmd = [YERUX, "build", source, *flags, "-o", path]
    p = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
    if p.returncode:
        raise RuntimeError(p.stderr.decode(errors="replace"))
    p = subprocess.run([path], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
    return p.returncode, p.stdout, p.stderr


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-n", type=int, default=30, help="generated programs")
    ap.add_argument("--seed", type=int, default=50127)
    ap.add_argument("--cases", type=int, default=8, help="calls per operator in each program")
    args = ap.parse_args()
    rng = random.Random(args.seed)
    for i in range(args.n):
        source, expected = (build_source, build_flow_source, build_var_source, build_adv_source)[i % 4](rng, args.cases)
        with tempfile.TemporaryDirectory(prefix="yerux_range_fuzz_") as td:
            src = os.path.join(td, "range.yx")
            with open(src, "w", encoding="utf-8") as f:
                f.write(source)
            try:
                first = build_and_run(src, os.path.join(td, "c"), [])
            except RuntimeError as err:
                print(f"compile error seed={args.seed} program={i}\n{source}\n{err}")
                return 1
            outputs = [
                first,
                build_and_run(src, os.path.join(td, "native"), ["--native"]),
                build_and_run(src, os.path.join(td, "noopt"), ["--native", "--no-opt"]),
                build_and_run(src, os.path.join(td, "checked"), ["--native", "--check-ranges"]),
            ]
            if any(out != expected for out in outputs) or any(outputs[0] != o for o in outputs[1:]):
                print(f"mismatch seed={args.seed} program={i}")
                print(source)
                for label, out in zip(("C", "native", "no-opt", "check-ranges"), outputs):
                    print(label, out)
                return 1
    print(f"range fuzz: {args.n} programs, {args.cases} calls/operator, seed={args.seed}: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
