#!/usr/bin/env python3
"""Differential fuzz of the aarch64 backend (under Unicorn) against the x86-64 native backend.
Programs come from tests/fuzz.py generators (scalar `Gen`, or `AggGen` with --agg). Compares exit code,
stdout and stderr; with --agg also builds both with --native-debug-alloc and compares allocs/frees/live.
usage: python3 tests/a64_fuzz.py [--agg] [-n COUNT] [--seed SEED] [-j JOBS]"""
import argparse, os, random, sys, tempfile
from concurrent.futures import ProcessPoolExecutor
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fuzz, a64_emu

def one(args):
    seed, agg, num = args
    lines = (fuzz.NumGen if num else fuzz.AggGen if agg else fuzz.Gen)(seed, True).program()
    with tempfile.TemporaryDirectory() as tmp:
        src = os.path.join(tmp, "p.yx")
        open(src, "w").write("\n".join(lines) + "\n")
        for extra in ([], ["--native-debug-alloc"] if agg else None):
            if extra is None:
                break
            x = a64_emu.run_x64(src, tmp, extra)
            if x is None:
                return seed, "skip", ""
            ec, so, se, err, rep = a64_emu.a64_run(src, tmp, extra, check_decode=False)
            if err:
                return seed, "fail", err
            if (ec, so, se) != x:
                keep = f"/tmp/a64_fuzz_{seed}.yx"
                open(keep, "w").write("\n".join(lines) + "\n")
                return seed, "fail", f"mismatch {extra}: a64 code={ec} x64 code={x[0]}; saved {keep}"
    return seed, "ok", ""

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--agg", action="store_true")
    ap.add_argument("--num", action="store_true", help="Float / sized-integer programs (tests/fuzz_num.py)")
    ap.add_argument("-n", type=int, default=200)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("-j", type=int, default=os.cpu_count() or 4)
    a = ap.parse_args()
    s0 = a.seed if a.seed is not None else random.randrange(1 << 30)
    print(f"a64 fuzz: seed={s0} n={a.n} agg={a.agg} num={a.num}")
    with ProcessPoolExecutor(a.j) as ex:
        res = list(ex.map(one, [(s0 + i, a.agg, a.num) for i in range(a.n)], chunksize=4))
    cnt = {}
    for seed, st, msg in res:
        cnt[st] = cnt.get(st, 0) + 1
        if st == "fail":
            print(f"FAIL seed={seed}: {msg}")
    print(cnt)
    sys.exit(1 if cnt.get("fail") else 0)

if __name__ == "__main__":
    main()
