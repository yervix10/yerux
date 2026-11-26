#!/usr/bin/env python3
"""Generate small, typed Yerux programs and compare the C and native runners."""

import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from functools import partial
import json
import os
from pathlib import Path
import random
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[2]
I_MIN = -(1 << 63)
I_MAX = (1 << 63) - 1
UNICODE = ("a", "Z", "é", "e\u0301", "界", "٣", "🌍", "\U0010ffff", "\n", "\t", " ", '"')


def literal(text):
    return json.dumps(text, ensure_ascii=False)


def sample_text(rng):
    return "".join(rng.choices(UNICODE, k=rng.randrange(9)))


def program(body, declarations=""):
    return f'{declarations}\nfn main() {{\n  print("running")\n{body}\n}}\n'


def signed(rng):
    edges = (I_MIN, I_MIN + 1, -3037000500, -129, -1, 0, 1, 127, 3037000500, I_MAX - 1, I_MAX)
    a = rng.choice(edges) + rng.randrange(-2, 3)
    a = min(max(a, I_MIN), I_MAX)
    b = rng.choice(edges) if rng.randrange(3) else rng.randrange(-1000, 1001)
    op = rng.choice(("+", "-", "*", "/", "%", "<=", ">=", "=="))
    wrap = rng.choice(("add", "sub", "mul"))
    left = f"seed({a})" if rng.randrange(2) else str(a)
    right = f"seed({b})" if rng.randrange(2) else str(b)
    return program(f"""  let a = {left}
  let b = {right}
  print(a.wrapping_{wrap}(b))
  print(a.truncate_int8())
  print(a {op} b)""", "fn seed(n: Int) -> Int { return n }")


def unsigned(rng):
    bits = rng.choice((8, 16, 32, 64))
    ceiling = (1 << bits) - 1
    a = rng.choice((0, 1, ceiling, ceiling - 1, 1 << (bits - 1), rng.randrange(ceiling + 1)))
    b = rng.choice((0, 1, ceiling, rng.randrange(ceiling + 1)))
    op = rng.choice(("+", "-", "*", "/", "%", "<=", ">=", "=="))
    shift = rng.randrange(bits)
    wrap = rng.choice(("add", "sub", "mul"))
    return program(f"""  let a = seed({a})
  let b = seed({b})
  let power: UInt{bits} = {1 << shift}
  print("{shift}")
  print(a / power)
  print(a.wrapping_mul(power))
  print(a.wrapping_{wrap}(b))
  print(a {op} b)""", f"fn seed(n: UInt{bits}) -> UInt{bits} {{ return n }}")


def strings(rng):
    fragments = [sample_text(rng) for _ in range(rng.randrange(2, 7))]
    lines = "\n".join(f"  s.push_str({literal(text)})" for text in fragments)
    at = rng.randrange(len("".join(fragments)) + 1)
    pattern = literal(rng.choice(UNICODE))
    replacement = literal(sample_text(rng))
    return program(f"""  var s = String.new()
{lines}
  let chars = text.chars(s)
  print(s.len())
  print(s.char_count())
  print(text.from_chars(chars))
  let offsets = text.char_offsets(s)
  if {at} < offsets.len() {{
    let byte = offsets.at({at})
    print(s.char_at(byte))
    print(s.slice(0, byte))
  }}
  print(s.replace({pattern}, {replacement}))
  print(s.find({pattern}))""", "import std.text")


def lists(rng):
    values = [sample_text(rng) for _ in range(rng.randrange(1, 10))]
    pushes = "\n".join(f"  xs.push(String.from({literal(text)}))" for text in values)
    i, j = rng.choices(range(len(values)), k=2)
    return program(f"""  var xs = List<String>.new()
{pushes}
  xs.swap(mark({i}), mark({j}))
  let removed = xs.remove(mark({i}))
  xs.insert(mark({j}), move removed)
  let copy = xs.clone()
  xs.set(mark({i}), String.from({literal(sample_text(rng))}))
  for s in copy {{ print(s) }}
  for s in xs {{ print(s) }}""", "fn mark(n: Int) -> Int { print(n) return n }")


def maps(rng):
    # Keys share a prefix, may repeat, and deliberately cross the first growth threshold.
    size = rng.randrange(4, 13)
    keys = [rng.randrange(size) for _ in range(size * 2)]
    insertions = "\n".join(f'  m.insert(String.from("界{key}"), String.from({literal(sample_text(rng))}))' for key in keys)
    removed = rng.choice(keys)
    return program(f"""  var m = Map<String, String>.new()
{insertions}
  let copy = m.clone()
  let old = m.take("界{removed}")
  print(old)
  m.insert(String.from("界{removed}"), String.from({literal(sample_text(rng))}))
  print(m.len())
  for i in 0..{size} {{
    let key = "界{{i}}"
    if m.contains(key) {{
      print(m.vals.at(m.find(key)))
      print(copy.vals.at(copy.find(key)))
    }}
  }}""")


def variants(rng):
    initial = rng.randrange(-2000, 2001)
    limit = rng.randrange(-2000, 2001)
    commands = []
    for _ in range(rng.randrange(3, 10)):
        op = rng.choice(("Text", "Number", "Skip"))
        constructor = {
            "Text": f"Op.Text(String.from({literal(sample_text(rng))}))",
            "Number": f"Op.Number({rng.randrange(-500, 501)})",
            "Skip": "Op.Skip",
        }[op]
        commands.append(f"  state = apply(move state, {constructor})\n  print(show(state))")
    trace = "\n".join(commands)
    return program(f"""  var state: Result<Option<Counter>, String> = Ok(Some(Counter {{
    value: {initial}, label: String.from({literal(sample_text(rng))}),
  }}))
{trace}""", f"""struct Counter {{ value: Int, label: String }}
enum Op {{ Text(String), Number(Int), Skip }}

fn apply(state: Result<Option<Counter>, String>, op: Op) -> Result<Option<Counter>, String> {{
  match move state {{
    Ok(value) => {{
      match move value {{
        Some(c) => {{
          var counter = move c
          match move op {{
            Text(s) => {{ counter.label.push_str(s) }}
            Number(n) => {{ counter.value = counter.value + n }}
            Skip => {{ return Ok(None) }}
          }}
          if counter.value < {limit} {{ return Err(String.from(counter.label)) }}
          return Ok(Some(move counter))
        }}
        None => {{ return Ok(None) }}
      }}
    }}
    Err(s) => {{ return Err(move s) }}
  }}
}}

fn show(state: view Result<Option<Counter>, String>) -> String {{
  match state {{
    Ok(Some(c)) => {{ return "ok {{c.value}} {{c.label}}" }}
    Ok(None) => {{ return String.from("empty") }}
    Err(s) => {{ return "error {{s}}" }}
  }}
}}""")


def closures(rng):
    alpha, beta, x, y = (rng.randrange(-500, 501) for _ in range(4))
    captured = literal(sample_text(rng))
    left = rng.choice(("a + x", "a - x", "a * x"))
    right = rng.choice(("b + y", "b - y", "b * y"))
    return program(f"""  let a = {alpha}
  let b = {beta}
  let text = String.from({captured})
  let first = invoke(|x: Int, y: Int| {{ return ({left}) + ({right}) + text.len() }}, {x}, {y})
  let stored = save(move text, first)
  match stored {{
    Value(f) => {{
      print(invoke(f, {y}, {x}))
      print(invoke(f, first, {y}))
    }}
  }}""", """enum Callback { Value(fn(Int, Int) -> Int) }

fn save(text: String, bias: Int) -> Callback {
  return Callback.Value(move |x: Int, y: Int| x - y + bias + text.len())
}

fn mark(n: Int) -> Int { print(n) return n }
fn invoke(f: view fn(Int, Int) -> Int, a: Int, b: Int) -> Int {
  return f(mark(a), mark(b))
}""")


GENERATORS = {generate.__name__: generate for generate in
              (signed, unsigned, strings, lists, maps, variants, closures)}


def run(binary, file, mode, args, timeout):
    command = [binary, "run", file, *args]
    if mode == "native":
        command.append("--native")
    try:
        result = subprocess.run(
            command, stdin=subprocess.DEVNULL, capture_output=True, timeout=timeout,
            cwd=file.parent, env=dict(os.environ, TMPDIR=str(file.parent)),
        )
        return result.returncode, result.stdout, result.stderr
    except subprocess.TimeoutExpired as error:
        return None, error.stdout or b"", error.stderr or b""


def compare(binary, output, args, timeout, case):
    seed, profile = case
    source = GENERATORS[profile](random.Random(seed))
    with tempfile.TemporaryDirectory(prefix="yerux-backends-") as directory:
        file = Path(directory) / "case.yx"
        file.write_text(source, encoding="utf-8")
        c = run(binary, file, "c", args, timeout)
        native = run(binary, file, "native", args, timeout)
    if None in (c[0], native[0]):
        kind = "timeout"
    elif b"no C compiler found" in c[2]:
        kind = "no-c-compiler"
    elif c[:2] != native[:2]:
        kind = "difference"
    elif c[0] not in (0, 101) or not c[1].startswith(b"running\n"):
        kind = "unexpected-exit"
    else:
        return profile, "match"
    saved = output / f"{profile}-{seed}"
    saved.mkdir()
    (saved / "case.yx").write_text(source, encoding="utf-8")
    for label, (code, stdout, stderr) in (("c", c), ("native", native)):
        (saved / f"{label}.out").write_bytes(stdout)
        (saved / f"{label}.err").write_bytes(stderr)
        (saved / f"{label}.code").write_text(f"{code}\n", encoding="utf-8")
    print(f"{kind}: {saved} (C={c[0]}, native={native[0]})", flush=True)
    return profile, kind


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--yerux", type=Path, default=os.environ.get("YERUX", ROOT / "target/release/yerux"))
    parser.add_argument("--seed", type=int, default=0, help="first case seed")
    parser.add_argument("--count", type=int, default=2000)
    parser.add_argument("--jobs", "-j", type=int, default=4)
    parser.add_argument("--timeout", type=float, default=30, help="per backend and case, in seconds")
    parser.add_argument("--profile", choices=GENERATORS, help="run only one generator")
    parser.add_argument("--no-opt", action="store_true", help="also pass --no-opt to yerux run")
    parser.add_argument("--out", type=Path, required=True, help="new directory for failed cases")
    options = parser.parse_args()
    if min(options.count, options.jobs, options.timeout) <= 0:
        parser.error("count, jobs and timeout must be positive")
    binary = Path(options.yerux).resolve()
    if not os.access(binary, os.X_OK):
        parser.error(f"compiler is not executable: {binary}")
    options.out.mkdir(parents=True, exist_ok=False)
    profiles = (options.profile,) if options.profile else tuple(GENERATORS)
    cases = ((seed, profiles[seed % len(profiles)])
             for seed in range(options.seed, options.seed + options.count))
    args = ("--no-opt",) if options.no_opt else ()
    check = partial(compare, binary, options.out, args, options.timeout)
    counts = {profile: Counter() for profile in profiles}
    print(f"seed={options.seed} cases={options.count} profiles={','.join(profiles)}", flush=True)
    with ThreadPoolExecutor(max_workers=options.jobs) as pool:
        for profile, kind in pool.map(check, cases):
            counts[profile][kind] += 1
    for profile, results in counts.items():
        print(f"{profile}: {dict(sorted(results.items()))}")
    matched = sum(results["match"] for results in counts.values())
    print(f"cases={options.count} matched={matched} flagged={options.count - matched}")
    return int(matched != options.count)


if __name__ == "__main__":
    raise SystemExit(main())
