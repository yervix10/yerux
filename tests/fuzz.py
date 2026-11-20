#!/usr/bin/env python3
"""Differential fuzzer for the Yerux compiler.

Generates random programs in the native-supported subset (Int/Bool/Text, functions,
if/while, checked arithmetic, short-circuit logic, print), compiles each with the C
backend and the native backend (and native --no-opt when the binary supports that flag),
compares stdout / stderr / exit code and saves delta-minimized reproducers of any
mismatch into tests/bugs/.

usage: tests/fuzz.py [--restrict-order] [--a64] [-n COUNT] [--seed SEED] [-j JOBS] [--keep-going-saves MAX]
"""
import argparse, hashlib, os, random, shutil, subprocess, sys, tempfile, threading
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fuzz_num import NumGen

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
YERUX = os.path.join(ROOT, "target", "release", "yerux")
BUGS = os.path.join(ROOT, "tests", "bugs")
RUN_TIMEOUT = 10

MAXI = "9223372036854775807"
MINI = "(-9223372036854775807 - 1)"
TEXTS = ["hello", "", "a b  c", "50% off %d %s", "back\\slash", "café ñ 中文 \U0001F600",
         "??/ ?? trigraph", "tab\there", "x", "long " * 30, "// not a comment", "{ } ( )"]


def esc(t):
    """Text -> source literal body (escapes, and braces which would start an interpolation)."""
    return t.replace("\\", "\\\\").replace("{", "{{").replace("}", "}}")


class Gen:
    def __init__(self, seed, free_order=True):
        self.free_order = free_order
        self.r = random.Random(seed)
        self.extreme = self.r.choice([0.0, 0.02, 0.1, 0.25, 0.5])
        self.danger = self.r.choice([0.0, 0.03, 0.1, 0.3])
        self.funcs = []        # (name, [param types], ret type, cost)
        self.n = 0
        self.lines = []
        self.mult = 1
        self.cost = 0
        self.depth_loop = 0
        self.in_fn = False
        self.ret = None

    # ---- helpers
    def fresh(self, p):
        self.n += 1
        return f"{p}{self.n}"

    def emit(self, ind, s):
        self.lines.append("  " * ind + s)

    # ---- constants
    def const(self):
        r = self.r
        if r.random() < self.extreme:
            c = r.choice([
                MAXI, f"(-{MAXI})", MINI, "4611686018427387904", "3037000500", "3037000499",
                "4294967296", "(-4294967296)", "2147483648", f"({MAXI} - {r.randint(1, 5)})",
                f"(-{MAXI} + {r.randint(0, 5)})", "9223372036854775806", "(-1)", "0", "1", "2",
                "(-2)", "4611686018427387903", "(-4611686018427387904)", "1000000007"])
            return c
        v = r.choice([0, 1, 2, 3, 5, 7, 10, 100, 1000, r.randint(0, 50), r.randint(-50, 50)])
        return str(v) if v >= 0 else f"({v})"

    # ---- expressions
    def vars_of(self, env, t):
        return [n for n, ty, _ in env if ty == t]

    def callable_funcs(self, t):
        out = []
        for f in self.funcs:
            if f[2] == t and self.cost + self.mult * f[3] < 4000:
                out.append(f)
        return out

    # Expressions return (text, risky). "risky" = may panic or has side effects (calls print).
    # Evaluation order is defined left to right (SPEC.md), so risky operands may sit side by side
    # (function arguments, binary operands). `--restrict-order` brings back the old behaviour of never
    # putting two risky operands together.
    def safe_int(self, env):
        r = self.r
        vs = self.vars_of(env, "Int")
        k = r.random()
        if vs and k < 0.55:
            return r.choice(vs)
        if vs and k < 0.7:
            return f"({r.choice(vs)} {r.choice('/%')} {r.choice(['2', '3', '7', '10', '1000', '(-7)'])})"
        return self.const()

    def safe_bool(self, env):
        r = self.r
        vs = self.vars_of(env, "Bool")
        if vs and r.random() < 0.5:
            return r.choice(vs)
        if r.random() < 0.6:
            return f"({self.safe_int(env)} {r.choice(['<', '>', '<=', '>=', '==', '!='])} {self.safe_int(env)})"
        return r.choice(["true", "false"])

    def call(self, f, env, d, ok):
        self.cost += self.mult * f[3]
        args = []
        for t in f[1]:
            e, rk = (self.int_e(env, d - 1, ok) if t == "Int" else self.bool_e(env, d - 1, ok))
            if rk and not self.free_order:
                ok = False
            args.append(e)
        return f"{f[0]}({', '.join(args)})"

    def two(self, gen, env, d, ok):
        """Generate two operands honouring the one-risky-operand rule."""
        a, ra = gen(env, d - 1, ok)
        b, rb = gen(env, d - 1, ok and not (ra and not self.free_order))
        return a, b, ra or rb

    def int_e(self, env, d, ok=True):
        r = self.r
        if not ok and not self.free_order:
            return self.safe_int(env), False
        if d <= 0 or r.random() < 0.25:
            return self.safe_int(env), False
        k = r.random()
        if k < 0.07:
            e, _ = self.int_e(env, d - 1, ok)
            return f"(-{e})", True
        if k < 0.17:
            fs = self.callable_funcs("Int")
            if fs:
                return self.call(r.choice(fs), env, d, ok), True
        op = r.choice(["+", "+", "-", "-", "*", "*", "/", "%"])
        a, ra = self.int_e(env, d - 1, ok)
        ok2 = ok and not (ra and not self.free_order)
        if op in "/%":
            q = r.random()
            vs = self.vars_of(env, "Int")
            if q < self.danger:
                # may be zero (and MIN / -1 overflow): exercises the checks
                z = r.random()
                if z < 0.35 and vs:
                    b, rb = r.choice(vs), False
                elif z < 0.6:
                    b, rb = r.choice(["0", "(-1)", MAXI, MINI]), False
                elif ok2 or self.free_order:
                    x, y, rb = self.two(self.int_e, env, d, ok2)
                    b = f"({x} - {y})"   # subtraction itself may overflow
                else:
                    b, rb = "0", False
            elif q < 0.6:
                b, rb = r.choice(["1", "2", "3", "(-7)", "10", "(-3)", "1000", "(-1)"]), False
            else:
                x, rb = self.int_e(env, d - 1, ok2)
                b = f"({x} % 7 + 8)"   # never zero
        else:
            b, rb = self.int_e(env, d - 1, ok2)
        return f"({a} {op} {b})", True

    def bool_e(self, env, d, ok=True):
        r = self.r
        if not ok and not self.free_order:
            return self.safe_bool(env), False
        if d <= 0 or r.random() < 0.2:
            return self.safe_bool(env), False
        k = r.random()
        if k < 0.40:
            op = r.choice(["<", ">", "<=", ">=", "==", "!="])
            a, b, rk = self.two(self.int_e, env, d, ok)
            return f"({a} {op} {b})", rk
        if k < 0.50:
            e, rk = self.bool_e(env, d - 1, ok)
            return f"(!{e})", rk
        if k < 0.58:
            op = r.choice(["==", "!="])
            a, b, rk = self.two(self.bool_e, env, d, ok)
            return f"({a} {op} {b})", rk
        if k < 0.66:
            fs = self.callable_funcs("Bool")
            if fs:
                return self.call(r.choice(fs), env, d, ok), True
        if k < 0.76:
            vs = self.vars_of(env, "Int")
            if vs:
                x = r.choice(vs)
                y, ry = self.int_e(env, d - 1, ok)
                cmp_ = f"(({y} {r.choice('/%')} {x}) {r.choice(['>', '==', '<'])} {self.const()})"
                if r.random() < 0.5:   # guarded division: short-circuit must prevent the panic
                    return f"({x} != 0 && {cmp_})", ry
                return f"({x} == 0 || {cmp_})", ry
        op = r.choice(["&&", "||"])
        # && / || have a defined evaluation order: both sides may be risky
        a, ra = self.bool_e(env, d - 1, ok)
        b, rb = self.bool_e(env, d - 1, ok)
        return f"({a} {op} {b})", ra or rb

    # ---- statements
    def I(self, env, d):
        return self.int_e(env, d)[0]

    def B(self, env, d):
        return self.bool_e(env, d)[0]

    def block(self, ind, env, n, allow_ret):
        env = list(env)
        for _ in range(n):
            self.stmt(ind, env, allow_ret)

    def stmt(self, ind, env, allow_ret, top=False):
        r = self.r
        self.cost += self.mult
        k = r.random()
        ed = r.choice([1, 2, 3, 4])
        if r.random() < 0.06 and self.guarded_div(ind, env):
            return
        ints_assign = [n for n, t, a in env if t == "Int" and a]
        bools_assign = [n for n, t, a in env if t == "Bool" and a]
        if k < 0.14:
            nm = self.fresh("v")
            kw = r.choice(["let", "var", "var"])
            self.emit(ind, f"{kw} {nm} = {self.I(env, ed)}")
            env.append((nm, "Int", kw == "var"))
        elif k < 0.20:
            nm = self.fresh("b")
            kw = r.choice(["let", "var"])
            self.emit(ind, f"{kw} {nm} = {self.B(env, ed)}")
            env.append((nm, "Bool", kw == "var"))
        elif k < 0.34 and ints_assign:
            nm = r.choice(ints_assign)
            if r.random() < 0.3:
                self.emit(ind, f"{nm} = ({nm} * {r.choice([3, 31, 7, 1000003])} + {self.safe_int(env)}) % {r.choice([1000003, 65537, 97, MAXI])}")
            else:
                self.emit(ind, f"{nm} = {self.I(env, ed)}")
        elif k < 0.38 and bools_assign:
            self.emit(ind, f"{r.choice(bools_assign)} = {self.B(env, ed)}")
        elif k < 0.58:
            k2 = r.random()
            if k2 < 0.7:
                self.emit(ind, f"print({self.I(env, ed)})")
            elif k2 < 0.9:
                self.emit(ind, f"print({self.B(env, ed)})")
            else:
                self.emit(ind, f'print("{esc(r.choice(TEXTS))}")')
        elif k < 0.74 and ind < 5:
            self.emit(ind, f"if {self.B(env, ed)} {{")
            self.block(ind + 1, env, r.randint(1, 3), allow_ret)
            if r.random() < 0.6:
                self.emit(ind, "} else {")
                self.block(ind + 1, env, r.randint(1, 3), allow_ret)
            self.emit(ind, "}")
        elif k < 0.86 and ind < 5 and self.depth_loop < 2:
            self.loop(ind, env, allow_ret)
        elif k < 0.92 and allow_ret and self.ret is not None:
            e = self.I(env, ed) if self.ret == "Int" else self.B(env, ed)
            self.emit(ind, f"if {self.B(env, ed)} {{")
            self.emit(ind + 1, f"return {e}")
            self.emit(ind, "}")
        else:
            self.emit(ind, f"print({self.I(env, ed)})")

    # ---- range-analysis stress: bounded loops over boundary values and guarded divisions
    def bound(self, env):
        """A start/limit value: small constant, i64 boundary neighbour, or a variable."""
        r = self.r
        vs = self.vars_of(env, "Int")
        k = r.random()
        if vs and k < 0.3:
            return r.choice(vs)
        if k < 0.5:
            return r.choice([MAXI, f"({MAXI} - {r.randint(1, 4)})", MINI, f"({MINI} + {r.randint(1, 4)})",
                             f"(-{MAXI})", "4611686018427387904", "3037000500", "(-3037000500)"])
        return str(r.choice([0, 1, 2, 3, 5, 10, 100, 1000])) if r.random() < 0.8 else f"({r.randint(-9, -1)})"

    def range_loop(self, ind, env, allow_ret):
        r = self.r
        i, g = self.fresh("i"), self.fresh("g")
        step = r.choice(["+ 1", "+ 1", "+ 2", "- 1", "- 1", "- 3"])
        op = r.choice(["<", "<=", ">", ">=", "!="])
        cond = f"{i} {op} {self.bound(env)}"
        if r.random() < 0.3:   # a second, redundant-looking test next to the real one
            cond = f"{cond} && {i} {r.choice(['<', '>', '!=', '<=', '>='])} {self.bound(env)}"
        self.depth_loop += 1
        self.emit(ind, f"var {i} = {self.bound(env)}")
        self.emit(ind, f"var {g} = 0")
        self.emit(ind, f"while ({cond}) && {g} < {r.randint(1, 7)} {{")
        old = self.mult
        self.mult *= 7
        env2 = env + [(i, "Int", False)]
        n = r.randint(1, 3)
        for _ in range(n):
            k = r.random()
            vs = self.vars_of(env2, "Int")
            if k < 0.25:
                self.emit(ind + 1, f"print({i} {r.choice('+-*')} {r.choice(vs + ['1', '3', '1000'])})")
            elif k < 0.45:
                self.emit(ind + 1, f"print({r.choice(vs)} {r.choice('/%')} {i})")
            elif k < 0.55:
                self.emit(ind + 1, f"print({r.choice(vs)} {r.choice('/%')} ({i} {r.choice('+-')} {r.randint(0, 4)}))")
            else:
                self.stmt(ind + 1, env2, allow_ret)
        self.mult = old
        self.emit(ind + 1, f"{i} = {i} {step}")
        self.emit(ind + 1, f"{g} = {g} + 1")
        self.emit(ind, "}")
        self.depth_loop -= 1

    def for_loop(self, ind, env, allow_ret):
        """`for v in lo..hi [step k]` / `lo..=hi` with break / continue / defer inside (bounded iterations)."""
        r = self.r
        v, cnt = self.fresh("v"), self.fresh("c")
        lo = r.choice([self.bound(env), "0", "(-3)", MAXI + " - 4", "(-" + MAXI + ")"])
        hi = r.choice([self.bound(env), "5", "9", MAXI, "(-" + MAXI + " + 4)"])
        lo, hi = f"({lo})", f"({hi})"
        dots = r.choice(["..", "..="])
        step = r.choice(["", "", " step 2", " step 3", " step 7"])
        self.depth_loop += 1
        self.emit(ind, f"var {cnt} = 0")
        self.emit(ind, f"for {v} in {lo}{dots}{hi}{step} {{")
        old = self.mult
        self.mult *= 8
        env2 = env + [(v, "Int", False)]
        self.emit(ind + 1, f"{cnt} = {cnt} + 1")
        self.emit(ind + 1, f"if {cnt} > {r.randint(1, 6)} {{ break }}")
        if r.random() < 0.5:
            self.emit(ind + 1, f"defer print({cnt} + 1000)")
        if r.random() < 0.6:
            self.emit(ind + 1, f"if {v} % 2 == 0 {{ continue }}")
        self.emit(ind + 1, f"print({v})")
        for _ in range(r.randint(0, 2)):
            self.stmt(ind + 1, env2, allow_ret)
        self.mult = old
        self.emit(ind, "}")
        self.depth_loop -= 1

    def guarded_div(self, ind, env):
        r = self.r
        vs = self.vars_of(env, "Int")
        if not vs:
            return False
        d, a = r.choice(vs), r.choice(vs + [self.bound(env)])
        guard = r.choice([f"{d} > 0", f"{d} != 0", f"{d} < -1", f"{d} >= 1 && {d} < 5", f"{d} < 0", f"{d} != 0 && {d} != -1",
                          f"{d} > -3 && {d} < 3", f"{d} >= 0"])
        self.emit(ind, f"if {guard} {{")
        self.emit(ind + 1, f"print({a} / {d})")
        self.emit(ind + 1, f"print({a} % {d})")
        self.emit(ind, "}")
        return True

    def loop(self, ind, env, allow_ret):
        r = self.r
        if r.random() < 0.2:
            return self.for_loop(ind, env, allow_ret)
        if r.random() < 0.4:
            return self.range_loop(ind, env, allow_ret)
        i = self.fresh("i")
        self.depth_loop += 1
        if r.random() < 0.25:
            # halving loop: bounded by 64 iterations
            t = self.fresh("t")
            self.emit(ind, f"var {t} = {self.I(env, 2)}")
            self.emit(ind, f"while {t} != 0 {{")
            old = self.mult
            self.mult *= 64
            env2 = env + [(t, "Int", False)]
            self.block(ind + 1, env2, r.randint(1, 3), allow_ret)
            self.mult = old
            self.emit(ind + 1, f"{t} = {t} / 2")
            self.emit(ind, "}")
        else:
            k = r.randint(0, 6)
            self.emit(ind, f"var {i} = 0")
            cond = f"{i} < {k}"
            if r.random() < 0.2:
                cond += f" && {self.B(env, 1)}"
            self.emit(ind, f"while {cond} {{")
            old = self.mult
            self.mult *= max(k, 1)
            env2 = env + [(i, "Int", False)]
            self.block(ind + 1, env2, r.randint(1, 3), allow_ret)
            self.mult = old
            self.emit(ind + 1, f"{i} = {i} + 1")
            self.emit(ind, "}")
        self.depth_loop -= 1

    def function(self, idx):
        r = self.r
        ptypes = [r.choice(["Int", "Int", "Bool"]) for _ in range(r.randint(0, 3))]
        ret = r.choice(["Int", "Int", "Bool"])
        name = f"f{idx}"
        params = [(f"p{j}", t) for j, t in enumerate(ptypes)]
        env = [(n, t, True) for n, t in params]
        self.cost = 0
        self.mult = 1
        self.ret = ret
        self.emit(0, f"fn {name}({', '.join(f'{n}: {t}' for n, t in params)}) -> {ret} {{")
        self.block(1, env, self.r.randint(1, 6), True)
        # final return sees every param (locals are scoped inside block())
        self.emit(1, "return " + (self.I(env, 3) if ret == "Int" else self.B(env, 3)))
        self.emit(0, "}")
        self.emit(0, "")
        self.funcs.append((name, ptypes, ret, max(self.cost, 1)))
        self.ret = None

    def program(self):
        for i in range(self.r.randint(0, 4)):
            self.function(i)
        self.cost = 0
        self.mult = 1
        self.emit(0, "fn main() {")
        self.block(1, [], self.r.randint(3, 12), False)
        self.emit(0, "}")
        return self.lines



# ---------------------------------------------------------------- aggregate generator (--agg)
AGG_PRELUDE = """struct P { a: Int, name: String }
struct Q { x: Int, y: Int, z: Int }
struct PB { ok: Bool, bits: List<Bool> }

fn flipb(b: mut Bool) {
  b = !b
}

fn setb(l: mut List<Bool>, n: Int) {
  var k = 0
  while k < n {
    l.push(k % 3 != 1)
    k = k + 1
  }
}

fn cnt(l: view List<Bool>) -> Int {
  var k = 0
  var c = 0
  while k < l.len() {
    if l.get(k) {
      c = c + 1
    }
    k = k + 1
  }
  return c
}

fn grow(l: mut List<String>, n: Int) {
  var i = 0
  while i < n {
    l.push(String.from("g"))
    i = i + 1
  }
}

fn mk(n: Int) -> String {
  var s = String.from("m")
  s.push_int(n)
  return s
}

fn total(l: view List<String>) -> Int {
  var i = 0
  var t = 0
  while i < l.len() {
    t = t + l.at(i).len()
    i = i + 1
  }
  return t
}

fn bump(p: mut P) {
  p.a = p.a + 1
  p.name.push_str("+")
}

fn take(s: String) -> Int {
  return s.len()
}

fn early(flag: Bool) -> String {
  var a = String.from("a")
  var l = List<String>.new()
  l.push(String.from("x"))
  if flag {
    return a
  }
  a.push_str("-")
  return String.concat(a, l.at(0))
}
"""


class AggGen:
    """Random programs over String and List<T> (owned, multi-word and nested elements).
    Programs are valid by construction; `danger` controls how often indices may run out of range."""

    LITS = ["", "a", "hello", "x y", "0123456789abcdef0123", "é中"]

    def __init__(self, seed, free_order=False):
        self.r = random.Random(seed)
        self.danger = self.r.choice([0.0, 0.0, 0.02, 0.1])
        self.lines = []
        self.n = 0

    def emit(self, ind, s):
        self.lines.append("  " * ind + s)

    def fresh(self, p):
        self.n += 1
        return f"{p}{self.n}"

    def lit(self):
        return '"' + self.r.choice(self.LITS) + '"'

    def num(self, ctx):
        r = self.r
        k = r.random()
        if k < 0.5:
            return str(r.randint(0, 9))
        if k < 0.7:
            return r.choice(ctx["ints"])
        if k < 0.8:
            return f"{r.choice(['li', 'ls', 'lp', 'lq', 'lls'])}.len()"
        if k < 0.85:
            return f"({MINI})"
        return f"({r.choice(ctx['ints'])} * {r.randint(1, 1000)})"

    def bexp(self, ctx):
        r = self.r
        return r.choice(["true", "false", f"{self.num(ctx)} % 2 == 0", "lb.len() > 3", "s0.len() > 2",
                         f"{self.num(ctx)} > {self.num(ctx)}"])

    def idx(self, ctx, lst):
        """An index that is normally in range for the list (guarded by the caller)."""
        if self.r.random() < self.danger:
            return str(self.r.randint(0, 9))
        return f"{self.r.choice(ctx['small'])} % {lst}.len()"

    def stmt(self, ind, ctx):
        r = self.r
        k = r.randint(0, 41)
        s = r.choice(["s0", "s1", "s2"])
        if k == 0:
            self.emit(ind, f"{s}.push_str({self.lit()})")
        elif k == 1:
            self.emit(ind, f"{s}.push_int({self.num(ctx)})")
        elif k == 2:
            self.emit(ind, f"print({s})")
        elif k == 3:
            self.emit(ind, f"print({s}.len())")
        elif k == 4:
            self.emit(ind, f"print({s}.eq({self.lit()}))")
        elif k == 5:
            t = r.choice(["s0", "s1", "s2"])
            if t != s:
                self.emit(ind, f"print({s}.eq({t}))")
                self.emit(ind, f"{s}.push_str({t})")
        elif k == 6:
            self.emit(ind, f"{s} = String.from({self.lit()})")
        elif k == 7:
            t = r.choice(["s0", "s1", "s2"])
            self.emit(ind, f"{s} = String.concat({s if r.random() < 0.5 else t}, {t})")
        elif k == 8:
            self.emit(ind, f"if {s}.len() > 0 {{")
            self.emit(ind + 1, f"print({s}.byte({self.idx(ctx, s)}))")
            self.emit(ind, "}")
        elif k == 9:
            self.emit(ind, f"{s}.clear()")
        elif k == 10:
            self.emit(ind, f"li.push({self.num(ctx)})")
        elif k == 11:
            self.emit(ind, f"if li.len() > 0 {{")
            self.emit(ind + 1, f"print(li.get({self.idx(ctx, 'li')}))")
            self.emit(ind + 1, f"li.set({self.idx(ctx, 'li')}, {self.num(ctx)})")
            self.emit(ind, "}")
        elif k == 12:
            self.emit(ind, f"if li.len() > 0 {{")
            self.emit(ind + 1, f"print(li.pop())")
            self.emit(ind, "}")
        elif k == 13:
            self.emit(ind, f"ls.push(String.from({self.lit()}))")
        elif k == 14:
            self.emit(ind, f"if ls.len() > 0 {{")
            self.emit(ind + 1, f"ls.at({self.idx(ctx, 'ls')}).push_str({self.lit()})")
            self.emit(ind + 1, f"print(ls.at({self.idx(ctx, 'ls')}))")
            self.emit(ind + 1, f"ls.at({self.idx(ctx, 'ls')}).push_int({self.num(ctx)})")
            self.emit(ind, "}")
        elif k == 15:
            self.emit(ind, f"if ls.len() > 0 {{")
            self.emit(ind + 1, f"ls.set({self.idx(ctx, 'ls')}, String.from({self.lit()}))")
            self.emit(ind, "}")
        elif k == 16:
            self.emit(ind, f"if ls.len() > 0 {{")
            nm = self.fresh("pp")
            self.emit(ind + 1, f"let {nm} = ls.pop()")
            self.emit(ind + 1, f"print({nm})")
            self.emit(ind, "}")
        elif k == 17:
            self.emit(ind, "ls.clear()")
        elif k == 18:
            self.emit(ind, f"grow(ls, {r.randint(0, 40)})")
        elif k == 19:
            self.emit(ind, "print(total(ls))")
        elif k == 20:
            self.emit(ind, f"lp.push(P {{ a: {self.num(ctx)}, name: String.from({self.lit()}) }})")
        elif k == 21:
            self.emit(ind, "if lp.len() > 0 {")
            self.emit(ind + 1, f"bump(lp.at({self.idx(ctx, 'lp')}))")
            self.emit(ind + 1, f"lp.at({self.idx(ctx, 'lp')}).name.push_str({self.lit()})")
            self.emit(ind + 1, f"print(lp.at({self.idx(ctx, 'lp')}).name)")
            self.emit(ind + 1, f"print(lp.at({self.idx(ctx, 'lp')}).a)")
            self.emit(ind, "}")
        elif k == 22:
            self.emit(ind, "if lp.len() > 0 {")
            self.emit(ind + 1, f"lp.set({self.idx(ctx, 'lp')}, P {{ a: 1, name: String.from({self.lit()}) }})")
            nm = self.fresh("pp")
            self.emit(ind + 1, f"let {nm} = lp.pop()")
            self.emit(ind + 1, f"print({nm}.name)")
            self.emit(ind, "}")
        elif k == 23:
            self.emit(ind, f"lq.push(Q {{ x: {self.num(ctx)}, y: 2, z: {self.num(ctx)} }})")
        elif k == 24:
            self.emit(ind, "if lq.len() > 0 {")
            nm = self.fresh("q")
            self.emit(ind + 1, f"let {nm} = lq.get({self.idx(ctx, 'lq')})")
            self.emit(ind + 1, f"print({nm}.x + {nm}.y)")
            self.emit(ind + 1, f"lq.at({self.idx(ctx, 'lq')}).z = 5")
            self.emit(ind + 1, f"lq.set({self.idx(ctx, 'lq')}, {nm})")
            self.emit(ind + 1, f"let {self.fresh('q')} = lq.pop()")
            self.emit(ind, "}")
        elif k == 25:
            self.emit(ind, "lls.push(List<String>.new())")
        elif k == 26:
            self.emit(ind, "if lls.len() > 0 {")
            self.emit(ind + 1, f"lls.at({self.idx(ctx, 'lls')}).push(String.from({self.lit()}))")
            self.emit(ind + 1, f"print(lls.at({self.idx(ctx, 'lls')}).len())")
            self.emit(ind + 1, f"if lls.at(0).len() > 0 {{")
            self.emit(ind + 2, "print(lls.at(0).at(0))")
            self.emit(ind + 1, "}")
            self.emit(ind, "}")
        elif k == 27:
            self.emit(ind, "if lls.len() > 0 {")
            self.emit(ind + 1, "lls.at(0).clear()")
            self.emit(ind + 1, "lls.set(0, List<String>.new())")
            self.emit(ind, "}")
        elif k == 28:
            self.emit(ind, "if lls.len() > 2 {")
            nm = self.fresh("ll")
            self.emit(ind + 1, f"let {nm} = lls.pop()")
            self.emit(ind + 1, f"print({nm}.len())")
            self.emit(ind, "}")
        elif k == 29:
            nm = self.fresh("t")
            self.emit(ind, f"var {nm} = mk({self.num(ctx)})")
            self.emit(ind, f"if {self.num(ctx)} % 2 == 0 {{")
            self.emit(ind + 1, f"ls.push(move {nm})")
            self.emit(ind, "}")
        elif k == 30:
            nm = self.fresh("e")
            self.emit(ind, f"let {nm} = early({r.choice(['true', 'false'])})")
            self.emit(ind, f"print({nm})")
        elif k == 31:
            self.emit(ind, f"print(take(String.from({self.lit()})))")
        elif k == 34:
            self.emit(ind, f"lb.push({self.bexp(ctx)})")
        elif k == 35:
            self.emit(ind, "if lb.len() > 0 {")
            self.emit(ind + 1, f"print(lb.get({self.idx(ctx, 'lb')}))")
            self.emit(ind + 1, f"lb.set({self.idx(ctx, 'lb')}, {self.bexp(ctx)})")
            self.emit(ind + 1, f"print(lb.at({self.idx(ctx, 'lb')}))")
            self.emit(ind, "}")
        elif k == 36:
            self.emit(ind, "if lb.len() > 0 {")
            i = self.idx(ctx, 'lb')
            self.emit(ind + 1, f"flipb(lb.at({i}))")
            self.emit(ind + 1, f"print(lb.at({i}) || lb.get(0))")
            self.emit(ind, "}")
        elif k == 37:
            self.emit(ind, "if lb.len() > 0 {")
            self.emit(ind + 1, "print(lb.pop())")
            self.emit(ind, "}")
        elif k == 38:
            self.emit(ind, f"setb(lb, {r.randint(0, 150)})")
            self.emit(ind, "print(cnt(lb))")
            if r.random() < 0.4:
                self.emit(ind, "lb.clear()")
        elif k == 39:
            self.emit(ind, f"lpb.push(PB {{ ok: {self.bexp(ctx)}, bits: List<Bool>.new() }})")
            self.emit(ind, "if lpb.len() > 0 {")
            i = self.idx(ctx, 'lpb')
            self.emit(ind + 1, f"lpb.at({i}).bits.push({self.bexp(ctx)})")
            self.emit(ind + 1, f"setb(lpb.at({i}).bits, {r.randint(0, 40)})")
            self.emit(ind + 1, f"flipb(lpb.at({i}).bits.at(0))")
            self.emit(ind + 1, f"lpb.at({i}).ok = {self.bexp(ctx)}")
            self.emit(ind + 1, f"print(cnt(lpb.at({i}).bits))")
            self.emit(ind + 1, f"print(lpb.at({i}).ok)")
            self.emit(ind, "}")
        elif k == 40:
            self.emit(ind, "llb.push(List<Bool>.new())")
            self.emit(ind, "if llb.len() > 0 {")
            i = self.idx(ctx, 'llb')
            self.emit(ind + 1, f"llb.at({i}).push({self.bexp(ctx)})")
            self.emit(ind + 1, f"setb(llb.at({i}), {r.randint(0, 40)})")
            self.emit(ind + 1, f"flipb(llb.at({i}).at(0))")
            self.emit(ind + 1, f"print(llb.at({i}).get(0))")
            self.emit(ind + 1, f"print(llb.at({i}).len())")
            self.emit(ind, "}")
        elif k == 41:
            self.emit(ind, "if lpb.len() > 0 {")
            nm = self.fresh("pb")
            self.emit(ind + 1, f"let {nm} = lpb.pop()")
            self.emit(ind + 1, f"print({nm}.ok)")
            self.emit(ind + 1, f"print(cnt({nm}.bits))")
            self.emit(ind, "}")
            self.emit(ind, "if llb.len() > 1 {")
            nm = self.fresh("rb")
            self.emit(ind + 1, f"let {nm} = llb.pop()")
            self.emit(ind + 1, f"print({nm}.len() + cnt({nm}))")
            self.emit(ind, "}")
        elif k == 32 and ind < 4:
            i = self.fresh("i")
            self.emit(ind, f"var {i} = 0")
            self.emit(ind, f"while {i} < {r.randint(0, 5)} {{")
            ctx2 = dict(ctx, small=ctx["small"] + [i], ints=ctx["ints"] + [i])
            for _ in range(r.randint(1, 4)):
                self.stmt(ind + 1, ctx2)
            self.emit(ind + 1, f"{i} = {i} + 1")
            self.emit(ind, "}")
        elif k == 33 and ind < 4:
            self.emit(ind, f"if {self.num(ctx)} % 2 == 0 {{")
            for _ in range(r.randint(1, 3)):
                self.stmt(ind + 1, ctx)
            self.emit(ind, "} else {")
            for _ in range(r.randint(1, 3)):
                self.stmt(ind + 1, ctx)
            self.emit(ind, "}")

    def program(self):
        self.lines = AGG_PRELUDE.split("\n")
        self.emit(0, "fn main() {")
        for sv in ("s0", "s1", "s2"):
            self.emit(1, f"var {sv} = String.from({self.lit()})")
        self.emit(1, "var li = List<Int>.new()")
        self.emit(1, "var ls = List<String>.new()")
        self.emit(1, "var lp = List<P>.new()")
        self.emit(1, "var lq = List<Q>.new()")
        self.emit(1, "var lls = List<List<String>>.new()")
        self.emit(1, "var lb = List<Bool>.new()")
        self.emit(1, "var lpb = List<PB>.new()")
        self.emit(1, "var llb = List<List<Bool>>.new()")
        self.emit(1, "var n0 = 3")
        ctx = {"ints": ["n0"], "small": ["n0", "1", "2", "7"]}
        for _ in range(self.r.randint(5, 40)):
            self.stmt(1, ctx)
        for v in ("s0", "ls", "lp", "lls", "lb", "lpb", "llb"):
            self.emit(1, f"print({v}.len())" if v != "s0" else "print(s0)")
        self.emit(0, "}")
        return self.lines


# ---------------------------------------------------------------- running
def sh(args, timeout=RUN_TIMEOUT):
    try:
        p = subprocess.run(args, capture_output=True, timeout=timeout, stdin=subprocess.DEVNULL)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return -999, b"", b"TIMEOUT"


# ---------------------------------------------------------------- struct generator (--structs)
STRUCT_DEFS = """struct A { x: Int, y: Int }
struct B { x: Int, y: Int, z: Int }
struct N { a: A, b: B, k: Int }
"""
STRUCT_FIELDS = {"A": ["x", "y"], "B": ["x", "y", "z"]}


class StructGen:
    """Random programs over small Copy structs (A, B, nested N): many tiny functions taking structs by value,
    `view` and `mut`, returning structs, early returns, loops and self recursion. Used to test the inliner and
    scalar replacement. Overflow/division panics are possible and must agree across backends."""

    def __init__(self, seed, free_order=False):
        self.r = random.Random(seed)
        self.lines = []
        self.n = 0
        self.funcs = []  # (name, [(mode, ty)], ret ty or None)
        self.ro = set()  # view parameters of the function being generated (cannot be lent as mut)

    def emit(self, ind, s):
        self.lines.append("  " * ind + s)

    def fresh(self, p):
        self.n += 1
        return f"{p}{self.n}"

    def paths(self, vars_, want):
        """All places (strings) of type `want` reachable from the variables: (path, root)."""
        out = []
        for name, ty in vars_:
            if ty == "N":
                cands = [(name, "N"), (f"{name}.a", "A"), (f"{name}.b", "B"), (f"{name}.k", "Int")]
                cands += [(f"{name}.a.{f}", "Int") for f in STRUCT_FIELDS["A"]]
                cands += [(f"{name}.b.{f}", "Int") for f in STRUCT_FIELDS["B"]]
            elif ty == "Int":
                cands = [(name, "Int")]
            else:
                cands = [(name, ty)] + [(f"{name}.{f}", "Int") for f in STRUCT_FIELDS[ty]]
            out += [(pth, name) for pth, t in cands if t == want]
        return out

    def small(self):
        return str(self.r.choice([0, 1, 2, 3, 5, 7, 10, 100, 999, 1000003, -4, -1]))

    def iexpr(self, vars_, d, may_call=True):
        r = self.r
        k = r.random()
        ps = self.paths(vars_, "Int")
        if d <= 0 or k < 0.3:
            return r.choice(ps)[0] if ps and r.random() < 0.75 else self.small()
        if k < 0.75:
            op = r.choice(["+", "-", "*", "+", "-", "%"])
            a, b = self.iexpr(vars_, d - 1, may_call), self.iexpr(vars_, d - 1, may_call)
            if r.random() < 0.05:
                return f"({a} + {MAXI})"
            if r.random() < 0.04:
                return f"({a} / ({b} % 3))"
            if op == "%":
                return f"({a} % {r.choice(['7', '1000', '13'])})"
            if op == "*":
                if r.random() < 0.15:
                    return f"({a} * {b} * 4000000000)"
                return f"(({a} % 1000) * ({b} % 1000))"
            return f"({a} {op} {b})"
        if may_call:
            c = self.call(vars_, d - 1, "Int")
            if c:
                return c
        return r.choice(ps)[0] if ps else self.small()

    def sexpr(self, vars_, d, ty, may_call=True):
        r = self.r
        k = r.random()
        ps = self.paths(vars_, ty)
        if k < 0.35 and ps:
            return r.choice(ps)[0]
        if k < 0.6 and may_call:
            c = self.call(vars_, d - 1, ty)
            if c:
                return c
        if ty == "N":
            return f"N {{ a: {self.sexpr(vars_, d - 1, 'A', may_call)}, b: {self.sexpr(vars_, d - 1, 'B', may_call)}, k: {self.iexpr(vars_, d - 1, may_call)} }}"
        fs = STRUCT_FIELDS[ty]
        return f"{ty} {{ " + ", ".join(f"{f}: {self.iexpr(vars_, d - 1, may_call)}" for f in fs) + " }"

    def call(self, vars_, d, ret):
        """A call of an earlier function returning `ret` (None = statement call); None if impossible."""
        r = self.r
        cands = [f for f in self.funcs if f[2] == ret]
        if not cands or d < 0:
            return None
        name, params, _ = r.choice(cands)
        used = set()
        args = []
        for mode, ty in params:
            if mode == "val":
                args.append(self.iexpr(vars_, d, True) if ty == "Int" else self.sexpr(vars_, d, ty, True))
            else:
                ps = [(p, root) for p, root in self.paths(vars_, ty) if root not in used and root not in self.ro and not any(p == u or p.startswith(u + ".") or u.startswith(p + ".") for u in used)]
                if not ps:
                    return None
                p, root = r.choice(ps)
                used.add(p)
                used.add(root)
                args.append(p)
        # a by-value argument must not mention a root that is passed as mut/view in the same call: pass-by-reference
        # + evaluation order is not what we test, so require disjointness textually
        refs = [a for a, (m, _) in zip(args, params) if m != "val"]
        for a, (m, _) in zip(args, params):
            if m == "val" and any(x.split(".")[0] in a for x in refs):
                return None
        return f"{name}({', '.join(args)})"

    def stmt(self, ind, vars_, params, d, ret):
        r = self.r
        k = r.random()
        muts = [(n, t) for n, t in vars_ if n in params and params[n] == "mut" or n not in params]
        if k < 0.35:
            tys = ["Int", "A", "B", "N"]
            pick = [(p, root) for t in tys for p, root in self.paths(muts, "Int")]
            if pick:
                p, _ = r.choice(pick)
                self.emit(ind, f"{p} = {self.iexpr(vars_, 2)}")
                return
        if k < 0.5:
            ty = r.choice(["A", "B", "N"])
            pk = self.paths(muts, ty)
            if pk:
                self.emit(ind, f"{r.choice(pk)[0]} = {self.sexpr(vars_, 2, ty)}")
                return
        if k < 0.62:
            ty = r.choice(["A", "B", "N"])
            nm = self.fresh("t")
            self.emit(ind, f"var {nm} = {self.sexpr(vars_, 2, ty)}")
            vars_.append((nm, ty))
            return
        if k < 0.75 and d > 0:
            self.emit(ind, f"if {self.iexpr(vars_, 1)} {r.choice(['<', '>', '==', '!='])} {self.iexpr(vars_, 1)} {{")
            inner = list(vars_)
            for _ in range(r.randint(1, 2)):
                self.stmt(ind + 1, inner, params, d - 1, ret)
            if ret and r.random() < 0.4:
                self.emit(ind + 1, f"return {self.ret_expr(inner, ret)}")
            if r.random() < 0.4:
                self.emit(ind, "} else {")
                inner = list(vars_)
                self.stmt(ind + 1, inner, params, d - 1, ret)
            self.emit(ind, "}")
            return
        if k < 0.85 and d > 0:
            c = self.fresh("i")
            self.emit(ind, f"var {c} = 0")
            self.emit(ind, f"while {c} < {r.randint(1, 4)} {{")
            inner = list(vars_) + [(c, "Int")]
            for _ in range(r.randint(1, 2)):
                self.stmt(ind + 1, [v for v in inner if v[0] != c], params, d - 1, ret)
            self.emit(ind + 1, f"{c} = {c} + 1")
            self.emit(ind, "}")
            return
        if k < 0.92:
            c = self.call(vars_, 2, None)
            if c:
                self.emit(ind, c)
                return
        if k < 0.97:
            self.emit(ind, f"print({self.iexpr(vars_, 2)})")
            return
        self.emit(ind, f"print({self.iexpr(vars_, 1)})")

    def ret_expr(self, vars_, ret):
        return self.iexpr(vars_, 2) if ret == "Int" else self.sexpr(vars_, 2, ret)

    def func(self, idx):
        r = self.r
        name = f"f{idx}"
        params = []
        pm = {}
        for j in range(r.randint(0, 3)):
            mode = r.choice(["val", "val", "view", "mut"])
            ty = r.choice(["Int", "A", "B", "N", "A", "B"])
            if mode != "val" and ty == "Int" and mode == "view":
                mode = "val"
            params.append((mode, ty))
        ret = r.choice([None, "Int", "A", "B", "N", "A", "B"])
        sig = []
        vars_ = []
        for j, (mode, ty) in enumerate(params):
            pn = f"p{j}"
            sig.append(f"{pn}: {'' if mode == 'val' else mode + ' '}{ty}")
            vars_.append((pn, ty))
            pm[pn] = mode
        # view params are read-only: mark them so the generator does not assign through them
        self.emit(0, f"fn {name}({', '.join(sig)})" + (f" -> {ret}" if ret else "") + " {")
        # by-value params are fresh locals (assignable); view params are not
        roparams = {n for n, m in pm.items() if m == "view"}
        self.ro = roparams
        assignable = {n: ("mut" if m in ("mut", "val") else "view") for n, m in pm.items()}
        vs = list(vars_)
        for _ in range(r.randint(0, 4)):
            self.stmt(1, vs, {n: m for n, m in assignable.items() if n not in roparams} | {n: "view" for n in roparams}, 2, ret)
        if ret:
            self.emit(1, f"return {self.ret_expr(vs, ret)}")
        self.emit(0, "}")
        self.funcs.append((name, params, ret))

    def program(self):
        r = self.r
        self.lines = STRUCT_DEFS.splitlines()
        self.emit(0, "")
        if r.random() < 0.5:
            self.emit(0, "fn rec(n: Int, p: A) -> A {")
            self.emit(1, "if n <= 0 { return p }")
            self.emit(1, "if n > 12 { return p }")
            self.emit(1, "let q = rec(n - 1, A { x: p.y, y: (p.x + 1) % 1000 })")
            self.emit(1, "return A { x: (q.x + p.x) % 1000, y: q.y }")
            self.emit(0, "}")
            self.funcs.append(("rec", [("val", "Int"), ("val", "A")], "A"))
        for i in range(r.randint(2, 7)):
            self.func(i)
            self.emit(0, "")
        self.ro = set()
        self.emit(0, "fn main() {")
        vs = []
        for ty, nm in (("A", "a"), ("B", "b"), ("N", "n"), ("Int", "s")):
            self.emit(1, f"var {nm} = {self.sexpr(vs, 1, ty, False) if ty != 'Int' else self.small()}")
            vs.append((nm, ty))
        loop = r.random() < 0.6
        if loop:
            self.emit(1, "var it = 0")
            self.emit(1, f"while it < {r.randint(1, 20)} {{")
        ind = 2 if loop else 1
        ctx = list(vs)
        for _ in range(r.randint(3, 10)):
            self.stmt(ind, ctx, {n: "mut" for n, _ in vs}, 2, None)
        if loop:
            self.emit(2, "it = it + 1")
            self.emit(1, "}")
        for pth, _ in [p for t in ("Int",) for p in self.paths(vs, t)]:
            self.emit(1, f"print({pth})")
        self.emit(0, "}")
        return self.lines


def detect_noopt():
    try:
        with open(YERUX, "rb") as f:
            return b"--no-opt" in f.read()
    except OSError:
        return False


class Runner:
    def __init__(self, tmp, noopt, a64=False):
        self.tmp = tmp
        self.noopt = noopt
        self.a64 = a64
        self.lock = threading.Lock()
        self.counter = 0

    def uid(self):
        with self.lock:
            self.counter += 1
            return self.counter

    def backends(self):
        b = [("c", []), ("native", ["--native"])]
        if self.noopt:
            b.append(("native-noopt", ["--native", "--no-opt"]))
        return b

    def evaluate(self, lines, *, skip_timeouts=True):
        """Returns None if the program is rejected by `check`, else dict backend -> (code, out, err)."""
        u = self.uid()
        src = os.path.join(self.tmp, f"p{u}.yx")
        with open(src, "w") as f:
            f.write("\n".join(lines) + "\n")
        res = {}
        try:
            rc, _, err = sh([YERUX, "check", src])
            if rc != 0:
                return None, err
            for name, flags in self.backends():
                exe = os.path.join(self.tmp, f"p{u}_{name}")
                rc, out, err = sh([YERUX, "build", src, "-o", exe] + flags, timeout=60)
                if rc != 0:
                    if b"native backend:" in err:
                        res[name] = "SKIP"
                    else:
                        res[name] = (-1000, b"", b"BUILD FAILED: " + err)
                    continue
                res[name] = sh([exe])
                try:
                    os.remove(exe)
                except OSError:
                    pass
            if self.a64:
                # Use the same emulator as the full a64 suite, but compare directly with this program's C result.
                import a64_emu
                a64tmp = os.path.join(self.tmp, f"p{u}_a64")
                os.makedirs(a64tmp)
                ec, so, se, err, _ = a64_emu.a64_run(src, a64tmp, [], check_decode=False)
                res["aarch64"] = (-1000, b"", (err or "aarch64 emulation failed").encode()) if err else (ec, so, se)
            # A backend timeout under machine load is not a semantic mismatch. Keep it out of
            # the minimizer, which would otherwise “reproduce” transient scheduler delays.
            if skip_timeouts and any(isinstance(v, tuple) and v[0] == -999 for v in res.values()):
                return {name: "SKIP" for name in res}, None
        finally:
            try:
                os.remove(src)
            except OSError:
                pass
        return res, b""


def leak_check(runner, lines, *, require_success=False):
    """--agg: a native --native-debug-alloc build of a non-panicking program must end with `live: 0`."""
    u = runner.uid()
    src = os.path.join(runner.tmp, f"l{u}.yx")
    exe = os.path.join(runner.tmp, f"l{u}_leak")
    with open(src, "w") as f:
        f.write("\n".join(lines) + "\n")
    try:
        rc, _, err = sh([YERUX, "build", src, "-o", exe, "--native-debug-alloc"], timeout=60)
        if rc != 0:
            return "leak build failed: " + err.decode(errors="replace")[:200]
        rc, _, err = sh([exe])
        if require_success and rc != 0:
            return f"debug allocation run exited with {rc}: {err.decode(errors='replace')[:200]}"
        if rc == 0 and b"live: 0\n" not in err:
            return "leak: " + err.decode(errors="replace").replace("\n", " ")
        return None
    finally:
        for f in (src, exe):
            try:
                os.remove(f)
            except OSError:
                pass


def mismatch_key(res):
    """Which (backend, field) pairs differ from the C reference; None when all agree."""
    ref = res["c"]
    key = []
    for name, v in res.items():
        if name == "c" or v == "SKIP":
            continue
        for i, field in enumerate(("code", "stdout", "stderr")):
            if v[i] != ref[i]:
                key.append((name, field))
    return tuple(key) or None


# ---------------------------------------------------------------- minimizer
def block_ranges(lines):
    out = []
    for i, l in enumerate(lines):
        if l.rstrip().endswith("{") and not l.lstrip().startswith("}"):
            depth = 0
            for j in range(i, len(lines)):
                s = lines[j]
                if s.rstrip().endswith("{"):
                    depth += 1
                if s.lstrip().startswith("}"):
                    depth -= 1 if not s.rstrip().endswith("{") else 0
                    if s.rstrip().endswith("{"):
                        pass
                if depth == 0 or (s.lstrip().startswith("}") and not s.rstrip().endswith("{") and depth == 0):
                    out.append((i, j))
                    break
    return out


def minimize(lines, runner, key):
    def bad(cand):
        res, _ = runner.evaluate(cand)
        return res is not None and mismatch_key(res) == key

    cur = [l for l in lines]
    changed = True
    while changed:
        changed = False
        # ddmin over chunks of lines
        n = max(len(cur) // 2, 1)
        while n >= 1:
            i = 0
            while i < len(cur):
                cand = cur[:i] + cur[i + n:]
                if cand and bad(cand):
                    cur = cand
                    changed = True
                else:
                    i += n
            n //= 2
        # remove whole blocks / unwrap them (drop the header and closing brace, keep the body)
        for (a, b) in reversed(block_ranges(cur)):
            for cand in (cur[:a] + cur[b + 1:], cur[:a] + cur[a + 1:b] + cur[b + 1:]):
                if cand and bad(cand):
                    cur = cand
                    changed = True
                    break
            else:
                continue
            break
    # drop blank lines
    cur = [l for l in cur if l.strip()]
    return cur


# ---------------------------------------------------------------- main
def fmt(v):
    if v == "SKIP":
        return "SKIP"
    return f"code={v[0]}\n--stdout--\n{v[1].decode(errors='replace')}--stderr--\n{v[2].decode(errors='replace')}"


def concurrent_program(seed):
    rng = random.Random(seed)
    workers = rng.randint(2, 8)
    lines = [
        "fn worker(counter: Int, rounds: Int, step: Int) -> Int {",
        "  var sum = 0",
        "  for i in 0..rounds {",
        "    var scratch = List<Int>.new()",
        "    scratch.push(i * step)",
        "    sum = sum + scratch.get(0)",
        "    __atomic_fetch_add(counter, step)",
        "  }",
        "  return sum",
        "}",
        "fn main() {",
        "  let counter = __alloc(8)",
        "  __atomic_store(counter, 0)",
    ]
    results = []
    total = 0
    for i in range(workers):
        rounds = rng.randint(0, 200)
        step = rng.randint(-40, 40)
        results.append(rounds * (rounds - 1) // 2 * step)
        total += rounds * step
        lines.append(f"  let worker{i} = __thread_start(move || worker(counter, {rounds}, ({step})))")
    for i, result in enumerate(results):
        lines.append(f"  let result{i} = __thread_wait(worker{i})")
        lines.append(f"  assert(result{i} == ({result}))")
        lines.append(f"  print(result{i})")
    lines += [f"  assert(__atomic_load(counter) == ({total}))", "  __free(counter)", "}"]
    return lines


def structured_concurrent_program(seed):
    rng = random.Random(seed)
    lines = [
        "fn worker(counter: view Shared<Atomic<Int>>, rounds: Int, step: Int) -> Int {",
        "  var sum = 0",
        "  for i in 0..rounds {",
        "    var scratch = List<Int>.new()",
        "    scratch.push(i * step)",
        "    sum = sum + scratch.get(0)",
        "    counter.add_wrapping(step)",
        "  }",
        "  return sum",
        "}",
        "fn failing_worker(rounds: Int) -> Int {",
        "  var scratch = List<Int>.new()",
        "  for i in 0..rounds { scratch.push(i) }",
        "  return scratch.get(rounds)",
        "}",
        "fn main() {",
        "  let counter = Shared.new(Atomic.new(0))",
        "  scope {",
    ]
    results = []
    total = 0
    for i in range(rng.randint(2, 8)):
        rounds = rng.randint(0, 200)
        step = rng.randint(-40, 40)
        results.append(rounds * (rounds - 1) // 2 * step)
        total += rounds * step
        lines.append(f"    let counter{i} = counter.clone()")
        lines.append(f"    let worker{i} = spawn move || worker(counter{i}, {rounds}, ({step}))")
    lines.append(f"    let failed = spawn move || failing_worker({rng.randint(1, 200)})")
    for i, result in enumerate(results):
        lines.append(f"    let result{i} = worker{i}.join().unwrap()")
        lines.append(f"    assert(result{i} == ({result}))")
        lines.append(f"    print(result{i})")
    end = rng.randint(0, 200)
    total += end * (end - 1) // 2
    lines += [
        "    assert(failed.join().is_err())",
        "  }",
        f"  parallel for index in 0..{end} {{ counter.add_wrapping(index) }}",
        f"  assert(counter.load() == ({total}))",
        "}",
    ]
    return lines


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-n", type=int, default=3000)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("-j", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--restrict-order", dest="free_order", action="store_false", default=True,
                    help="never put two risky (panicking / printing) operands side by side (default: do, order is left to right)")
    ap.add_argument("--free-order", dest="free_order", action="store_true", help=argparse.SUPPRESS)  # now the default
    ap.add_argument("--a64", action="store_true", help="also compare against the aarch64 backend under Unicorn")
    ap.add_argument("--agg", action="store_true", help="generate programs over String and List<T> (aggregates) instead of scalars")
    ap.add_argument("--structs", action="store_true", help="generate programs over small Copy structs (inliner/SROA stress)")
    ap.add_argument("--coll", action="store_true", help="collections: random Map/Set/List/String operations, C vs native vs a Python model")
    ap.add_argument("--num", action="store_true", help="generate programs over Float / Float32 / sized integers (tests/fuzz_num.py)")
    ap.add_argument("--max-saves", type=int, default=15)
    ap.add_argument("--conc", action="store_true", help="generate concurrent allocations and atomic updates on x86-64 Linux")
    a = ap.parse_args()
    if a.conc and (a.a64 or a.agg or a.structs or a.coll or a.num):
        ap.error("--conc requires the x86-64 Linux concurrency generator")
    if a.coll:
        import fuzz_coll
        return fuzz_coll.run(a.n, a.seed, a.j, a.a64)
    seed0 = a.seed if a.seed is not None else random.randrange(1 << 30)
    os.makedirs(BUGS, exist_ok=True)
    noopt = detect_noopt()
    tmp = tempfile.mkdtemp(prefix="yerux_fuzz_")
    runner = Runner(tmp, noopt, a.a64)
    print(f"fuzz: seed={seed0} n={a.n} jobs={a.j} backends={[b for b, _ in runner.backends()]}")
    stats = dict(run=0, invalid=0, mismatch=0, skipped=0, panics=0, build_fail=0)
    found = {}
    lock = threading.Lock()

    def one(i):
        seed = seed0 + i
        if a.conc:
            lines = structured_concurrent_program(seed) if seed % 2 else concurrent_program(seed)
        else:
            generator = NumGen if a.num else StructGen if a.structs else AggGen if a.agg else Gen
            lines = generator(seed, a.free_order).program()
        res, err = runner.evaluate(lines, skip_timeouts=not a.conc)
        with lock:
            stats["run"] += 1
            if res is None:
                stats["invalid"] += 1
                print(f"generator bug: seed {seed} rejected by check: {err.decode()[:200]}")
                return
            if not a.conc and any(v == "SKIP" for v in res.values()):
                stats["skipped"] += 1
                return
            if res["c"][0] == 101:
                stats["panics"] += 1
            key = mismatch_key(res)
            if key is None and a.conc and any(v == "SKIP" or v[0] != 0 for v in res.values()):
                key = (("concurrency model", "unexpected failure"),)
            if key is None and (a.agg or a.conc):
                bad = leak_check(runner, lines, require_success=a.conc)
                if bad:
                    key = (("native-debug-alloc", bad),)
            if key is None:
                return
            stats["mismatch"] += 1
        # minimize outside the lock
        print(f"MISMATCH seed={seed} {key}; minimizing ({len(lines)} lines)")
        small = lines if a.conc else minimize(lines, runner, key)
        res2, _ = runner.evaluate(small, skip_timeouts=not a.conc)
        text = "\n".join(small) + "\n"
        h = hashlib.sha1(text.encode()).hexdigest()[:8]
        with lock:
            if h in found or len(found) >= a.max_saves:
                return
            found[h] = key
            base = os.path.join(BUGS, f"fuzz_{h}")
            with open(base + ".yx", "w") as f:
                f.write(text)
            with open(base + ".txt", "w") as f:
                f.write(f"seed: {seed}\nmismatch: {key}\n\n")
                for name, v in (res2 or res).items():
                    f.write(f"=== {name}\n{fmt(v)}\n")
            print(f"  saved {base}.yx ({len(small)} lines)")

    with ThreadPoolExecutor(a.j) as ex:
        list(ex.map(one, range(a.n)))
    shutil.rmtree(tmp, ignore_errors=True)
    print(f"done: {stats} distinct reproducers saved: {len(found)}")
    return 1 if stats["mismatch"] or stats["invalid"] else 0


if __name__ == "__main__":
    sys.exit(main())
