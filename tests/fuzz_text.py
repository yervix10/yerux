#!/usr/bin/env python3
"""Differential stdin tests for the stdlib's text readers with seeded Python models."""
import argparse
import base64
import binascii
import json
import os
from pathlib import Path
import random
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
YERUX = ROOT / "target/release/yerux"
HARNESS = ROOT / "tests/fuzz/text_input.yx"
TAGS = "jthxb"
WORD = "a Zé€中😀#\"\\\r\n\t"


def word(rng):
    return "".join(rng.choices(WORD, k=rng.randrange(10)))


def json_case(rng, i):
    def value(depth):
        if depth and rng.randrange(2):
            if rng.randrange(2):
                return [value(depth - 1) for _ in range(rng.randrange(5))]
            return {word(rng): value(depth - 1) for _ in range(rng.randrange(5))}
        return rng.choice([None, True, False, word(rng) + "\0",
                           rng.randrange(-(2**63), 2**63), rng.randrange(-5000, 5001) / 8])

    if i % 3:
        v = value(3)
        return json.dumps(v, ensure_ascii=bool(i % 2)).encode(), v, True
    fragment = rng.choice([b"NaN", b"01", b"1e+", b"falsex", b"1.", b"[,]", b"{",
                           b'"\\ud800"', b'"\\udc00"', b'"a\0b"', b'"\\u00xz"', b"1x",
                           b'"\xc0\xaf"', b'"\xff"', b'"\xe2\x82"', b'{"\xed\xa0\x80":0}',
                           b'"\xff\\q"', b"1e999", b"-1e999", b"9223372036854775808"])
    return b" " * rng.randrange(5) + fragment, "error", False


def toml_case(rng, i):
    name = word(rng).replace("\r", "")
    items = [word(rng).replace("\r", "") for _ in range(rng.randrange(5))]
    n = rng.randrange(-10000, 10001)
    flag = rng.choice(["true", "false"])
    quote = lambda s: json.dumps(s, ensure_ascii=False)
    layout = rng.choice([", ", ",\n # comment\n "])
    valid = f'[section]\nname = {quote(name)} # comment\nn = {n}\nflag = {flag}\nitems = [{layout.join(map(quote, items))}]\n'
    if i % 3:
        expected = ":".join([name.encode().hex(), str(n), flag, *(s.encode().hex() for s in items)])
        return valid.encode(), expected, False
    # All have a malformed header or malformed UTF-8 before the requested keys.
    broken = rng.choice([b"[", b"[\xc3\xa9", b"[section", b"[section]\nname=\"\xff\"",
                         b"[section]\n#\xe2\x82", b"[section]\nitems=[\xed\xa0\x80]"])
    return broken + b"\nname=\"unreachable\"\n", ":0:false", False


def percent(s):
    return "".join(f"%{b:02X}" for b in s.encode())


def http_case(rng, i):
    name = rng.choice(["a b", "é", "/", "€", "x+y", "😀"])
    query = rng.choice(["€", "&=", "a b", "+", "é"])
    target = f"/item/{percent(name)}?k={percent(query)}"
    base = f"POST {target} HTTP/1.1\r\nHost: h\r\n".encode()
    payload = rng.randbytes(rng.randrange(1, 33))
    mode = i % 14
    if mode == 0:
        msg = base + f"Content-Length: {len(payload)}\r\n\r\n".encode() + payload
    elif mode == 1:
        split = rng.randrange(1, len(payload) + 1)
        parts = (payload[:split], payload[split:])
        msg = base + b"Transfer-Encoding: chunked\r\n\r\n"
        for part in parts:
            if part:
                msg += f"{len(part):x};ext=1\r\n".encode() + part + b"\r\n"
        msg += b"0\r\nX-End: yes\r\n\r\n"
    else:
        cases = {
            2: (b"\r\n", "ok"),
            3: (b"Content-Length: 33\r\n\r\n", "413"),
            4: (b"Content-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n", "400"),
            5: (b"Content-Length: 1\r\nContent-Length: 2\r\n\r\n00", "400"),
            6: (b"Transfer-Encoding: chunked\r\n\r\n1\r\nx!\n0\r\n\r\n", "400"),
            7: (b"Transfer-Encoding: chunked\r\n\r\n21\r\n", "413"),
            8: (b" Host: folded\r\n\r\n", "400"),
            9: (b"X: 0\r\n" * 5 + b"\r\n", "431"),
            10: (b"Content-Length: 1\r\n\r\n", "need"),
            11: (b"Transfer-Encoding: gzip\r\n\r\n", "501"),
            12: (b"\r\n", "400"),
            13: (b"\r\n", "414"),
        }
        suffix, expected = cases[mode]
        if mode == 12:
            bad = rng.choice([b"%FF", b"%C3", b"%ED%A0%80", b"%00", b"%zz"])
            base = rng.choice([b"GET /item/" + bad, b"GET /item/x?k=" + bad]) + b" HTTP/1.1\r\nHost: h\r\n"
        if mode == 13:
            base = b"GET /item/" + b"x" * rng.randrange(64, 100) + b" HTTP/1.1\r\nHost: h\r\n"
        msg = base + suffix
        if expected != "ok":
            return msg, expected, False
        payload = b""
    expected = f"ok:{('/item/' + name).encode().hex()}:{name.encode().hex()}:{query.encode().hex()}:{payload.hex()}:{len(msg)}"
    if rng.randrange(2):
        msg += b"GET /next HTTP/1.1\r\nHost: h\r\n\r\n"
    return msg, expected, False


def binary_case(rng, i, kind):
    b64 = kind == "b"
    raw = rng.randbytes(rng.randrange(50)) if i % 2 else word(rng).encode()
    data = base64.b64encode(raw) if b64 else (raw.hex().upper() if i % 4 else raw.hex()).encode()
    if i % 3 == 0:
        at = rng.randrange(len(data) + 1)
        data = data[:at] + rng.choice([b"", b"=", b"\0", b"f", b"\xff", b"!", b" "]) + data[at + 1:]
    if b64:
        try:
            raw = base64.b64decode(data, validate=True)
        except binascii.Error:
            return data, "error/error", False
        if base64.b64encode(raw) != data:
            return data, "error/error", False
    else:
        if re.fullmatch(rb"[0-9a-fA-F]*", data) is None or len(data) % 2:
            return data, "error/error", False
        raw = bytes.fromhex(data.decode())
    answer = "ok:" + raw.hex()
    try:
        raw.decode("utf-8")
        return data, f"{answer}/{answer}", False
    except UnicodeDecodeError:
        return data, f"{answer}/error", False


def check(exe, cases, timeout):
    def record(case):
        tag, data, _, _ = case
        return (tag + data.hex() + "\n").encode("ascii")

    def run(batch):
        return subprocess.run([exe], input=b"".join(map(record, batch)), capture_output=True,
                              env=dict(os.environ, ASAN_OPTIONS="detect_leaks=1:halt_on_error=1"),
                              timeout=timeout)

    def locate_crash(batch):
        if len(batch) == 1:
            return batch[0]
        mid = len(batch) // 2
        for chunk in (batch[:mid], batch[mid:]):
            try:
                reply = run(chunk)
                bad = reply.returncode != 0 or len(reply.stdout.splitlines()) != len(chunk)
            except subprocess.TimeoutExpired:
                bad = True
            if bad:
                return locate_crash(chunk)
        return None  # The failure requires a longer input or shared state.

    try:
        reply = run(cases)
        if reply.returncode or len(reply.stdout.splitlines()) != len(cases):
            raise ValueError(f"exit {reply.returncode}; {reply.stderr.decode(errors='replace')[:500]}")
    except (subprocess.TimeoutExpired, ValueError) as e:
        case = locate_crash(cases)
        print(f"{exe.name}: {e}; single-case stdin: {record(case) if case else 'none'}")
        return None
    rows = reply.stdout.decode("ascii").splitlines()
    errors = []
    for i, ((tag, data, expected, json_value), row) in enumerate(zip(cases, rows)):
        try:
            actual = json.loads(bytes.fromhex(row[3:])) if json_value and row.startswith("ok:") else row
            matches = (row.startswith("ok:") and
                       json.dumps(actual, sort_keys=True) == json.dumps(expected, sort_keys=True)
                       if json_value else actual == expected)
        except (UnicodeError, ValueError):
            matches = False
        if not matches:
            errors.append((i, record(cases[i]), expected, row))
    for i, fixture, expected, row in errors[:8]:
        print(f"{exe.name} case {i}: {fixture!r} expected {expected!r}, got {row!r}")
    print(f"{exe.name}: {len(rows)} cases, {len(errors)} model failures")
    return rows, len(errors)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--cases", type=int, default=250, help="cases per selected surface")
    ap.add_argument("--seed", type=int, default=539108)
    ap.add_argument("--only", choices=tuple(TAGS))
    ap.add_argument("--asan", action="store_true")
    ap.add_argument("--timeout", type=float, default=45.0)
    args = ap.parse_args()
    rng = random.Random(args.seed)
    generators = dict(j=json_case, t=toml_case, h=http_case,
                      x=lambda r, i: binary_case(r, i, "x"),
                      b=lambda r, i: binary_case(r, i, "b"))
    cases = [(tag, *generators[tag](rng, i))
             for i in range(args.cases) for tag in args.only or TAGS]
    print(f"text fuzz: seed {args.seed}, surfaces {args.only or TAGS}, {args.cases} cases each", flush=True)
    with tempfile.TemporaryDirectory(prefix="yerux_textfuzz_") as directory:
        folder = Path(directory)
        results = {}
        for mode in ("c", "native", "asan") if args.asan else ("c", "native"):
            exe = folder / mode
            if mode == "asan":
                source = folder / "harness.c"
                with source.open("wb") as out:
                    subprocess.run([YERUX, "c", HARNESS], stdout=out, check=True)
                subprocess.run(["cc", "-g", "-O1", "-w", "-fsanitize=address,undefined",
                                "-fno-sanitize-recover=undefined", "-pthread", "-no-pie",
                                source, "-lm", "-o", exe], check=True)
            else:
                flags = ["--native"] if mode == "native" else []
                subprocess.run([YERUX, "build", HARNESS, *flags, "-o", exe], check=True)
            results[mode] = check(exe, cases, args.timeout)

    if any(result is None for result in results.values()):
        return 1
    diff = sum(any(rows[i] != results["c"][0][i] for rows, _ in results.values())
               for i in range(len(cases)))
    print(f"backend/sanitizer differences: {diff}")
    return bool(diff or any(failures for _, failures in results.values()))


if __name__ == "__main__":
    raise SystemExit(main())
