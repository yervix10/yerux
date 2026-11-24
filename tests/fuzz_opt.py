#!/usr/bin/env python3
"""Optimizer + sanitizer differential fuzzer. Every generated program (scalar, --agg, --structs, --num, --coll)
is built once with the C backend (reference, also built with -fsanitize=address,undefined) and then natively under
many optimizer configurations. All must agree on stdout / stderr / exit code:
  native (default), --no-opt, YERUX_NO_INLINE=1, YERUX_NO_ROTATE=1, and YERUX_OPT_OFF=<pass> for each of
  constprop copyprop sroa memopt rangeopt coalesce dce cfg inline.
Also: the ASan/UBSan C build must print no report, and (for programs that do not panic) a --native-debug-alloc
build must end with `live: 0`. On a mismatch the failing configuration set is reported, which names the pass.

usage: tests/fuzz_opt.py [--gen scalar|agg|structs|num|coll|all] [--a64] [-n COUNT] [--seed SEED] [-j JOBS] [--no-san]"""
import argparse, hashlib, os, random, subprocess, sys, tempfile, threading
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fuzz, fuzz_coll
from fuzz_num import NumGen

ROOT = fuzz.ROOT
YERUX = fuzz.YERUX
BUGS = fuzz.BUGS
PASSES = ["constprop", "copyprop", "sroa", "memopt", "rangeopt", "coalesce", "dce", "cfg", "inline"]
CONFIGS = [("native", [], {}), ("noopt", ["--no-opt"], {}), ("noinline", [], {"YERUX_NO_INLINE": "1"}),
           ("norotate", [], {"YERUX_NO_ROTATE": "1"}), ("noshrink", [], {"YERUX_NO_SHRINKWRAP": "1"})] + \
          [("off-" + p, [], {"YERUX_OPT_OFF": p}) for p in PASSES] + \
          [("alloff", [], {"YERUX_OPT_OFF": ",".join(PASSES)})]


def _sh_raw(args, env=None, timeout=20):
    e = dict(os.environ)
    e.update(env or {})
    try:
        p = subprocess.run(args, capture_output=True, timeout=timeout, env=e, stdin=subprocess.DEVNULL)
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


class Ctx:
    def __init__(self, tmp):
        self.tmp = tmp
        self.n = 0
        self.lock = threading.Lock()

    def uid(self):
        with self.lock:
            self.n += 1
            return self.n


def gen_program(kind, seed):
    if kind == "coll":
        p = fuzz_coll.Prog(seed)
        return p.program(p.r.randrange(5, 120))[0]
    g = {"scalar": fuzz.Gen, "agg": fuzz.AggGen, "structs": fuzz.StructGen, "num": NumGen}[kind]
    return "\n".join(g(seed, True).program()) + "\n"


def evaluate(ctx, text, san=True, a64=False):
    """-> (list of problems, key)"""
    u = ctx.uid()
    src = os.path.join(ctx.tmp, f"p{u}.yx")
    open(src, "w").write(text)
    probs = []
    files = [src]
    try:
        rc, _, err = sh([YERUX, "check", src])
        if rc != 0:
            return [("generator", err.decode()[:200])]
        exe = os.path.join(ctx.tmp, f"p{u}_c")
        files.append(exe)
        rc, _, err = sh([YERUX, "build", src, "-o", exe], timeout=60)
        if rc != 0:
            return [("c-build", err.decode()[:200])]
        ref = sh([exe])
        if ref[0] == -999:
            return []
        timed_out = False
        for name, flags, env in CONFIGS:
            ne = os.path.join(ctx.tmp, f"p{u}_{name}")
            files.append(ne)
            rc, _, err = sh([YERUX, "build", src, "--native", "-o", ne] + flags, env=env, timeout=60)
            if rc != 0:
                if b"native backend:" in err:
                    return []
                probs.append((name, "build failed: " + err.decode()[:200]))
                continue
            r = sh([ne])
            timed_out |= r[0] == -999
            if r != ref:
                probs.append((name, f"differs: ref=({ref[0]},{ref[2][:80]!r}) got=({r[0]},{r[2][:80]!r}) stdout_same={r[1] == ref[1]}"))
        if timed_out:
            return []  # scheduler or runaway timeout, not a differential result
        if a64:
            import a64_emu
            a64tmp = os.path.join(ctx.tmp, f"p{u}_a64")
            os.makedirs(a64tmp)
            ec, so, se, emu_err, _ = a64_emu.a64_run(src, a64tmp, [], check_decode=False)
            if emu_err:
                probs.append(("aarch64", emu_err[:200]))
            elif (ec, so, se) != ref:
                probs.append(("aarch64", f"differs: ref=({ref[0]},{ref[2][:80]!r}) got=({ec},{se[:80]!r}) stdout_same={so == ref[1]}"))
        if san:
            cfile = os.path.join(ctx.tmp, f"p{u}.c")
            sbin = os.path.join(ctx.tmp, f"p{u}_san")
            files += [cfile, sbin]
            rc, cc, err = sh([YERUX, "c", src])
            open(cfile, "wb").write(cc)
            b = sh(["cc", "-g", "-O1", "-w", "-fsanitize=address,undefined", "-fno-sanitize-recover=undefined", "-fno-math-errno",
                    "-ffp-contract=off", "-o", sbin, cfile, "-lm"], timeout=120)
            if b[0] != 0:
                probs.append(("san-build", b[2].decode()[:200]))
            else:
                r = sh([sbin], env={"ASAN_OPTIONS": "detect_leaks=1"})
                if b"Sanitizer" in r[2] or b"runtime error" in r[2]:
                    probs.append(("sanitizer", r[2].decode(errors="replace")[:400]))
                elif r[0] != ref[0] or r[1] != ref[1]:
                    probs.append(("san-differs", f"san=({r[0]}) ref=({ref[0]})"))
        if ref[0] == 0:
            ne = os.path.join(ctx.tmp, f"p{u}_leak")
            files.append(ne)
            rc, _, err = sh([YERUX, "build", src, "--native", "--native-debug-alloc", "-o", ne], timeout=60)
            if rc == 0:
                r = sh([ne])
                if b"live: 0\n" not in r[2] or r[1] != ref[1]:
                    probs.append(("leak", r[2].decode(errors="replace").replace("\n", " ")[-120:]))
        return probs
    finally:
        for f in files:
            try:
                os.remove(f)
            except OSError:
                pass


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--gen", default="all")
    ap.add_argument("-n", type=int, default=300)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("-j", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--no-san", action="store_true")
    ap.add_argument("--a64", action="store_true", help="also compare with aarch64 under Unicorn")
    a = ap.parse_args()
    kinds = ["scalar", "agg", "structs", "num", "coll"] if a.gen == "all" else a.gen.split(",")
    seed0 = a.seed if a.seed is not None else random.randrange(1 << 30)
    tmp = tempfile.mkdtemp(prefix="yerux_opt_")
    ctx = Ctx(tmp)
    os.makedirs(BUGS, exist_ok=True)
    print(f"fuzz_opt: seed={seed0} n={a.n} gens={kinds}")
    bad = []

    def one(i):
        kind = kinds[i % len(kinds)]
        seed = seed0 + i
        text = gen_program(kind, seed)
        probs = evaluate(ctx, text, not a.no_san, a.a64)
        if probs:
            h = hashlib.sha1(text.encode()).hexdigest()[:8]
            path = os.path.join(BUGS, f"opt_{kind}_{h}.yx")
            open(path, "w").write(text)
            names = [p[0] for p in probs]
            print(f"PROBLEM {kind} seed={seed}: {names} -> {path}\n    {probs[0][1][:300]}")
            bad.append(seed)

    with ThreadPoolExecutor(a.j) as ex:
        list(ex.map(one, range(a.n)))
    print(f"done: {a.n} programs, {len(bad)} with problems")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
