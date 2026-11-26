#!/usr/bin/env python3
"""Differential fuzz of the Yerux standard library against Python oracles.

Random and malformed inputs for std.json, std.csv, std.toml, std.text and std.args are compared with the
Python library that defines the same behaviour (`json`, `csv`, `tomllib`) or with an implementation of the
rule documented in docs/STDLIB.md / docs/REGEX_CSV_TOML.md / docs/STDLIB_IO.md. Cases are compiled in
batches (one Yerux program per batch, native backend by default), so a few thousand cases take seconds.

    python3 tools/fuzz/fuzz_stdlib.py [--cases N] [--seed S] [--suite LIST] [--batch B]
                                      [--jobs J] [--backend c|native] [--out DIR] [--verbose]

Every discrepancy prints the suite, the Yerux input (already written as a `.yx` literal when it is not
printable), the expected value and the value the compiler produced; the offending program of a case that
crashed the compiler or the program is kept under the output directory (default /tmp/yerux_fuzz_stdlib).

Exit status: 0 when nothing was found, 1 when at least one discrepancy was. This is a tool: `tests/` keeps
the curated cases and `tests/run_all.sh` does not run it.
"""
import argparse
import csv
import io
import json
import os
import random
import re
import subprocess
import sys
import tempfile
import tomllib
from concurrent.futures import ThreadPoolExecutor
from decimal import Decimal, ROUND_HALF_UP

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
YERUX = os.environ.get("YERUX", os.path.join(ROOT, "target/release/yerux"))
SEP, REC = 31, 30  # unit/record separators, for flattening rows of strings

HEADER = r'''import std.json
import std.csv
import std.toml
import std.text
import std.fmt
import std.args

fn mk(codes: view [Int]) -> String {
  var s = String.new()
  for c in codes { s.push_char(Char.from_int(c)) }
  return s
}

fn esc(s: view String) -> String {
  var o = text.replace(s, "\\", "\\\\")
  o = text.replace(o, "\n", "\\n")
  o = text.replace(o, "\t", "\\t")
  o = text.replace(o, "\r", "\\r")
  return o
}

fn emit(id: Int, v: view String) {
  let e = esc(v)
  print("{id}\t{e}")
}

fn flat1(parts: view List<String>) -> String {
  var o = String.new()
  o.push_str("[")
  for i in 0..parts.len() {
    if i > 0 { o.push_str(", ") }
    o.push_str(parts.at(i))
  }
  o.push_str("]")
  return o
}

fn flat2(rows: view List<List<String>>) -> String {
  var o = String.new()
  for i in 0..rows.len() {
    o.push_char(Char.from_int(31))
    let r = rows.at(i)
    for j in 0..r.len() {
      o.push_str(r.at(j))
      o.push_char(Char.from_int(30))
    }
  }
  return o
}

fn summary(p: view args.Parsed) -> String {
  var o = String.new()
  o.push_str(text.join(p.flags, ","))
  o.push_str("|")
  for i in 0..p.options.len() {
    let t = p.options.at(i)
    if i > 0 { o.push_str(",") }
    o.push_str(t.0)
    o.push_str("=")
    o.push_str(t.1)
  }
  o.push_str("|")
  o.push_str(text.join(args.positionals(p), ","))
  o.push_str("|")
  o.push_str(p.subcommand)
  o.push_str("|")
  if p.help { o.push_str("help") }
  o.push_str("|")
  o.push_str(text.join(p.errors, ";"))
  return o
}

fn main() {
'''

TAIL = "}\n"


# ───────────────────────── Yerux source helpers ─────────────────────────

SAFE = set('\n\t\r"\\') | {chr(c) for c in range(32, 127)}


def yxexpr(s, tag, pre):
    """A Yerux expression of type String for `s`; appends setup lines to `pre` when it is not printable."""
    if all(ch in SAFE for ch in s):
        lit = (s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")
                .replace("\t", "\\t").replace("\r", "\\r"))
        return '"' + lit.replace("{", "{{").replace("}", "}}") + '"'
    name = "v%d" % tag
    pre.append("  let %s = mk([%s])" % (name, ", ".join(str(ord(ch)) for ch in s)))
    return name


def esc_py(s):
    return (s.replace("\\", "\\\\").replace("\n", "\\n").replace("\t", "\\t").replace("\r", "\\r"))


def unesc(s):
    out, i = [], 0
    while i < len(s):
        if s[i] == "\\" and i + 1 < len(s):
            nxt = s[i + 1]
            out.append({"n": "\n", "t": "\t", "r": "\r", "\\": "\\"}.get(nxt, nxt))
            i += 2
        else:
            out.append(s[i])
            i += 1
    return "".join(out)


class Case:
    __slots__ = ("sid", "suite", "lines", "expect", "src")

    def __init__(self, sid, suite, lines, expect, src):
        self.sid, self.suite, self.lines, self.expect, self.src = sid, suite, lines, expect, src


def flat_py(rows):
    return "".join(chr(SEP) + "".join(f + chr(REC) for f in row) for row in rows)


# ───────────────────────── generators ─────────────────────────

def json_value(rng, depth=0):
    kinds = ["int", "float", "str", "bool", "null"] + ([] if depth > 2 else ["arr", "obj"])
    k = rng.choice(kinds)
    if k == "int":
        return rng.choice([0, 1, -1, 42, -7, 2**31, -(2**31), 2**63 - 1, -2**63, rng.randint(-10**6, 10**6)])
    if k == "float":
        return rng.choice([0.0, 1.5, -2.25, 1e10, 1.0, 3.14159, 1e-7, 123456789.125])
    if k == "str":
        return rng.choice(["", "a", "hello", "with space", 'quote"in', "back\\slash", "tab\there",
                           "line\nfeed", "unicode: é 漢 🎉", "ctrl", "{}", "[]", "null", "true"])
    if k == "bool":
        return rng.choice([True, False])
    if k == "null":
        return None
    if k == "arr":
        return [json_value(rng, depth + 1) for _ in range(rng.randint(0, 4))]
    return {rng.choice(["a", "b", "key", "n1", "x y", "é"]): json_value(rng, depth + 1)
            for _ in range(rng.randint(0, 4))}


def json_text(rng):
    if rng.random() < 0.6:
        return json.dumps(json_value(rng), ensure_ascii=rng.random() < 0.5)
    base = json.dumps(json_value(rng), ensure_ascii=False)
    if not base:
        return base
    i = rng.randrange(len(base))
    op = rng.choice(["truncate", "insert", "drop", "swap", "pad", "dup"])
    if op == "truncate":
        return base[:i]
    if op == "insert":
        return base[:i] + rng.choice(['"', "}", "]", ",", ":", "0", "-", ".", "e", "\\", " ", "t"]) + base[i:]
    if op == "drop":
        return base[:i] + base[i + 1:]
    if op == "swap" and i + 1 < len(base):
        return base[:i] + base[i + 1] + base[i] + base[i + 2:]
    if op == "pad":
        return rng.choice([" ", "\n", "\t"]) + base + rng.choice([" ", "\n", "\t"])
    return base + base


def gen_json(rng, n, start):
    cases = []
    for k in range(n):
        sid = start + k
        text = json_text(rng)
        pre = []
        e = yxexpr(text, sid, pre)
        lines = pre + [
            "  match json.parse(%s) {" % e,
            "    Ok(v) => { let s%d = json.to_string(v)" % sid,
            "      emit(%d, s%d) }" % (sid, sid),
            '    Err(e) => { emit(%d, "!ERR") }' % sid,
            "  }",
        ]
        try:
            expected = json.dumps(json.loads(text), ensure_ascii=False, separators=(",", ":"))
        except Exception:
            expected = "!ERR"
        cases.append(Case(sid, "json", lines, expected, text))
    return cases


def csv_text(rng):
    rows = [[rng.choice(["", "a", "b c", 'q"uote', "comma,here", "line\nbreak", " space ", "ünïcode",
                          "0", "1.5", "x" * rng.randint(0, 5)])
             for _ in range(rng.randint(1, 4))] for _ in range(rng.randint(1, 3))]
    buf = io.StringIO()
    csv.writer(buf, lineterminator="\r\n" if rng.random() < 0.7 else "\n").writerows(rows)
    return buf.getvalue()


def csv_cases(rng, n, start):
    cases = []
    for k in range(n):
        sid = start + 2 * k
        text = csv_text(rng)
        pre = []
        e = yxexpr(text, sid, pre)
        rows = list(csv.reader(io.StringIO(text, newline="")))
        cases.append(Case(sid, "csv-parse", pre + [
            "  let rows%d = csv.parse(%s)" % (sid, e),
            "  emit(%d, flat2(rows%d))" % (sid, sid)], flat_py(rows), text))
        build = ["  var t%d = List<List<String>>.new()" % sid]
        for ri, row in enumerate(rows):
            rn = "r%d_%d" % (sid, ri)
            build.append("  var %s = List<String>.new()" % rn)
            for f in row:
                fx = yxexpr(f, sid * 100 + len(build), build)
                build.append("  %s.push(%s)" % (rn, ("move " + fx) if fx.startswith("v") else fx))
            build.append("  t%d.push(move %s)" % (sid, rn))
        buf = io.StringIO()
        csv.writer(buf, lineterminator="\r\n").writerows(rows)
        build.append("  emit(%d, csv.write(t%d))" % (sid + 1, sid))
        cases.append(Case(sid + 1, "csv-write", build, buf.getvalue(), text))
    return cases


def toml_text(rng):
    lines, keys = [], []
    for table in rng.sample(["server", "build", "pkg", "a"], rng.randint(0, 2)):
        lines.append("[ %s ]" % table if rng.random() < 0.3 else "[%s]" % table)
        names = rng.sample(["port", "host", "name", "flag", "list", "n"], rng.randint(1, 3))
        for name in names:
            kind = rng.choice(["int", "str", "bool", "arr"])
            if kind == "int":
                lines.append("%s = %d" % (name, rng.choice([0, 8080, -1, 42])))
            elif kind == "str":
                lines.append('%s = "%s"' % (name, rng.choice(["x", "hello world", 'q\\"uote', "tab\\\\there", "ünïcode"])))
            elif kind == "bool":
                lines.append("%s = %s" % (name, rng.choice(["true", "false"])))
            else:
                lines.append("%s = [%s]" % (name, ", ".join(
                    rng.choice(["1", "2", '"a"', '"b c"']) for _ in range(rng.randint(0, 3)))))
            keys.append(table + "." + name)
    if rng.random() < 0.3:
        lines = [rng.choice(["", "# comment"])] + lines
    return "\n".join(lines) + ("\n" if rng.random() < 0.8 else ""), keys


def toml_cases(rng, n, start):
    cases = []
    for k in range(n):
        sid = start + k
        text, keys = toml_text(rng)
        try:
            doc = tomllib.loads(text)
        except Exception:
            doc = None
        key = rng.choice(keys + ["missing", "server.missing"]) if keys else "missing"
        want = None
        if doc is not None:
            cur = doc
            for part in key.split("."):
                cur = cur.get(part) if isinstance(cur, dict) else None
            want = cur
        pre = []
        e = yxexpr(text, sid, pre)
        ke = yxexpr(key, sid * 100 + 1, pre)
        lines = pre + [
            "  let g%d = toml.get_int(%s, %s)" % (sid, e, ke),
            "  let h%d = toml.get_string(%s, %s)" % (sid, e, ke),
            "  let b%d = toml.get_bool(%s, %s)" % (sid, e, ke),
            "  let a%d = toml.get_array(%s, %s)" % (sid, e, ke),
            "  let m%d = toml.has(%s, %s)" % (sid, e, ke),
            '  emit(%d, "{g%d}|{h%d}|{b%d}|{a%d}|{m%d}")' % (sid, sid, sid, sid, sid, sid),
        ]
        gi = want if isinstance(want, int) and not isinstance(want, bool) else 0
        gs = want if isinstance(want, str) else ""
        gb = "true" if want is True else "false"
        ga = "[" + ", ".join(str(x) for x in want) + "]" if isinstance(want, list) else "[]"
        gm = "true" if want is not None else "false"
        cases.append(Case(sid, "toml", lines, "%d|%s|%s|%s|%s" % (gi, gs, gb, ga, gm),
                          "%s  (key %s)" % (text, key)))
    return cases


INT_RE = re.compile(r"^[+-]?[0-9]+$")


def parse_int_py(s):
    if not INT_RE.match(s):
        return "!ERR"
    v = int(s)
    return "!ERR" if v < -(2**63) or v > 2**63 - 1 else str(v)


def text_cases(rng, n, start):
    cases = []
    for k in range(n):
        sid = start + k
        kind = rng.choice(["parse_int", "parse_int", "split", "trim", "replace", "capitalize",
                           "format_int", "fixed"])
        if kind == "parse_int":
            s = rng.choice(["", "0", "-0", "+7", " 7", "7 ", "007", "-9223372036854775808",
                            "9223372036854775807", "9223372036854775808", "-9223372036854775809",
                            "abc", "1a", "--1", "+-1", "-", "+", "1.5", "١٢٣", "1_000",
                            str(rng.randint(-10**18, 10**18))])
            pre = []
            e = yxexpr(s, sid, pre)
            lines = pre + ["  match text.parse_int(%s) {" % e,
                           '    Ok(v) => { emit(%d, "{v}") }' % sid,
                           '    Err(e) => { emit(%d, "!ERR") }' % sid, "  }"]
            cases.append(Case(sid, "parse_int", lines, parse_int_py(s), s))
        elif kind == "split":
            s = rng.choice(["a,b,,c", "", ",", "a", "abc", "a::b::", "x y z", "a,b,c,", "é,ü"])
            sep = rng.choice([",", ":", " ", "::", "b", "é"])
            parts = s.split(sep)
            pre = []
            e1 = yxexpr(s, sid, pre)
            e2 = yxexpr(sep, sid * 100 + 1, pre)
            lines = pre + ["  let p%d = text.split(%s, %s)" % (sid, e1, e2),
                           "  emit(%d, flat1(p%d))" % (sid, sid)]
            cases.append(Case(sid, "split", lines, "[" + ", ".join(parts) + "]", "%r by %r" % (s, sep)))
        elif kind == "trim":
            ws = " \t\n\r\x0b\x0c"
            s = ("".join(rng.choice(ws) for _ in range(rng.randint(0, 3))) +
                 "".join(rng.choice("abcé ") for _ in range(rng.randint(0, 4))) +
                 "".join(rng.choice(ws) for _ in range(rng.randint(0, 3))))
            which = rng.choice(["trim", "trim_start", "trim_end"])
            want = {"trim": s.strip(ws), "trim_start": s.lstrip(ws), "trim_end": s.rstrip(ws)}[which]
            pre = []
            e = yxexpr(s, sid, pre)
            cases.append(Case(sid, which, pre + ["  emit(%d, text.%s(%s))" % (sid, which, e)], want, s))
        elif kind == "replace":
            s = rng.choice(["aaaa", "abcabc", "", "a", "hello world", "x.y.z"])
            a = rng.choice(["a", "abc", "l", ".", ""])
            b = rng.choice(["", "Z", "aa", "-"])
            pre = []
            e0 = yxexpr(s, sid, pre)
            e1 = yxexpr(a, sid * 100 + 1, pre)
            e2 = yxexpr(b, sid * 100 + 2, pre)
            lines = pre + ["  emit(%d, text.replace(%s, %s, %s))" % (sid, e0, e1, e2)]
            cases.append(Case(sid, "replace", lines, s.replace(a, b), "%r %r -> %r" % (s, a, b)))
        elif kind == "capitalize":
            s = rng.choice(["", "a", "abc", "ABC", "1x", "éa", "aBc", "ünicode"])
            f = s[:1]
            want = ((f.upper() if f.isascii() else f) + s[1:]) if s else s
            pre = []
            e = yxexpr(s, sid, pre)
            cases.append(Case(sid, "capitalize", pre + ["  emit(%d, text.capitalize(%s))" % (sid, e)],
                              want, s))
        elif kind == "format_int":
            v = rng.choice([0, 1, -1, 255, -255, 2**63 - 1, -2**63, rng.randint(-10**9, 10**9)])
            base = rng.randint(2, 36)
            digits = "0123456789abcdefghijklmnopqrstuvwxyz"
            n2, rep = abs(v), ""
            while n2:
                rep = digits[n2 % base] + rep
                n2 //= base
            want = ("-" if v < 0 else "") + (rep or "0")
            cases.append(Case(sid, "format_int", ["  emit(%d, fmt.format_int(%d, %d))" % (sid, v, base)],
                              want, "%d base %d" % (v, base)))
        else:
            x = rng.choice([0.0, 2.5, -2.5, 1.005, 0.125, 3.14159, -0.5, 1.15, 9.999])
            d = rng.randint(0, 4)
            want = fixed_py(x, d)  # the exact binary value scaled, rounded half away from zero
            cases.append(Case(sid, "fixed", ["  emit(%d, fmt.fixed(%r, %d))" % (sid, x, d)],
                              want, "%r decimals %d" % (x, d)))
    return cases


def parse_with_py(argv, valued, with_subcommand):
    """The rule documented for `args.parse_with` (docs/STDLIB.md, src/std/args.yx)."""
    flags, options, positionals, errors = [], [], [], []
    sub, help_ = "", False
    i, positional_only = 1, False
    while i < len(argv):
        a = argv[i]
        if not positional_only and a == "--":
            positional_only = True
        elif not positional_only and a == "--help":
            help_ = True
            flags.append("help")
        elif not positional_only and a.startswith("--"):
            if "=" in a:
                k, v = a[2:].split("=", 1)
                options.append((k, v))
            else:
                k = a[2:]
                if k in valued and i + 1 < len(argv):
                    i += 1
                    options.append((k, argv[i]))
                else:
                    if k in valued:
                        errors.append("option --%s needs a value" % k)
                    flags.append(k)
        elif not positional_only and a.startswith("-") and len(a) > 1:
            j = 1
            while j < len(a):
                f = a[j]
                if f in valued:
                    if j + 1 < len(a):
                        options.append((f, a[j + 1:]))
                    elif i + 1 < len(argv):
                        i += 1
                        options.append((f, argv[i]))
                    else:
                        errors.append("option -%s needs a value" % f)
                        flags.append(f)
                    break
                flags.append(f)
                j += 1
        else:
            if with_subcommand and not sub:
                sub = a
            else:
                positionals.append(a)
        i += 1
    return flags, options, positionals, sub, help_, errors


def args_cases(rng, n, start):
    cases = []
    pool = ["--help", "--", "-", "-v", "-abc", "-n5", "-n", "5", "-ev", "pat", "--prefix", "--title=A",
            "--title", "TEXT", "-o", "FILE", "sub", "a", "b", "x=y", "--verbose", "-n=3"]
    for k in range(n):
        sid = start + k
        argv = ["prog"] + [rng.choice(pool) for _ in range(rng.randint(0, 5))]
        valued = rng.choice([[], ["n"], ["e"], ["o", "prefix"], ["title"], ["v"]])
        with_sub = rng.random() < 0.5
        flags, options, pos, sub, help_, errors = parse_with_py(argv, valued, with_sub)
        want = "%s|%s|%s|%s|%s|%s" % (
            ",".join(flags),
            ",".join("%s=%s" % kv for kv in options),
            ",".join(pos), sub, "help" if help_ else "", ";".join(errors))
        av = ["  var av%d = List<String>.new()" % sid]
        for a in argv:
            av.append("  av%d.push(%s)" % (sid, yxexpr(a, sid * 100 + len(av), av)))
        av.append("  var val%d = List<String>.new()" % sid)
        for v in valued:
            av.append("  val%d.push(%s)" % (sid, yxexpr(v, sid * 100 + len(av), av)))
        av.append("  emit(%d, summary(args.parse_with(av%d, val%d, %s)))"
                  % (sid, sid, sid, "true" if with_sub else "false"))
        cases.append(Case(sid, "args", av, want, "%r valued=%r subcommand=%s" % (argv, valued, with_sub)))
    return cases



DIGITS_L = "0123456789abcdefghijklmnopqrstuvwxyz"
DIGITS_U = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"


def base_digits(v, base, upper=False):
    ds = DIGITS_U if upper else DIGITS_L
    n, rep = abs(v), ""
    while n:
        rep = ds[n % base] + rep
        n //= base
    return ("-" if v < 0 else "") + (rep or "0")


def fmt_cases(rng, n, start):
    cases = []
    for k in range(n):
        sid = start + k
        kind = rng.choice(["format_int", "format_int_upper", "bits", "pad", "center", "zero_pad",
                           "hex_padded", "precision", "width", "group", "fixed", "percent",
                           "bytes_human", "fixed_width"])
        v = rng.choice([0, 1, -1, 7, -7, 255, -255, 1000, 1234567, -1234567, 2**63 - 1, -2**63,
                        rng.randint(-10**12, 10**12)])
        base = rng.randint(2, 36)
        width = rng.randint(0, 10)
        fill = rng.choice(["0", "*", " ", "."])
        s = rng.choice(["", "a", "abc", "hello", "é漢", "x" * 12])
        if kind == "format_int":
            want = base_digits(v, base)
            code = "  emit(%d, fmt.format_int(%d, %d))" % (sid, v, base)
        elif kind == "format_int_upper":
            want = base_digits(v, base, True)
            code = "  emit(%d, fmt.format_int_upper(%d, %d))" % (sid, v, base)
        elif kind == "bits":
            want = base_digits(v if v >= 0 else v + 2**64, base)
            code = "  emit(%d, fmt.format_bits(%d, %d))" % (sid, v, base)
        elif kind == "pad":
            which = rng.choice(["pad_left", "pad_right"])
            want = (s.rjust(width, fill) if which == "pad_left" else s.ljust(width, fill))
            code = "  emit(%d, fmt.%s(%s, %d, '%s'))" % (sid, which, yxexpr_s(s, sid, rng), width, fill)
            if len(s) >= width:
                want = s
            else:
                want = (fill * (width - len(s)) + s) if which == "pad_left" else (s + fill * (width - len(s)))
            # rjust/ljust give the same answer; compute explicitly to be sure about the odd cases
        elif kind == "center":
            if len(s) >= width:
                want = s
            else:
                left = (width - len(s)) // 2
                want = fill * left + s + fill * (width - len(s) - left)
            code = "  emit(%d, fmt.center(%s, %d, '%s'))" % (sid, yxexpr_s(s, sid, rng), width, fill)
        elif kind == "zero_pad":
            mag = base_digits(abs(v), 10)
            want = ("-" if v < 0 else "") + mag.rjust(width, "0")
            code = "  emit(%d, fmt.zero_pad(%d, %d))" % (sid, v, width)
        elif kind == "hex_padded":
            want = base_digits(v, 16).rjust(width, "0") if v >= 0 else base_digits(v, 16)
            code = "  emit(%d, fmt.hex_padded(%d, %d))" % (sid, v, width)
        elif kind == "precision":
            prec = rng.randint(0, 8)
            mag = base_digits(abs(v), base)
            body = mag.rjust(prec, "0")
            if v == 0 and prec == 0:
                body = ""
            want = ("-" if v < 0 else "") + body
            code = "  emit(%d, fmt.format_int_precision(%d, %d, %d))" % (sid, v, base, prec)
        elif kind == "width":
            left = rng.random() < 0.5
            body = base_digits(v, base)
            want = (body + fill * max(0, width - len(body))) if left else (fill * max(0, width - len(body)) + body)
            code = "  emit(%d, fmt.format_int_width(%d, %d, %d, '%s', %s))" % (sid, v, base, width, fill,
                                                                              "true" if left else "false")
        elif kind == "group":
            sep = rng.choice([",", "_", " "])
            mag = base_digits(abs(v), 10)
            out = []
            for i, ch in enumerate(mag):
                if i and (len(mag) - i) % 3 == 0:
                    out.append(sep)
                out.append(ch)
            want = ("-" if v < 0 else "") + "".join(out)
            code = "  emit(%d, fmt.group(%d, \"%s\"))" % (sid, v, sep)
        elif kind == "fixed":
            x = rng.choice([0.0, -0.0, 2.5, -2.5, 1.005, 0.125, 3.14159, -0.5, 1.15, 9.999, 1e10, 1e-7,
                            rng.uniform(-1000, 1000)])
            d = rng.randint(0, 9)
            want = fixed_py(x, d)
            code = "  emit(%d, fmt.fixed(%r, %d))" % (sid, x, d)
        elif kind == "percent":
            x = rng.choice([0.0, 0.256, 0.5, 1.0, -0.5, 0.125, 0.3333, rng.uniform(-2, 2)])
            d = rng.randint(0, 4)
            want = fixed_py(x * 100.0, d) + "%"
            code = "  emit(%d, fmt.percent(%r, %d))" % (sid, x, d)
        elif kind == "bytes_human":
            nb = rng.choice([0, 1, 1023, 1024, 1536, 1 << 20, 3 * 1024 * 1024, 1 << 30, rng.randint(0, 10**12)])
            want = bytes_human_py(nb)
            code = "  emit(%d, fmt.bytes_human(%d))" % (sid, nb)
        else:
            x = rng.choice([0.0, 2.5, -1.25, 3.14159, 1e6, rng.uniform(-100, 100)])
            d = rng.randint(0, 6)
            left = rng.random() < 0.5
            body = fixed_py(x, d)
            want = (body + fill * max(0, width - len(body))) if left else (fill * max(0, width - len(body)) + body)
            code = "  emit(%d, fmt.fixed_width(%r, %d, %d, '%s', %s))" % (sid, x, d, width, fill,
                                                                           "true" if left else "false")
        cases.append(Case(sid, "fmt", [code], want, "%s %d %d %s %r" % (kind, v, base, fill, s)))
    return cases


def yxexpr_s(s, sid, rng):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def fixed_py(x, d):
    if x != x:
        return "nan"
    if x == float("inf"):
        return "inf"
    if x == float("-inf"):
        return "-inf"
    v = Decimal(x).scaleb(d).quantize(Decimal(1), rounding=ROUND_HALF_UP).scaleb(-d)
    sign = "-" if v < 0 else ""
    body = format(abs(v), "f")
    return sign + body


def bytes_human_py(n):
    if n < 1024:
        return "%d B" % n
    v = float(n)
    unit = 0
    while v >= 1024.0 and unit < 6:
        v = v / 1024.0
        unit += 1
    return "%.1f %sB" % (v, ["Ki", "Mi", "Gi", "Ti", "Pi", "Ei"][unit - 1])


GENERATORS = {"json": gen_json, "csv": csv_cases, "toml": toml_cases, "text": text_cases,
              "args": args_cases, "fmt": fmt_cases}


# ───────────────────────── running and comparing ─────────────────────────

def run_source(source, backend, tmpdir, name):
    src = os.path.join(tmpdir, name + ".yx")
    exe = os.path.join(tmpdir, name)
    open(src, "w").write(source)
    flags = ["--native"] if backend == "native" else []
    r = subprocess.run([YERUX, "build", src] + flags + ["-o", exe], capture_output=True, text=True)
    if r.returncode != 0:
        return None, r.stderr, src
    p = subprocess.run([exe], capture_output=True, text=True, timeout=120)
    return (p.returncode, p.stdout, p.stderr), None, src


def program_for(cases):
    body = []
    for c in cases:
        body.extend(c.lines)
    return HEADER + "\n".join(body) + "\n" + TAIL


def parse_out(text):
    got = {}
    for line in text.split("\n"):
        if "\t" in line:
            k, v = line.split("\t", 1)
            try:
                got[int(k)] = v
            except ValueError:
                pass
    return got


def same(case, expected, actual):
    if case.suite == "json":
        pass
    else:
        return esc_py(expected) == actual
    if expected == "!ERR" or actual == "!ERR":
        return expected == actual
    try:
        a = json.loads(unesc(actual), parse_float=Decimal, parse_int=int)
        b = json.loads(expected, parse_float=Decimal, parse_int=int)
    except Exception:
        return False
    return a == b and type(a) == type(b)


def run_batch(batch, backend, outdir, idx):
    source = program_for(batch)
    with tempfile.TemporaryDirectory() as tmp:
        res, err, src = run_source(source, backend, tmp, "batch%04d" % idx)
        if err is not None or res[0] != 0:
            # a crash or a compile error: attribute it to a single case
            bad = []
            for c in batch:
                with tempfile.TemporaryDirectory() as tmp2:
                    r2, e2, s2 = run_source(program_for([c]), backend, tmp2, "one%05d" % c.sid)
                    if e2 is not None or r2[0] != 0:
                        keep = os.path.join(outdir, "case%05d_%s.yx" % (c.sid, c.suite))
                        open(keep, "w").write(program_for([c]))
                        msg = (e2 or (r2[2] or ""))[:400]
                        bad.append((c, "CRASH" if e2 is None else "COMPILE", msg, keep))
                    else:
                        got = parse_out(r2[1]).get(c.sid)
                        if got is None or not same(c, c.expect, got):
                            keep = os.path.join(outdir, "case%05d_%s.yx" % (c.sid, c.suite))
                            open(keep, "w").write(program_for([c]))
                            bad.append((c, "MISMATCH", "expected %r got %r" % (c.expect, got), keep))
            return bad
        got = parse_out(res[1])
        bad = []
        for c in batch:
            g = got.get(c.sid)
            if g is None or not same(c, c.expect, g):
                bad.append((c, "MISMATCH", "expected %r got %r" % (c.expect, g), None))
        return bad


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cases", type=int, default=2000)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--suite", default="all")
    ap.add_argument("--batch", type=int, default=40)
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--backend", default="native", choices=["native", "c"])
    ap.add_argument("--out", default="/tmp/yerux_fuzz_stdlib")
    ap.add_argument("--verbose", action="store_true")
    a = ap.parse_args()

    suites = list(GENERATORS) if a.suite == "all" else [s.strip() for s in a.suite.split(",")]
    rng = random.Random(a.seed)
    per = max(1, a.cases // len(suites))
    all_cases, sid = [], 1
    for s in suites:
        made = GENERATORS[s](rng, per, sid)
        sid += 10 * per
        all_cases.extend(made)
    os.makedirs(a.out, exist_ok=True)
    batches = [(all_cases[i:i + a.batch], a.backend, a.out, i // a.batch)
               for i in range(0, len(all_cases), a.batch)]
    found = []
    with ThreadPoolExecutor(max_workers=a.jobs) as ex:
        for res in ex.map(lambda t: run_batch(*t), batches):
            found.extend(res)
    by_suite = {}
    for c, kind, msg, keep in found:
        by_suite.setdefault(c.suite, []).append((c, kind, msg, keep))
    for suite, items in sorted(by_suite.items()):
        print("== %s: %d discrepanc%s" % (suite, len(items), "y" if len(items) == 1 else "ies"))
        for c, kind, msg, keep in items[:12]:
            print("  [%s] %s" % (kind, c.src if len(c.src) < 300 else c.src[:300] + "..."))
            print("     %s" % msg)
            if keep:
                print("     %s" % keep)
        if len(items) > 12:
            print("  ... %d more" % (len(items) - 12))
    print("done %d cases (%s), %d discrepancies" % (len(all_cases), ",".join(suites), len(found)))
    return 0 if not found else 1


if __name__ == "__main__":
    sys.exit(main())
