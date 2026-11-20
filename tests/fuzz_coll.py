#!/usr/bin/env python3
"""Collections differential fuzzer (tests/fuzz.py --coll).

Random programs over Map<Int,Int>, Map<String,Int>, Set<Int>, List<Int>, List<String> and String
operations. Each program prints the result of every query; the expected output comes from a Python
model, and the program is built with the C backend and the native backend. All three must agree
exactly (stdout and exit code 0). Failing programs are saved to tests/bugs/coll_<hash>.yx.
"""
import hashlib, os, random, subprocess, sys, tempfile
from concurrent.futures import ThreadPoolExecutor

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
YERUX = os.path.join(ROOT, "target", "release", "yerux")
BUGS = os.path.join(ROOT, "tests", "bugs")
MINI = -(1 << 63)
MAXI = (1 << 63) - 1


def lit(n):
    return f"(-{-n})" if n < 0 and n != MINI else ("(-9223372036854775807 - 1)" if n == MINI else str(n))


def fmt_list(v):
    return "[" + ", ".join(str(x) for x in v) + "]"


class Prog:
    def __init__(self, seed):
        self.r = random.Random(seed)
        self.code = []
        self.out = []
        self.n = 0
        # models
        self.m = {}          # Map<Int,Int>: dict (order modelled only through sorted queries)
        self.sm = {}         # Map<String,Int>
        self.s = set()       # Set<Int>
        self.l = []          # List<Int>
        self.ls = []         # List<String>
        self.keyrange = self.r.choice([5, 20, 200, 3000])

    def emit(self, line):
        self.code.append("  " + line)

    def key(self):
        r = self.r.random()
        if r < 0.85:
            return self.r.randrange(-self.keyrange // 4, self.keyrange)
        return self.r.choice([MINI, MAXI, MINI + 1, 2147483647, 2147483646, -2147483647, 2147483648, 1 << 40, -(1 << 40)])

    def word(self):
        return self.r.choice(["", "a", "b", "key", "ключ", "ñandú", "x y", "k%d" % self.r.randrange(self.keyrange)])

    def step(self):
        r = self.r
        op = r.choice(["mi", "mi", "mi", "mr", "mg", "mc", "mt", "si", "si", "sr", "sc", "li", "lr", "lx", "ls", "lw",
                       "lrev", "lt", "lcont", "smi", "smr", "smg", "str", "clear"])
        if op == "mi":
            k, v = self.key(), r.randrange(-1000, 1000)
            self.emit(f"m.insert({lit(k)}, {lit(v)})")
            self.m[k] = v
        elif op == "mr":
            k = self.key() if r.random() < 0.5 or not self.m else r.choice(list(self.m))
            self.emit(f"print(m.remove({lit(k)}))")
            self.out.append("true" if k in self.m else "false")
            self.m.pop(k, None)
        elif op == "mg":
            k = self.key() if r.random() < 0.4 or not self.m else r.choice(list(self.m))
            self.emit(f"print(m.get_or({lit(k)}, -77))")
            self.out.append(str(self.m.get(k, -77)))
        elif op == "mc":
            k = self.key()
            self.emit(f"print(m.contains({lit(k)}))")
            self.out.append("true" if k in self.m else "false")
        elif op == "mt":
            if self.m:
                k = r.choice(list(self.m))
                self.emit(f"print(m.take({lit(k)}))")
                self.out.append(str(self.m.pop(k)))
        elif op == "si":
            k = self.key()
            self.emit(f"print(s.insert({lit(k)}))")
            self.out.append("false" if k in self.s else "true")
            self.s.add(k)
        elif op == "sr":
            k = self.key() if r.random() < 0.5 or not self.s else r.choice(list(self.s))
            self.emit(f"print(s.remove({lit(k)}))")
            self.out.append("true" if k in self.s else "false")
            self.s.discard(k)
        elif op == "sc":
            k = self.key()
            self.emit(f"print(s.contains({lit(k)}))")
            self.out.append("true" if k in self.s else "false")
        elif op == "li":
            v = r.randrange(-50, 50)
            i = r.randrange(0, len(self.l) + 1)
            self.emit(f"l.insert({i}, {lit(v)})")
            self.l.insert(i, v)
        elif op == "lr":
            if self.l:
                i = r.randrange(len(self.l))
                self.emit(f"print(l.remove({i}))")
                self.out.append(str(self.l.pop(i)))
        elif op == "lx":
            for _ in range(r.randrange(1, 5)):
                v = r.randrange(-50, 50)
                self.emit(f"l.push({lit(v)})")
                self.l.append(v)
        elif op == "ls":
            self.emit("l.sort()")
            self.l.sort()
        elif op == "lw":
            if len(self.l) >= 2:
                i, j = r.randrange(len(self.l)), r.randrange(len(self.l))
                self.emit(f"l.swap({i}, {j})")
                self.l[i], self.l[j] = self.l[j], self.l[i]
        elif op == "lrev":
            self.emit("l.reverse()")
            self.l.reverse()
        elif op == "lt":
            n = r.randrange(0, len(self.l) + 1)
            self.emit(f"l.truncate({n})")
            del self.l[n:]
        elif op == "lcont":
            v = r.randrange(-50, 50)
            self.emit(f"print(l.index_of({lit(v)}))")
            self.out.append(str(self.l.index(v) if v in self.l else -1))
        elif op == "smi":
            w = self.word()
            v = r.randrange(100)
            self.emit(f'sm.insert(String.from("{w}"), {v})')
            self.sm[w] = v
        elif op == "smr":
            w = self.word()
            self.emit(f'let q{self.n} = String.from("{w}")')
            self.emit(f"print(sm.remove(q{self.n}))")
            self.n += 1
            self.out.append("true" if w in self.sm else "false")
            self.sm.pop(w, None)
        elif op == "smg":
            w = self.word()
            self.emit(f'let q{self.n} = String.from("{w}")')
            self.emit(f"print(sm.get_or(q{self.n}, -1))")
            self.n += 1
            self.out.append(str(self.sm.get(w, -1)))
        elif op == "str":
            # string API on a random word
            w = self.word() + r.choice(["", " ", "  ", ",", "a,b,,c"])
            sep = r.choice([",", "a", " "])
            self.emit(f'let t{self.n} = String.from("{w}")')
            self.emit(f"let u{self.n} = t{self.n}.trim()")
            self.emit(f'print("[{{u{self.n}}}] {{t{self.n}.len()}} {{t{self.n}.find(\\"{sep}\\")}}")' if False else f"print(u{self.n})")
            self.emit(f'let p{self.n} = t{self.n}.split("{sep}")')
            self.emit(f'print(p{self.n}.join("|"))')
            self.emit(f"print(p{self.n}.len())")
            self.emit(f'print(t{self.n}.replace("{sep}", "--"))' if False else f'let rp{self.n} = t{self.n}.replace("{sep}", "--")')
            self.emit(f"print(rp{self.n})")
            self.emit(f'print(t{self.n}.contains("{sep}"))')
            self.n += 1
            self.out.append(w.strip(" \t\r\n"))
            parts = w.split(sep)
            self.out.append("|".join(parts))
            self.out.append(str(len(parts)))
            self.out.append(w.replace(sep, "--"))
            self.out.append("true" if sep in w else "false")
        elif op == "clear":
            if r.random() < 0.15:
                self.emit("m.clear()")
                self.m.clear()

    def program(self, steps):
        self.code = ["fn main() {",
                     "  var m = Map<Int, Int>.new()",
                     "  var sm = Map<String, Int>.new()",
                     "  var s = Set<Int>.new()",
                     "  var l = List<Int>.new()"]
        for _ in range(steps):
            self.step()
        # final state, in a canonical order
        self.emit("print(m.len())")
        self.out.append(str(len(self.m)))
        self.emit("print(sm.len())")
        self.out.append(str(len(self.sm)))
        self.emit("print(s.len())")
        self.out.append(str(len(self.s)))
        # every key in insertion-agnostic order: sort the entries through a list
        self.emit("var ks = List<Int>.new()")
        self.emit("var i = 0")
        self.emit("while i < m.keys.len() {")
        self.emit("  ks.push(m.keys.at(i))")
        self.emit("  i = i + 1")
        self.emit("}")
        self.emit("ks.sort()")
        self.emit("i = 0")
        self.emit("while i < ks.len() {")
        self.emit('  print("{ks.at(i)} -> {m.get(ks.at(i))}")')
        self.emit("  i = i + 1")
        self.emit("}")
        for k in sorted(self.m):
            self.out.append(f"{k} -> {self.m[k]}")
        self.emit("var ss = List<Int>.new()")
        self.emit("i = 0")
        self.emit("while i < s.items.keys.len() {")
        self.emit("  ss.push(s.items.keys.at(i))")
        self.emit("  i = i + 1")
        self.emit("}")
        self.emit("ss.sort()")
        self.emit('print("{ss}")')
        self.out.append(fmt_list(sorted(self.s)))
        self.emit('print("{l}")')
        self.out.append(fmt_list(self.l))
        self.code.append("}")
        return "\n".join(self.code) + "\n", "\n".join(self.out) + ("\n" if self.out else "")


def run_one(seed, tmp, a64=False):
    p = Prog(seed)
    src, want = p.program(p.r.randrange(5, 120))
    f = os.path.join(tmp, f"p{seed}.yx")
    with open(f, "w") as fh:
        fh.write(src)
    res = {}
    for name, flags in (("c", []), ("native", ["--native"])):
        exe = os.path.join(tmp, f"p{seed}_{name}")
        b = subprocess.run([YERUX, "build", f, "-o", exe] + flags, capture_output=True)
        if b.returncode != 0:
            res[name] = ("BUILD", b.stderr.decode(errors="replace")[:300])
            continue
        try:
            r = subprocess.run([exe], capture_output=True, timeout=20)
            res[name] = (r.returncode, r.stdout.decode(errors="replace"))
        except subprocess.TimeoutExpired:
            res[name] = ("TIMEOUT", "")
    if a64:
        import a64_emu
        a64tmp = os.path.join(tmp, f"a64_{seed}")
        os.makedirs(a64tmp)
        ec, so, _, err, _ = a64_emu.a64_run(f, a64tmp, [], check_decode=False)
        res["aarch64"] = ("BUILD", err[:300]) if err else (ec, so.decode(errors="replace"))
    bad = [n for n, v in res.items() if v != (0, want)]
    for ext in ("", "_c", "_native"):
        try:
            os.remove(os.path.join(tmp, f"p{seed}{ext}" + (".yx" if ext == "" else "")))
        except OSError:
            pass
    return seed, src, want, res, bad


def run(n, seed0, jobs, a64=False):
    seed0 = seed0 if seed0 is not None else random.randrange(1 << 30)
    print(f"fuzz --coll: seed={seed0} n={n}")
    os.makedirs(BUGS, exist_ok=True)
    tmp = tempfile.mkdtemp(prefix="yerux_coll_")
    bad_count = 0
    with ThreadPoolExecutor(jobs or 4) as ex:
        for seed, src, want, res, bad in ex.map(lambda i: run_one(seed0 + i, tmp, a64), range(n)):
            if bad:
                bad_count += 1
                h = hashlib.sha1(src.encode()).hexdigest()[:8]
                with open(os.path.join(BUGS, f"coll_{h}.yx"), "w") as f:
                    f.write(src)
                with open(os.path.join(BUGS, f"coll_{h}.txt"), "w") as f:
                    f.write(f"seed {seed}, wrong: {bad}\n--- expected\n{want}\n")
                    for k, v in res.items():
                        f.write(f"--- {k}\n{v}\n")
                print(f"MISMATCH seed={seed} backends={bad} saved tests/bugs/coll_{h}.yx")
    print(f"done: {n} programs, {bad_count} mismatches")
    return 1 if bad_count else 0


if __name__ == "__main__":
    import argparse
    ap = argparse.ArgumentParser()
    ap.add_argument("-n", type=int, default=300)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("-j", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--a64", action="store_true", help="also compare against aarch64 under Unicorn")
    a = ap.parse_args()
    sys.exit(run(a.n, a.seed, a.j, a.a64))
