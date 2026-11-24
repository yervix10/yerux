#!/usr/bin/env python3
"""Evaluation-order fuzzer: SPEC says operands are evaluated left to right in every backend, so with several
panicking operands the FIRST one in source order is the panic that must fire (and the `pr(n)` operands before it
must have printed). Each program fills 2-3 "holes" of a syntactic context (binary operators, call arguments,
interpolation, tuples, struct literals, for-range bounds, match guards, loops, generic and inlined helpers, ...)
with operands that are fine, print, or panic in different ways. Compares C, native, native --no-opt (and, on a
mismatch, bisects over YERUX_OPT_OFF passes / YERUX_NO_INLINE) and checks the panic against the source-order oracle
for contexts where every hole is always evaluated.

usage: tests/fuzz_order.py [--a64] [-n COUNT] [--seed SEED] [-j JOBS]"""
import argparse, os, random, subprocess, sys, tempfile, threading
from concurrent.futures import ThreadPoolExecutor

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
YERUX = os.path.join(ROOT, "target", "release", "yerux")

PRELUDE = """\
struct P { a: Int, b: Int, c: Int }
struct Acc { n: Int }
impl Acc {
  fn add3(mut self, a: Int, b: Int, c: Int) { self.n = self.n + a + b + c }
}
fn big() -> Int { return 9223372036854775807 }
fn low() -> Int { return -9223372036854775807 - 1 }
fn zero() -> Int { return 0 }
fn pr(x: Int) -> Int { print(x)
  return x }
fn pm(x: Int) -> Int { return x * big() }
fn zdiv(x: Int) -> Int { return x / zero() }
fn gsel<T>(a: T, b: T) -> T { return b }
fn f3(a: Int, b: Int, c: Int) -> Int { return a + b + c }
fn wrap(a: Int, b: Int, c: Int) -> Int { return a * 1 + b * 1 + c * 1 }
fn gwrap<T>(a: T, b: Int, c: Int) -> Int { return b + c }
"""
SETUP = """\
  var k = 7
  var zz = 0
  var bg = 9223372036854775807
  var l = List<Int>.new()
  l.push(1)
  var e = List<Int>.new()
"""
OK = ["k", "(k + 1)", "1", "gsel(1, 2)", "l.at(0)", "pr(@)", "pr(@)", "(2 * 3)"]
BAD = {"mul": ["(bg * 2)", "pm(3)", "(9223372036854775807 * k)"], "mod": ["(5 % zz)", "(k % zero())"],
       "div": ["(7 / zz)", "zdiv(4)"], "sub": ["(low() - 1)"], "neg": ["(-low())"], "add": ["(big() + k)"],
       "idx": ["l.at(9)"], "pop": ["e.pop()"]}
# (holes, template, oracle: every hole is always evaluated, in textual order)
CTX = [
    (3, "print(@0 + @1 + @2)", True),
    (3, "print(@0 * @1 - @2)", True),
    (3, "print(f3(@0, @1, @2))", True),
    (3, 'print("a {@0} b {@1} c {@2}")', True),
    (3, "let t = (@0, @1, @2)\n  print(t.0)", True),
    (3, "let p = P { b: @0, a: @1, c: @2 }\n  print(p.a)", True),
    (3, "var q = List<Int>.new()\n  q.push(@0)\n  q.push(@1)\n  q.push(@2)", True),
    (3, "for i in @0..@1 step @2 { print(i) }", True),
    (3, "for i in @0..=@1 { print(i) }\n  print(@2)", True),
    (3, "if @0 < @1 { print(1) }\n  print(@2)", True),
    (3, "while @0 > @1 { break }\n  print(@2)", True),
    (3, "let x = -(@0) + @1 * @2\n  print(x)", True),
    (3, "print(gsel(@0, gsel(@1, @2)))", True),
    (3, "var a = Acc { n: 0 }\n  a.add3(@0, @1, @2)\n  print(a.n)", True),
    (3, "print(wrap(@0, @1, @2))", True),
    (3, "print(gwrap(@0, @1, @2))", True),
    (3, "let x = wrap(@0, wrap(@1, 1, 1), @2)\n  print(x)", True),
    (3, "var s = 0\n  var i = 0\n  while i < 3 {\n    print(i)\n    s = s + @0\n    s = s + @1\n    s = s + @2\n    i = i + 1\n  }", True),
    (3, "var s = 0\n  for i in 0..3 {\n    print(i)\n    s = s + @0 + @1 + @2\n  }", True),
    (3, "var t = (1, 2)\n  t = (@0, @1)\n  print(@2)", True),
    (3, "l.set(@0 % 1, @1)\n  print(@2)", True),
    (3, "match (@0, @1, @2) {\n    (a, b, c) => { print(a + b + c) }\n  }", True),
    (3, "let x = @0\n  let y = @1\n  let z = @2\n  print(x + y + z)", True),
    (3, "print(@0 == @1 && @2 == 0)", False),
    (3, "print(@0 < @1 || @2 < 0)", False),
    (3, "match @0 {\n    5 => { print(@1) }\n    n if n > @1 => { print(@2) }\n    _ => { print(@2) }\n  }", False),
    (3, "match 3 {\n    n if n > @0 && n < @1 => { print(@2) }\n    _ => { print(0) }\n  }", False),
    (3, "if @0 > 1 { print(@1) } else { print(@2) }", False),
    (3, "var i = 0\n  while i < 2 && @0 > 0 {\n    print(@1)\n    i = i + 1\n  }\n  print(@2)", False),
]


def _sh_raw(args, env=None, timeout=60):
    try:
        p = subprocess.run(args, capture_output=True, timeout=timeout, env=env, stdin=subprocess.DEVNULL)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return -999, b"", b"TIMEOUT"


def sh(*args, **kwargs):
    """Run a command; `  at file:line:col` panic location lines are dropped from stderr: the native backend prints
    them in release builds, the C backend only with YERUX_LOCS/--debug (docs/PANIC_DIAGNOSTICS.md)."""
    import re as _re
    r = _sh_raw(*args, **kwargs)
    if isinstance(r, tuple) and len(r) == 3 and isinstance(r[2], bytes):
        return r[0], r[1], _re.sub(rb"(?m)^  at [^\n]*\n?", b"", r[2])
    return r


def program(ctx, holes):
    n, tpl, _ = ctx
    s = tpl
    for i in range(n - 1, -1, -1):
        s = s.replace(f"@{i}", holes[i])
    return PRELUDE + "fn main() {\n" + SETUP + "  " + s + "\n}\n"


def msg_of(err):
    for l in err.decode(errors="replace").splitlines():
        if l.startswith("panic"):
            return l
    return ""


class Fuzz:
    def __init__(self, tmp):
        self.tmp = tmp
        self.n = 0
        self.lock = threading.Lock()
        self.calib = {}

    def uid(self):
        with self.lock:
            self.n += 1
            return self.n

    def build_run(self, src, flags, env_extra=None):
        u = self.uid()
        exe = os.path.join(self.tmp, f"x{u}")
        env = dict(os.environ)
        env.update(env_extra or {})
        rc, _, err = sh([YERUX, "build", src, "-o", exe] + flags, env=env)
        if rc != 0:
            return (-1000, b"", b"BUILD FAILED " + err)
        r = sh([exe])
        os.unlink(exe)
        return r

    def run_all(self, text, a64=False):
        u = self.uid()
        src = os.path.join(self.tmp, f"p{u}.yx")
        open(src, "w").write(text)
        try:
            rc, _, err = sh([YERUX, "check", src])
            if rc != 0:
                return None, err
            res = {"c": self.build_run(src, []), "native": self.build_run(src, ["--native"]),
                   "noopt": self.build_run(src, ["--native", "--no-opt"])}
            if a64:
                import a64_emu
                a64tmp = os.path.join(self.tmp, f"p{u}_a64")
                os.makedirs(a64tmp)
                ec, so, se, err, _ = a64_emu.a64_run(src, a64tmp, [], check_decode=False)
                res["aarch64"] = (-1000, b"", (err or "aarch64 emulation failed").encode()) if err else (ec, so, se)
            return res, src
        finally:
            pass

    def bisect(self, src):
        out = []
        ref = self.build_run(src, [])
        for off in ("inline", "constprop", "copyprop", "sroa", "memopt", "rangeopt", "coalesce", "dce", "cfg"):
            r = self.build_run(src, ["--native"], {"YERUX_OPT_OFF": off})
            if r == ref:
                out.append(off)
        return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-n", type=int, default=500)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("-j", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--a64", action="store_true", help="also compare against aarch64 under Unicorn")
    a = ap.parse_args()
    seed0 = a.seed if a.seed is not None else random.randrange(1 << 30)
    tmp = tempfile.mkdtemp(prefix="yerux_ord_")
    fz = Fuzz(tmp)
    # calibrate: the message each panic kind produces in the C backend
    for kind, exprs in BAD.items():
        for ex in exprs:
            t = PRELUDE + "fn main() {\n" + SETUP + f"  print({ex})\n}}\n"
            res, src = fz.run_all(t)
            assert res, (ex, src)
            m = msg_of(res["c"][2])
            assert m, (ex, res["c"])
            fz.calib[ex] = m
    print(f"fuzz_order: seed={seed0} n={a.n}")
    bad = []

    def one(i):
        r = random.Random(seed0 + i)
        ctx = r.choice(CTX)
        nb = r.choice([0, 1, 1, 2, 2, 3])
        kinds = [True] * nb + [False] * (ctx[0] - nb)
        r.shuffle(kinds)
        holes = []
        for isbad in kinds:
            if isbad:
                holes.append(r.choice(r.choice(list(BAD.values()))))
            else:
                holes.append(r.choice(OK).replace("@", str(r.randint(1, 9))))
        text = program(ctx, holes)
        res, src = fz.run_all(text, a.a64)
        if res is None:
            print(f"generator bug seed={seed0 + i}: {src.decode()[:300]}")
            bad.append(i)
            return
        why = None
        c = res["c"]
        for name in (("native", "noopt", "aarch64") if a.a64 else ("native", "noopt")):
            if res[name] != c:
                why = f"{name} differs from C: C=({c[0]},{msg_of(c[2])}) {name}=({res[name][0]},{msg_of(res[name][2])})"
                break
        if why is None and ctx[2]:
            first = next((fz.calib[h] for h, b in zip(holes, kinds) if b), None)
            got = msg_of(c[2])
            if first is None and c[0] != 0 or first and got != first:
                why = f"oracle: expected {first!r}, C gave {got!r} (code {c[0]})"
        if why:
            keep = f"/tmp/order_{seed0 + i}.yx"
            open(keep, "w").write(text)
            culprits = fz.bisect(keep) if "native" in why or "noopt" in why else []
            print(f"MISMATCH seed={seed0 + i} {why}; saved {keep}; disabling {culprits} fixes native")
            bad.append(i)

    with ThreadPoolExecutor(a.j) as ex:
        list(ex.map(one, range(a.n)))
    print(f"done: {a.n} programs, {len(bad)} problems")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
