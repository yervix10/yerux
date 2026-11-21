#!/usr/bin/env python3
"""Compiler robustness fuzzer: random token / byte mutations of the example and test programs, plus a fixed set
of pathological sources (empty file, huge expressions, deep nesting, unicode, invalid UTF-8, 10k functions, ...).
The compiler must never crash (Rust panic exits 101 and prints `panicked at`, a signal, a hang): it either
accepts the program or prints a diagnostic and exits 1.  Mutants that are accepted are also pushed through
the native backend (IR lowering, optimizer, code generation) and the aarch64 one.

usage: tests/fuzz_parse.py [-n COUNT] [--seed SEED] [-j JOBS] [--edge-only] [--c]
Crashing inputs are saved to tests/bugs/crash_<hash>.yx"""
import argparse, glob, hashlib, os, random, re, subprocess, sys, tempfile
from concurrent.futures import ThreadPoolExecutor

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
YERUX = os.path.join(ROOT, "target", "release", "yerux")
BUGS = os.path.join(ROOT, "tests", "bugs")
TOK = re.compile(r'"(?:[^"\\]|\\.)*"|//[^\n]*|[A-Za-z_][A-Za-z_0-9]*|[0-9][0-9_.]*|\s+|.', re.S)
EXTRA = ["(", ")", "{", "}", "[", "]", "<", ">", ",", ".", ":", ";", "=", "==", "->", "=>", "&&", "||", "!", "-", "%", "/", "*",
         "fn", "let", "var", "struct", "enum", "trait", "impl", "match", "if", "else", "while", "for", "in", "return", "defer",
         "break", "continue", "mod", "use", "pub", "view", "mut", "own", "Int", "Bool", "Text", "String", "List", "Map", "Set",
         "Float", "Char", "Self", "_", "|", "\"", "'", "0", "9223372036854775808", "1e999", "0x", "é", "中", "\U0001F600",
         "\x00", "{{", "}}", "\"{", "}\"", "..", "..=", "?", "@", "#", "$", "\\", "\t", "\n", "'a'", "'\\u{110000}'"]


def seeds():
    out = []
    for f in sorted(glob.glob(os.path.join(ROOT, "examples", "*.yx")) + glob.glob(os.path.join(ROOT, "tests", "*.yx"))):
        try:
            if os.path.getsize(f) < 20000:
                out.append(open(f, "rb").read().decode("utf-8", "replace"))
        except OSError:
            pass
    return out


def mutate(r, src):
    toks = TOK.findall(src)
    for _ in range(r.choice([1, 1, 1, 2, 3, 5])):
        if not toks:
            break
        k = r.randrange(7)
        i = r.randrange(len(toks))
        if k == 0:
            del toks[i]
        elif k == 1:
            toks.insert(i, toks[r.randrange(len(toks))])
        elif k == 2:
            toks[i] = r.choice(EXTRA)
        elif k == 3:
            j = r.randrange(len(toks)); toks[i], toks[j] = toks[j], toks[i]
        elif k == 4:
            toks = toks[:i]
        elif k == 5:
            j = min(len(toks), i + r.randint(1, 30)); del toks[i:j]
        else:
            toks.insert(i, r.choice(EXTRA))
    return "".join(toks)


def edge_cases():
    big = 20000
    yield "empty", b""
    yield "only_ws", b"   \n\t\n"
    yield "only_comment", b"// hi"
    yield "no_main", b"fn f() -> Int { return 1 }\n"
    yield "deep_paren", b"fn main() { print(" + b"(" * big + b"1" + b")" * big + b") }\n"
    yield "deep_neg", b"fn main() { print(" + b"-" * big + b"1) }\n"
    yield "deep_not", b"fn main() { print(" + b"!" * big + b"true) }\n"
    yield "long_sum", b"fn main() { print(" + b"1 + " * big + b"1) }\n"
    yield "long_and", b"fn main() { print(" + b"true && " * big + b"true) }\n"
    yield "deep_if", b"fn main() {" + b"if true {" * 5000 + b"print(1)" + b"}" * 5000 + b"}\n"
    yield "deep_while", b"fn main() {" + b"while false {" * 5000 + b"}" * 5000 + b"}\n"
    yield "deep_block", b"fn main() {" + b"{" * big + b"}" * big + b"}\n"
    yield "deep_call", b"fn f(x: Int) -> Int { return x }\nfn main() { print(" + b"f(" * 5000 + b"1" + b")" * 5000 + b") }\n"
    yield "deep_list_ty", b"fn main() { let x: " + b"List<" * 3000 + b"Int" + b">" * 3000 + b" = List.new() }\n"
    yield "deep_tuple_ty", b"fn main() { let x: " + b"(" * 3000 + b"Int, Int" + b")" * 3000 + b" = 1 }\n"
    yield "deep_match", b"fn main() { match 1 {" + b"_ => { match 1 {" * 2000 + b"_ => {}" + b"}}" * 2000 + b"} }\n"
    yield "deep_index", b"fn main() { let l = List<Int>.new()\n print(l" + b".len()" * 5000 + b") }\n"
    yield "deep_string_interp", b'fn main() { print("' + b"{" * 3000 + b"1" + b"}" * 3000 + b'") }\n'
    yield "long_ident", b"fn main() { let " + b"a" * 200000 + b" = 1\n print(" + b"a" * 200000 + b") }\n"
    yield "long_line", b"fn main() { print(1) }" + b" " * 300000 + b"\n"
    yield "long_string", b'fn main() { print("' + b"x" * 1000000 + b'") }\n'
    yield "long_comment", b"// " + b"x" * 1000000 + b"\nfn main() { }\n"
    yield "unterminated_str", b'fn main() { print("abc'
    yield "unterminated_comment", b"fn main() { /* x"
    yield "unterminated_interp", b'fn main() { print("a {1 + ") }\n'
    yield "invalid_utf8", b"fn main() { print(\"\xff\xfe\") }\n"
    yield "invalid_utf8_ident", b"fn main() { let \xc3 = 1 }\n"
    yield "nul", b"fn main() { \x00 }\n"
    yield "bom", b"\xef\xbb\xbffn main() { print(1) }\n"
    yield "crlf", b"fn main() {\r\n print(1)\r\n}\r\n"
    yield "unicode_ident", "fn main() { let año = 1\n let 中文 = 2\n let \U0001F600 = 3\n print(año) }\n".encode()
    yield "unicode_fn", "fn größe() -> Int { return 1 }\nfn main() { print(größe()) }\n".encode()
    yield "unicode_type", "struct Ñandú { x: Int }\nfn main() { let a = Ñandú { x: 1 }\n print(a.x) }\n".encode()
    yield "huge_int", b"fn main() { print(99999999999999999999999999999999999999999) }\n"
    yield "huge_float", b"fn main() { print(1" + b"0" * 5000 + b".0) }\n"
    yield "huge_exp", b"fn main() { print(1e99999999999) }\n"
    yield "hex_empty", b"fn main() { print(0x) }\n"
    yield "many_fns", b"".join(b"fn f%d() -> Int { return %d }\n" % (i, i) for i in range(10000)) + b"fn main() { print(f9999()) }\n"
    yield "many_fns_calls", b"".join(b"fn f%d() -> Int { return %s }\n" % (i, (b"f%d() + 1" % (i - 1)) if i else b"0") for i in range(3000)) + b"fn main() { print(f2999()) }\n"
    yield "many_structs", b"".join(b"struct S%d { a: Int, b: Int }\n" % i for i in range(3000)) + b"fn main() { }\n"
    yield "many_locals", b"fn main() {\n" + b"".join(b"let a%d = %d\n" % (i, i) for i in range(20000)) + b"print(a19999) }\n"
    yield "many_params", b"fn f(" + b", ".join(b"p%d: Int" % i for i in range(2000)) + b") -> Int { return p0 }\nfn main() { print(f(" + b", ".join(b"%d" % i for i in range(2000)) + b")) }\n"
    yield "many_fields", b"struct S {" + b", ".join(b"f%d: Int" % i for i in range(3000)) + b"}\nfn main() { }\n"
    yield "many_variants", b"enum E {" + b", ".join(b"V%d" % i for i in range(3000)) + b"}\nfn main() { }\n"
    yield "many_match_arms", b"fn main() { let x = 5\n match x {" + b"".join(b"%d => { print(%d) }\n" % (i, i) for i in range(5000)) + b"_ => {} } }\n"
    yield "many_args_list", b"fn main() { let l = List<Int>.new()\n" + b"".join(b"l.push(%d)\n" % i for i in range(30000)) + b"print(l.len()) }\n"
    yield "recursive_struct", b"struct A { b: B }\nstruct B { a: A }\nfn main() { }\n"
    yield "recursive_type_alias", b"fn f(x: f) { }\nfn main() { }\n"
    yield "recursive_generic", b"struct W<T> { x: T }\nfn main() { let a: W<W<W<W<W<W<Int>>>>>> = 1 }\n"
    yield "poly_recursion", b"fn f<T>(x: T) { f<List<T>>(List.new()) }\nfn main() { f(1) }\n"
    yield "infinite_inline", b"fn f(n: Int) -> Int { if n == 0 { return 0 }\n return f(n - 1) + f(n - 1) }\nfn main() { print(f(3)) }\n"
    yield "stray_brace", b"}\n"
    yield "only_fn", b"fn"
    yield "mod_missing", b"mod nothere\nfn main() { }\n"
    yield "mod_self", b"use nothere\nfn main() { }\n"
    yield "dup_main", b"fn main() { }\nfn main() { }\n"
    yield "main_args", b"fn main(x: Int) { }\n"
    yield "main_ret", b"fn main() -> Int { return 1 }\n"
    yield "div_const_zero", b"fn main() { print(1 / 0) }\n"
    yield "min_div", b"fn main() { print((-9223372036854775807 - 1) / (-1)) }\n"
    yield "shift_huge", b"fn main() { print(1 << 99999) }\n"
    yield "str_index_huge", b"fn main() { let s = \"abc\"\n print(s.byte(99999999999999)) }\n"
    yield "neg_lit_min", b"fn main() { print(-9223372036854775808) }\n"


class Result:
    pass


def load_factor():
    """>= 1: how much slower than an idle machine we should expect to be (load average per CPU)."""
    try:
        return max(1.0, os.getloadavg()[0] / (os.cpu_count() or 1))
    except OSError:
        return 1.0


def run(args, inp=None, timeout=60):
    """A timeout scales with the machine load and is retried once with twice the time before it counts: a
    compiler that is merely slow because other jobs are running must not look like a hang. Real hangs still
    fail (they exceed 2x the scaled limit)."""
    limit = timeout * load_factor()
    for attempt in (1, 2):
        try:
            p = subprocess.run(args, capture_output=True, timeout=limit, stdin=subprocess.DEVNULL)
            return p.returncode, p.stdout, p.stderr
        except subprocess.TimeoutExpired:
            limit *= 2
    return "timeout", b"", b""


def crashed(rc, err):
    if rc == "timeout":
        return "timeout"
    if rc is None or (isinstance(rc, int) and rc < 0):
        return f"signal {rc}"
    if b"panicked at" in err or b"RUST_BACKTRACE" in err or rc == 101 or b"internal compiler error" in err:
        return "rust panic"
    if b"stack overflow" in err.lower() and b"thread" in err:
        return "stack overflow"
    return None


def poorly_reported(rc, err):
    """exit 1 must come with an `error` diagnostic."""
    return rc == 1 and b"error" not in err.lower()


def check_one(src, tmp, native=True, tag="x"):
    path = os.path.join(tmp, f"{tag}.yx")
    with open(path, "wb") as f:
        f.write(src if isinstance(src, bytes) else src.encode("utf-8", "surrogatepass"))
    out = os.path.join(tmp, f"{tag}.bin")
    steps = [[YERUX, "check", path]]
    rc, so, se = run(steps[0])
    why = crashed(rc, se)
    if why:
        return why, "check", se
    if poorly_reported(rc, se):
        return "no diagnostic", "check", se
    if rc == 0 and native:
        for cmd in ([YERUX, "build", path, "--native", "-o", out], [YERUX, "build", path, "--native", "--no-opt", "-o", out],
                    [YERUX, "build", path, "--native", "--target", "aarch64", "-o", out + "a"], [YERUX, "ir", path]):
            rc2, so2, se2 = run(cmd)
            why = crashed(rc2, se2)
            if why:
                return why, " ".join(cmd[1:]), se2
        rc2, so2, se2 = run([YERUX, "fmt", "--stdout", path])
        why = crashed(rc2, se2)
        if why:
            return why, "fmt", se2
    elif rc != 0:
        rc2, so2, se2 = run([YERUX, "fmt", "--stdout", path])
        why = crashed(rc2, se2)
        if why:
            return why, "fmt", se2
    return None, "", b""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-n", type=int, default=2000)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("-j", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--edge-only", action="store_true")
    ap.add_argument("--no-edge", action="store_true")
    a = ap.parse_args()
    seed0 = a.seed if a.seed is not None else random.randrange(1 << 30)
    tmp = tempfile.mkdtemp(prefix="yerux_fp_")
    os.makedirs(BUGS, exist_ok=True)
    bad = {}
    jobs = []
    if not a.no_edge:
        for name, src in edge_cases():
            jobs.append((f"edge:{name}", src))
    if not a.edge_only:
        ss = seeds()
        for i in range(a.n):
            r = random.Random(seed0 + i)
            jobs.append((f"seed {seed0 + i}", mutate(r, r.choice(ss))))
    print(f"fuzz_parse: seed={seed0} cases={len(jobs)}")

    def one(ij):
        i, (name, src) = ij
        why, where, err = check_one(src, tmp, True, f"c{i % 64}_{os.getpid()}_{i}")
        if why:
            h = hashlib.sha1(src if isinstance(src, bytes) else src.encode("utf-8", "surrogatepass")).hexdigest()[:8]
            print(f"CRASH {name}: {why} in `{where}`: {err.decode(errors='replace').strip().splitlines()[:2]}")
            if not name.startswith("edge:"):  # edge cases are regenerated by this script
                with open(os.path.join(BUGS, f"crash_{h}.yx"), "wb") as f:
                    f.write(src if isinstance(src, bytes) else src.encode("utf-8", "surrogatepass"))
            bad[name] = why
        for fn in glob.glob(os.path.join(tmp, f"c{i % 64}_{os.getpid()}_{i}.*")):
            os.unlink(fn)

    with ThreadPoolExecutor(a.j) as ex:
        list(ex.map(one, enumerate(jobs)))
    print(f"done: {len(jobs)} cases, {len(bad)} crashes")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
