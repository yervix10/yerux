#!/usr/bin/env python3
"""Soundness fuzzer for the ownership checker: look for programs that `yerux check` ACCEPTS but that use freed memory.

Every program opens a borrow (a `match` arm binding an owned payload, `for x in list`, `for mut x in list`, a closure
passed as `view fn`, `view`/`mut` call arguments) on some place and then, while the borrow is alive, does something
that frees / moves / reallocates / overwrites that place (assignment, `move`, `clear`, `pop`, `truncate`, `set`,
`mut` calls, ...) followed by a use of the borrowed name. Accepted programs are compiled with the C backend under
AddressSanitizer (heap-use-after-free, double free) and with `--native-debug-alloc` (double free / live != 0).
A program that is accepted and trips a sanitizer is a hole in the checker; programs rejected by `check` are fine.

usage: tests/fuzz_unsound.py [-n COUNT] [--seed SEED] [--show-accepted]
"""
import argparse, os, random, subprocess, sys, tempfile, shutil

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
YERUX = os.path.join(ROOT, "target", "release", "yerux")
BUGS = os.path.join(ROOT, "tests", "bugs")

PRE = """enum Msg { Text(String), Nums(List<Int>), Pair(String, Int), Wrap(Box<Msg>), Quit }
struct Holder { m: Msg, xs: List<String>, name: String, b: Box<Msg> }
fn msize(m: view Msg) -> Int {
  match m {
    Text(s) => { return s.len() }
    Nums(l) => { return l.len() }
    Pair(s, n) => { return s.len() + n }
    Wrap(b) => { return msize(*b) + 1 }
    Quit => { return 0 }
  }
}
fn mgrow(m: mut Msg) {
  match mut m {
    Text(s) => { s.push_str("0123456789abcdefghijklmnopqrstuvwxyz0123456789") }
    Nums(l) => { l.push(1) }
    Pair(s, n) => { s.push_str("0123456789abcdefghijklmnopqrstuvwxyz0123456789") }
    Wrap(b) => { mgrow(*b) }
    Quit => { }
  }
}
fn takes(m: Msg) -> Int { return msize(m) }
fn takes_s(s: String) -> Int { return s.len() }
fn takes_l(l: List<String>) -> Int { return l.len() }
fn grow_s(s: mut String) { s.push_str("0123456789abcdefghijklmnopqrstuvwxyz0123456789") }
fn grow_l(l: mut List<String>) { l.push(String.from("zz")) }
fn mk() -> Msg { return Msg.Text(String.from("hello world, this is long enough")) }
fn mkh() -> Holder {
  var xs = List<String>.new()
  xs.push(String.from("one one one one one one"))
  xs.push(String.from("two two two two two two"))
  return Holder { m: mk(), xs: move xs, name: String.from("name name name name"), b: Box.new(mk()) }
}
fn each(l: view List<String>, f: view fn(view String)) { for i in 0..l.len() { f(l.at(i)) } }
fn call(f: view fn() -> Int, a: Int) -> Int { return f() + a }
"""

# scrutinee place -> (declaration lines, place expr, type)
def scrutinees(r):
    return [
        ("var m = mk()", "m", "Msg", "m"),
        ("var h = mkh()", "h.m", "Msg", "h"),
        ("var h = mkh()", "h.b.get()", "Msg", "h"),
        ("var xs = List<Msg>.new()\n  xs.push(mk())\n  xs.push(mk())", "xs.at(1)", "Msg", "xs"),
        ("var o = Some(mk())", "o", "Option<Msg>", "o"),
    ]


def conflicts(root, place, ty):
    """Statements that free / move / realloc / overwrite `place` or its root."""
    out = []
    if ty == "Msg":
        out += [f"{place} = Msg.Quit", f"{place} = mk()", f"mgrow({place})", f"print(takes(move {place}))" if place.isidentifier() else f"{place} = Msg.Nums(List<Int>.new())"]
    else:
        out += [f"{place} = None", f"{place} = Some(mk())"]
    if root == "xs":
        out += ["xs.clear()", "xs.pop()", "xs.truncate(0)", "xs.set(1, Msg.Quit)", "xs.push(mk())", "xs.insert(0, mk())", "xs.remove(0)", "xs.swap_remove(0)", "xs.reverse()", "xs.swap(0, 1)", "xs.extend(List<Msg>.new())"]
    if root == "h":
        out += ["h = mkh()", "h.xs.clear()", "h.name.push_str(\"0123456789abcdefghijklmnopqrstuvwxyz\")", "h.b = Box.new(Msg.Quit)", "h.m = Msg.Quit"]
    return out


def match_programs(r):
    decl, place, ty, root = r.choice(scrutinees(r))
    kind = r.choice(["match", "match mut"])
    conf = r.choice(conflicts(root, place, ty))
    if ty == "Msg":
        arms = ["Text(s) => {", "Nums(l) => {", "Pair(s, n) => {"]
        arm = r.choice(arms)
        use = {"Text": "print(s)", "Nums": "print(l.len())", "Pair": "print(s)"}[arm.split("(")[0]]
        body = f"  {kind} {place} {{\n    {arm}\n      {conf}\n      {use}\n    }}\n    _ => {{ }}\n  }}"
    else:
        body = f"  {kind} {place} {{\n    Some(x) => {{\n      {conf}\n      print(msize(x))\n    }}\n    None => {{ }}\n  }}"
    return f"{decl}\n{body}"


def loop_programs(r):
    kind = r.choice(["for x in xs", "for mut x in xs", "for i in 0..xs.len()"])
    conf = r.choice(["xs.clear()", "xs.pop()", "xs.truncate(0)", "xs.push(String.from(\"q\"))", "xs.set(0, String.from(\"q\"))", "grow_l(xs)", "xs = List<String>.new()",
                     "print(takes_l(move xs))", "xs.at(0).push_str(\"0123456789abcdefghijklmnopqrstuvwxyz\")", "xs.insert(0, String.from(\"q\"))", "xs.reverse()", "xs.remove(0)"])
    use = {"for x in xs": "print(x)", "for mut x in xs": "print(x)", "for i in 0..xs.len()": "print(xs.at(0))"}[kind]
    return f"var xs = List<String>.new()\n  xs.push(String.from(\"one one one one one one one\"))\n  xs.push(String.from(\"two two two two two two two\"))\n  {kind} {{\n    {conf}\n    {use}\n  }}"


def closure_programs(r):
    conf = r.choice(["xs.clear()", "xs.push(String.from(\"q\"))", "grow_l(xs)", "xs = List<String>.new()", "print(takes_l(move xs))", "xs.pop()"])
    k = r.random()
    if k < 0.4:
        return f"var xs = List<String>.new()\n  xs.push(String.from(\"one one one one one one one\"))\n  each(xs, |s: view String| {{\n    {conf}\n    print(s)\n  }})"
    if k < 0.7:
        return f"var xs = List<String>.new()\n  xs.push(String.from(\"one one one one one\"))\n  print(call(|| xs.len(), takes_l(move xs)))"
    return f"var s = String.from(\"one one one one one one\")\n  var xs = List<String>.new()\n  xs.push(String.from(\"q\"))\n  each(xs, |t: view String| {{\n    grow_s(s)\n    print(s)\n    print(t)\n  }})\n  print(call(|| s.len(), takes_s(move s)))"


def call_programs(r):
    k = r.random()
    if k < 0.3:
        return "var xs = List<String>.new()\n  xs.push(String.from(\"one one one\"))\n  grow2(xs.at(0), xs)".replace("grow2", "two")
    if k < 0.6:
        return "var h = mkh()\n  two2(h.name, h)"
    return "var m = mk()\n  two3(m, takes(move m))"


EXTRA = """fn two(a: view String, b: mut List<String>) { b.clear()
  print(a) }
fn two2(a: view String, b: mut Holder) { b.name = String.from("x")
  print(a) }
fn two3(a: view Msg, n: Int) { print(msize(a)) }
"""


SOUP = False


def program(seed):
    if SOUP:
        return Soup(seed).program()
    r = random.Random(seed)
    g = r.choice([match_programs, match_programs, loop_programs, closure_programs, call_programs])
    return PRE + EXTRA + "fn main() {\n  " + g(r) + "\n}\n"


# ---------------------------------------------------------------- ownership soup
SOUP_PRE = PRE + """fn takes_i(l: List<Int>) -> Int { return l.len() }
fn mkmsg(k: Int) -> Msg {
  if k % 3 == 0 { return Msg.Text(String.from("text payload long enough for the heap")) }
  if k % 3 == 1 { return Msg.Pair(String.from("pair payload long enough for the heap"), k) }
  return Msg.Wrap(Box.new(Msg.Text(String.from("boxed payload long enough"))))
}
"""


class Soup:
    """Random statements over owned variables with NO regard for ownership rules: the checker decides what is legal."""

    def __init__(self, seed):
        self.r = random.Random(seed)
        self.lines = []
        self.n = 0
        self.ints = ["i0", "i1"]
        self.vars = {"S": ["s0", "s1", "s2"], "L": ["l0", "l1"], "M": ["m0", "m1"], "F": ["f0"], "H": ["h0"], "O": ["o0"], "B": ["b0"]}
        self.depth = 0

    def e(self, ind, t):
        self.lines.append("  " * ind + t)

    def v(self, k):
        return self.r.choice(self.vars[k])

    def fresh(self, p):
        self.n += 1
        return f"{p}{self.n}"

    def str_e(self):
        r = self.r
        return r.choice([f'String.from("{r.choice(["abc long string value here", "x", "another long string value"])}")', f"String.from({self.v('S')})", f"mkstr({self.r.randint(0, 3)})"])

    def stmt(self, ind):
        r = self.r
        k = r.random()
        S, L, M = self.v("S"), self.v("L"), self.v("M")
        F = self.v("F")
        opts = [
            lambda: self.e(ind, f"print({S})"),
            lambda: self.e(ind, f"print({L}.len())"),
            lambda: self.e(ind, f"print(msize({M}))"),
            lambda: self.e(ind, f"{S} = {self.str_e()}"),
            lambda: self.e(ind, f"{L} = List<String>.new()"),
            lambda: self.e(ind, f"{M} = mkmsg({r.randint(0, 5)})"),
            lambda: self.e(ind, f"{L}.push(move {S})"),
            lambda: self.e(ind, f"{L}.push({self.str_e()})"),
            lambda: self.e(ind, f"print(takes_s(move {S}))"),
            lambda: self.e(ind, f"print(takes_l(move {L}))"),
            lambda: self.e(ind, f"print(takes(move {M}))"),
            lambda: self.e(ind, f"let {self.fresh('t')} = move {S}"),
            lambda: self.e(ind, f"var {self.fresh('t')} = move {L}"),
            lambda: self.e(ind, f"{S}.push_str({self.v('S')})"),
            lambda: self.e(ind, f"{S}.push_str(\"0123456789abcdefghijklmnopqrstuvwxyz\")"),
            lambda: self.e(ind, f"grow_s({S})"),
            lambda: self.e(ind, f"grow_l({L})"),
            lambda: self.e(ind, f"mgrow({M})"),
            lambda: self.e(ind, f"if {L}.len() > 0 {{ print({L}.at(0)) }}"),
            lambda: self.e(ind, f"if {L}.len() > 0 {{ {L}.at(0).push_str(\"zzzzzzzzzzzzzzzzzzzz\") }}"),
            lambda: self.e(ind, f"if {L}.len() > 0 {{ let p = {L}.pop()\n{'  ' * ind}  print(p) }}"),
            lambda: self.e(ind, f"{L}.clear()"),
            lambda: self.e(ind, f"{L}.truncate(0)"),
            lambda: self.e(ind, f"two({self.v('S')}, {self.v('L')})") ,
            lambda: self.e(ind, f"{F} = move || {S}.len()"),
            lambda: self.e(ind, f"{F} = move || {self.v('L')}.len() + {self.v('S')}.len()"),
            lambda: self.e(ind, f"print({F}())"),
            lambda: self.e(ind, f"print(call({F}, 1))"),
            lambda: self.e(ind, f"print(call(|| {S}.len(), 1))"),
            lambda: self.e(ind, f"print(call(|| {L}.len() + {self.v('S')}.len(), takes_s(move {self.v('S')})))"),
            lambda: self.e(ind, f"defer print({S})"),
            lambda: self.e(ind, f"defer print({L}.len())"),
            lambda: self.e(ind, f"{F} = move || 3"),
            lambda: self.e(ind, f"let {self.fresh('o')} = Some(move {S})"),
            lambda: self.e(ind, f"let {self.fresh('o')} = Some(move {M})"),
            lambda: self.e(ind, f"{M} = Msg.Wrap(Box.new(move {self.v('M')}))"),
            lambda: self.e(ind, f"print(take_opt(Some(move {S})))"),
            lambda: self.e(ind, f"{L}.set(0, move {S})"),
            lambda: self.e(ind, f"{L}.insert(0, move {S})"),
            lambda: self.e(ind, f"{L}.extend(move {self.v('L')})"),
            lambda: self.e(ind, f"{L}.swap(0, 1)"),
        ]
        H, O, B = self.v("H"), self.v("O"), self.v("B")
        opts += [
            lambda: self.e(ind, f"{H} = mkh()"),
            lambda: self.e(ind, f"print({H}.name)"),
            lambda: self.e(ind, f"grow_s({H}.name)"),
            lambda: self.e(ind, f"grow_l({H}.xs)"),
            lambda: self.e(ind, f"{H}.xs.push(move {S})"),
            lambda: self.e(ind, f"{H}.m = {M}" if False else f"{H}.m = mkmsg({r.randint(0, 5)})"),
            lambda: self.e(ind, f"{H}.name = {self.str_e()}"),
            lambda: self.e(ind, f"{H}.b = Box.new(move {M})"),
            lambda: self.e(ind, f"print(takes_h(move {H}))"),
            lambda: self.e(ind, f"two({H}.name, {H}.xs)"),
            lambda: self.e(ind, f"two({H}.name, {L})"),
            lambda: self.e(ind, f"if {H}.xs.len() > 0 {{ {H}.xs.at(0).push_str({H}.name) }}"),
            lambda: self.e(ind, f"{O} = Some(move {M})"),
            lambda: self.e(ind, f"{O} = None"),
            lambda: self.e(ind, f"print(opt_size({O}))"),
            lambda: self.e(ind, f"{B} = Box.new(move {M})"),
            lambda: self.e(ind, f"mgrow({B}.get())"),
            lambda: self.e(ind, f"print(msize({B}.get()))"),
            lambda: self.e(ind, f"print(takes({B}.take()))"),
            lambda: self.e(ind, f"{M} = {B}.take()"),
        ]
        compound = self.depth < 3 and k < 0.35
        if not compound:
            r.choice(opts)()
            return
        c = r.random()
        self.depth += 1
        if c < 0.25:
            self.e(ind, f"if i0 % 2 == {r.randint(0, 1)} {{")
            self.block(ind + 1)
            if r.random() < 0.5:
                self.e(ind, "} else {")
                self.block(ind + 1)
            self.e(ind, "}")
        elif c < 0.4:
            w = self.fresh("w")
            self.e(ind, f"var {w} = 0")
            self.e(ind, f"while {w} < {r.randint(1, 3)} {{")
            self.e(ind + 1, f"{w} = {w} + 1")
            self.block(ind + 1)
            if r.random() < 0.3:
                self.e(ind + 1, r.choice(["if i0 == 1 { break }", "if i0 == 0 { continue }"]))
            self.e(ind, "}")
        elif c < 0.55:
            self.e(ind, f"for {self.fresh('x')} in {L} {{")
            self.block(ind + 1)
            self.e(ind, "}")
        elif c < 0.7:
            self.e(ind, f"match {r.choice(['', 'mut ', 'move '])}{M} {{")
            self.e(ind + 1, "Text(s) => {")
            self.block(ind + 2)
            if r.random() < 0.5:
                self.e(ind + 2, "print(s)")
            self.e(ind + 1, "}")
            self.e(ind + 1, "Pair(s, n) => {")
            self.block(ind + 2)
            self.e(ind + 1, "}")
            self.e(ind + 1, "_ => { }")
            self.e(ind, "}")
        elif c < 0.75:
            sc = r.choice([f"{self.v('H')}.m", f"{self.v('H')}.b.get()", f"{self.v('B')}.get()"])
            self.e(ind, f"match {r.choice(['', 'mut '])}{sc} {{")
            self.e(ind + 1, "Text(s) => {")
            self.block(ind + 2)
            if r.random() < 0.6:
                self.e(ind + 2, "print(s)")
            self.e(ind + 1, "}")
            self.e(ind + 1, "_ => { }")
            self.e(ind, "}")
        elif c < 0.78:
            self.e(ind, f"match {r.choice(['', 'mut ', 'move '])}{self.v('O')} {{")
            self.e(ind + 1, "Some(x) => {")
            self.block(ind + 2)
            if r.random() < 0.6:
                self.e(ind + 2, "print(msize(x))")
            self.e(ind + 1, "}")
            self.e(ind + 1, "None => { }")
            self.e(ind, "}")
        elif c < 0.8:
            self.e(ind, f"for mut {self.fresh('x')} in {L} {{")
            self.block(ind + 1)
            self.e(ind, "}")
        elif c < 0.9:
            self.e(ind, f"for {self.fresh('x')} in move {L} {{")
            self.block(ind + 1)
            self.e(ind, "}")
        else:
            self.e(ind, "if i0 > -5 {")
            self.block(ind + 1)
            self.e(ind, "}")
        self.depth -= 1

    def block(self, ind):
        for _ in range(self.r.randint(1, 3)):
            self.stmt(ind)

    def program(self):
        self.e(0, "fn two(a: view String, b: view List<String>) { print(a) }")
        self.e(0, "fn takes_h(h: Holder) -> Int { return h.xs.len() }")
        self.e(0, "fn opt_size(o: view Option<Msg>) -> Int { match o { Some(m) => { return msize(m) } None => { return -1 } } }")
        self.e(0, "fn take_opt(o: Option<String>) -> Int { match move o { Some(s) => { return s.len() } None => { return 0 } } }")
        self.e(0, "fn mkstr(k: Int) -> String { return String.from(\"made string long enough\") }")
        self.e(0, "fn main() {")
        self.e(1, "var i0 = 1")
        self.e(1, "var i1 = 0")
        for s in self.vars["S"]:
            self.e(1, f"var {s} = String.from(\"initial string value {s}\")")
        for l in self.vars["L"]:
            self.e(1, f"var {l} = List<String>.new()")
            self.e(1, f"{l}.push(String.from(\"element one of {l}\"))")
            self.e(1, f"{l}.push(String.from(\"element two of {l}\"))")
        for m in self.vars["M"]:
            self.e(1, f"var {m} = mkmsg({self.r.randint(0, 5)})")
        self.e(1, "var f0 = move || 7")
        self.e(1, "var h0 = mkh()")
        self.e(1, "var o0 = Some(mkmsg(2))")
        self.e(1, "var b0 = Box.new(mkmsg(1))")
        for _ in range(self.r.randint(3, 10)):
            self.stmt(1)
        self.e(0, "}")
        return SOUP_PRE + "\n".join(self.lines) + "\n"


def sh(args, timeout=60, env=None):
    try:
        p = subprocess.run(args, capture_output=True, timeout=timeout, stdin=subprocess.DEVNULL, env=env)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return -999, b"", b"timeout"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-n", type=int, default=200)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("--show-accepted", action="store_true")
    ap.add_argument("--soup", action="store_true", help="random statements over owned variables (checker decides what is legal)")
    a = ap.parse_args()
    global SOUP
    SOUP = a.soup
    seed0 = a.seed if a.seed is not None else random.randrange(1 << 30)
    print(f"fuzz_unsound: seed={seed0} n={a.n}", flush=True)
    tmp = tempfile.mkdtemp(prefix="yfu_")
    stats = dict(rejected=0, accepted=0, holes=0, invalid=0)
    seen = set()
    for i in range(a.n):
        seed = seed0 + i
        src = program(seed)
        body = src.split("fn main() {")[-1]
        if body in seen:
            continue
        seen.add(body)
        f = os.path.join(tmp, "p.yx")
        open(f, "w").write(src)
        rc, _, err = sh([YERUX, "check", f])
        if rc != 0:
            e = err.decode(errors="replace")
            if "cannot use" in e or "cannot change" in e or "appears twice" in e or "moved" in e or "borrow" in e or "frozen" in e or "reading it" in e or "in place" in e or "dangle" in e or "cannot lend" in e or "cannot move" in e or "cannot append" in e:
                stats["rejected"] += 1
            else:
                stats["invalid"] += 1
                if stats["invalid"] <= 5:
                    print(f"seed {seed}: generator problem: {e[:200]}", flush=True)
            continue
        stats["accepted"] += 1
        rc, cc, err = sh([YERUX, "c", f])
        open(os.path.join(tmp, "p.c"), "wb").write(cc)
        asan = os.path.join(tmp, "p.asan")
        rc, _, err = sh(["cc", "-g", "-O0", "-w", "-fsanitize=address,undefined", "-fno-sanitize-recover=undefined", "-o", asan, os.path.join(tmp, "p.c"), "-lm"], timeout=120)
        bad = None
        if rc == 0:
            env = dict(os.environ, ASAN_OPTIONS="detect_leaks=0")
            rc2, out, err2 = sh([asan], env=env)
            if b"AddressSanitizer" in err2 or b"runtime error" in err2:
                bad = "ASAN: " + err2.decode(errors="replace").split("\n")[1][:150]
        exe = os.path.join(tmp, "p.n")
        rc, _, _ = sh([YERUX, "build", f, "-o", exe, "--native-debug-alloc"])
        if rc == 0:
            rc3, out3, err3 = sh([exe])
            if rc3 == 0 and b"live: 0\n" not in err3:
                bad = (bad or "") + " native-debug-alloc: " + err3.decode(errors="replace").replace("\n", " ")[-80:]
            if rc3 not in (0, 101, -999):
                bad = (bad or "") + f" native exit {rc3}"
        if a.show_accepted:
            print("ACCEPTED", seed, body.strip().split("\n")[1:6])
        if bad:
            stats["holes"] += 1
            path = os.path.join(BUGS, f"unsound_{seed}.yx")
            open(path, "w").write(src)
            print(f"HOLE seed {seed}: {bad}; saved {path}", flush=True)
    shutil.rmtree(tmp, ignore_errors=True)
    print("done:", stats)
    return 1 if stats["holes"] else 0


if __name__ == "__main__":
    sys.exit(main())
