#!/usr/bin/env python3
"""Fuzzer for std.io stdin handling (read_line / read_all, 64 KiB buffer, CRLF, UTF-8 validation, EOF).

Random stdin payloads (lines around the 65536-byte buffer boundary, CRLF, empty lines, invalid UTF-8,
missing final newline, NUL bytes) are fed to a few fixed reader programs. Each program's stdout is compared
between the C backend, native and native --no-opt, and against a Python model of the documented semantics
(docs/STDLIB_IO.md): a line has its `\\n` (and then one `\\r`) removed; invalid UTF-8 is an IoError value
(the line is still consumed); `read_all` returns the rest of stdin or an IoError.
Also `--native-debug-alloc` must end with live: 0.

usage: tests/fuzz_io.py [-n COUNT] [--seed SEED]
"""
import argparse, os, random, subprocess, sys, tempfile, shutil

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
YERUX = os.path.join(ROOT, "target", "release", "yerux")
BUGS = os.path.join(ROOT, "tests", "bugs")

HEAD = """import std.io
fn sum_bytes(s: view String) -> Int {
  var t = 0
  for i in 0..s.len() { t = (t * 31 + s.byte(i)) % 1000003 }
  return t
}
fn show_line(r: Result<Option<String>, io.IoError>) -> Bool {
  match r {
    Ok(o) => {
      match o {
        Some(line) => { print("L {line.len()} {sum_bytes(line)}"); return true }
        None => { print("EOF"); return false }
      }
    }
    Err(e) => { print("ERR {e.kind == io.IoErrorKind.InvalidData} {e.code}"); return true }
  }
}
"""

PROGRAMS = {
    # read lines until EOF
    "lines": HEAD + """fn main() {
  var n = 0
  while n < 100000 {
    n = n + 1
    if !show_line(io.read_line()) { break }
  }
}
""",
    # K lines, then the rest
    "lines_then_all": HEAD + """fn main() {
  for i in 0..K {
    if !show_line(io.read_line()) { break }
  }
  match io.read_all() {
    Ok(s) => { print("ALL {s.len()} {sum_bytes(s)}") }
    Err(e) => { print("ALLERR {e.code}") }
  }
  match io.read_line() {
    Ok(o) => { print("after {o.is_some()}") }
    Err(e) => { print("after err") }
  }
}
""",
    "all": HEAD + """fn main() {
  match io.read_all() {
    Ok(s) => { print("ALL {s.len()} {sum_bytes(s)}") }
    Err(e) => { print("ALLERR {e.code}") }
  }
  match io.read_all() {
    Ok(s) => { print("ALL2 {s.len()}") }
    Err(e) => { print("ALLERR2 {e.code}") }
  }
}
""",
}


def payload(r):
    parts = []
    for _ in range(r.randint(0, 8)):
        k = r.random()
        if k < 0.25:
            ln = r.choice([0, 1, 5, 70, 65534, 65535, 65536, 65537, 65538, 131071, 131072, 140000])
            body = bytes(r.choice(b"abcdefghij xyz0123456789") for _ in range(min(ln, 200))) * (ln // max(1, min(ln, 200)) + 1)
            body = body[:ln]
        elif k < 0.45:
            body = "héllo wörld 中文 😀".encode() * r.randint(1, 3)
        elif k < 0.55:
            body = bytes([0xFF, 0xFE, 0x41]) if r.random() < 0.5 else b"\xc3"  # invalid UTF-8
        elif k < 0.6:
            body = b"a\x00b"
        elif k < 0.7:
            body = b"\xed\xa0\x80"  # surrogate
        else:
            body = b"line%d" % r.randint(0, 99)
        eol = r.choice([b"\n", b"\n", b"\n", b"\r\n", b"\r", b"", b"\n\n", b"\r\r\n"])
        parts.append(body + eol)
    return b"".join(parts)


def valid_utf8(b):
    try:
        b.decode("utf-8")
        return True
    except UnicodeDecodeError:
        return False


def sum_bytes(b):
    t = 0
    for c in b:
        t = (t * 31 + c) % 1000003
    return t


def model(mode, data, K):
    out = []
    pos = 0

    def read_line():
        nonlocal pos
        if pos >= len(data):
            return None
        i = data.find(b"\n", pos)
        if i < 0:
            # a last line without `\n` is returned verbatim, including a final `\r` (docs/STDLIB_IO.md)
            line = data[pos:]
            pos = len(data)
            return line
        line = data[pos:i]
        pos = i + 1
        if line.endswith(b"\r"):
            line = line[:-1]
        return line

    def show_line():
        line = read_line()
        if line is None:
            out.append("EOF")
            return False
        if valid_utf8(line):
            out.append(f"L {len(line)} {sum_bytes(line)}")
        else:
            out.append("ERR true 0")
        return True

    if mode == "lines":
        while show_line():
            pass
    elif mode == "lines_then_all":
        for _ in range(K):
            if not show_line():
                break
        rest = data[pos:]
        pos = len(data)
        if valid_utf8(rest):
            out.append(f"ALL {len(rest)} {sum_bytes(rest)}")
        else:
            out.append("ALLERR 0")
        out.append("after false")
    else:
        if valid_utf8(data):
            out.append(f"ALL {len(data)} {sum_bytes(data)}")
            out.append("ALL2 0")
        else:
            out.append("ALLERR 0")
            out.append("ALL2 0")
    return "\n".join(out) + "\n"


def sh(args, inp=b"", timeout=30):
    try:
        p = subprocess.run(args, input=inp, capture_output=True, timeout=timeout)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return -999, b"", b"timeout"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-n", type=int, default=100)
    ap.add_argument("--seed", type=int, default=None)
    a = ap.parse_args()
    seed0 = a.seed if a.seed is not None else random.randrange(1 << 30)
    print(f"fuzz_io: seed={seed0} n={a.n}", flush=True)
    tmp = tempfile.mkdtemp(prefix="yfio_")
    built = {}
    for mode, src in PROGRAMS.items():
        for K in ([1, 2, 3] if mode == "lines_then_all" else [0]):
            s = src.replace("K {", f"{K} {{") if mode == "lines_then_all" else src
            f = os.path.join(tmp, f"{mode}_{K}.yx")
            open(f, "w").write(s)
            exes = {}
            for name, flags in (("c", []), ("native", ["--native"]), ("noopt", ["--native", "--no-opt"]), ("leak", ["--native", "--native-debug-alloc"])):
                exe = f + "." + name
                rc, _, err = sh([YERUX, "build", f, "-o", exe] + flags, timeout=120)
                if rc != 0:
                    print("build failed", mode, name, err.decode()[:300])
                    return 2
                exes[name] = exe
            built[(mode, K)] = exes
    bad = 0
    for i in range(a.n):
        seed = seed0 + i
        r = random.Random(seed)
        data = payload(r)
        key = r.choice(list(built))
        mode, K = key
        exp = model(mode, data, K).encode()
        res = {}
        for name, exe in built[key].items():
            res[name] = sh([exe], data)
        problems = []
        for name in ("c", "native", "noopt"):
            if res[name][0] != 0 or res[name][1] != exp:
                problems.append(name)
        if b"live: 0\n" not in res["leak"][2] or res["leak"][1] != exp:
            problems.append("leak")
        if problems:
            bad += 1
            base = os.path.join(BUGS, f"io_{seed}")
            open(base + ".stdin", "wb").write(data)
            print(f"MISMATCH seed {seed} mode {mode}/{K} {problems}; stdin saved {base}.stdin (len {len(data)})", flush=True)
            for name in problems:
                print(" ", name, res[name][0], res[name][1][:300], res[name][2][-100:], flush=True)
            print("  expected", exp[:300], flush=True)
            shutil.copy(os.path.join(tmp, f"{mode}_{K}.yx"), base + ".yx")
    shutil.rmtree(tmp, ignore_errors=True)
    print(f"done: n={a.n} bad={bad}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
