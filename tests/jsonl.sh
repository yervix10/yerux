#!/usr/bin/env bash
# examples/real/jsonl: unit tests on both backends; fixed input with exact output and exit statuses on both backends;
# a seeded NDJSON corpus whose lines get the same validity verdict from examples/real/json --check (per line, both
# backends); canonical output is a fixed point (running jsonl on its own output changes nothing). Exit 0 when all pass.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
D=examples/real/jsonl
fail=0
for flag in "" --native; do
  "$Y" test $D/tests.yx $flag >"$T/t.out" 2>&1 || { echo "FAIL jsonl unit tests ($flag)"; tail -5 "$T/t.out"; fail=1; }
done
"$Y" build $D/main.yx -o "$T/jsonl-c" >"$T/b1.out" 2>&1 || { echo "FAIL jsonl build (c)"; cat "$T/b1.out"; fail=1; }
"$Y" build $D/main.yx --native -o "$T/jsonl-n" >"$T/b2.out" 2>&1 || { echo "FAIL jsonl build (native)"; cat "$T/b2.out"; fail=1; }
"$Y" build examples/real/json/json.yx -o "$T/json-c" >"$T/b3.out" 2>&1 || { echo "FAIL json build"; fail=1; }
[ $fail = 0 ] || exit 1

# Fixed input: exact output and stderr, exit 1 because of the invalid lines.
printf '%s\n' '{"b": 1.50, "a": [true, null, "x\u00e9\n"], "c": {"z":-0,"y":1e+3}}' '' '   ' '[1,2' \
  '{"k":1,"k":2}' '"\ud800"' '  42  ' '01' 'tru' '{"a" : [ ] , "b":{}}' >"$T/in.jsonl"
cat >"$T/want.out" <<'OUT'
{"a":[true,null,"xé\n"],"b":1.50,"c":{"y":1e+3,"z":-0}}
42
{"a":[],"b":{}}
OUT
cat >"$T/want.err" <<ERR
$T/in.jsonl:4:5: expected ',' or ']' in array
$T/in.jsonl:5:8: duplicate key
$T/in.jsonl:6:8: unpaired surrogate
$T/in.jsonl:8:2: trailing characters after the value
$T/in.jsonl:9:1: invalid literal
ERR
for b in c n; do
  "$T/jsonl-$b" "$T/in.jsonl" >"$T/out.$b" 2>"$T/err.$b"; st=$?
  [ $st = 1 ] || { echo "FAIL jsonl fixed input status ($b): $st"; fail=1; }
  cmp -s "$T/want.out" "$T/out.$b" || { echo "FAIL jsonl fixed output ($b)"; diff "$T/want.out" "$T/out.$b"; fail=1; }
  cmp -s "$T/want.err" "$T/err.$b" || { echo "FAIL jsonl fixed diagnostics ($b)"; diff "$T/want.err" "$T/err.$b"; fail=1; }
done

# Seeded corpus: valid documents with unique keys, some corrupted by one edit. Lines are checked one at a time.
python3 - "$T/corpus.jsonl" 300 <<'PY'
import json, random, sys
rng = random.Random(20241)
def value(depth):
    k = rng.randrange(8 if depth < 3 else 5)
    if k == 0: return None
    if k == 1: return rng.random() < 0.5
    if k == 2: return rng.randrange(-10**6, 10**6)
    if k == 3: return rng.choice(["", "a b", "tab\there", "quote\"s", "back\\slash", "caf\u00e9", "emoji \U0001F600", "\u0001"])
    if k == 4: return round(rng.uniform(-1e3, 1e3), 3)
    if k == 5: return [value(depth + 1) for _ in range(rng.randrange(4))]
    return {f"key{i}": value(depth + 1) for i in range(rng.randrange(4))}
lines = []
for _ in range(300):
    text = json.dumps(value(0), ensure_ascii=rng.random() < 0.5, indent=rng.choice([None, 2]))
    text = text.replace("\n", " ")
    if rng.random() < 0.3 and len(text) > 1:
        i = rng.randrange(len(text))
        text = text[:i] + rng.choice(list("{}[],:\"x1-. ")) + text[i + 1:]
    lines.append(text)
open(sys.argv[1], "w", encoding="utf-8").write("\n".join(lines) + "\n")
PY
[ -s "$T/corpus.jsonl" ] || { echo "FAIL jsonl corpus generation"; exit 1; }
n=0; mism=0; valid=0
while IFS= read -r line; do
  n=$((n + 1))
  printf '%s' "$line" >"$T/one.json"
  "$T/json-c" --check "$T/one.json" >/dev/null 2>&1; ref=$?
  printf '%s\n' "$line" >"$T/one.jsonl"
  "$T/jsonl-c" --check "$T/one.jsonl" >/dev/null 2>&1; mine=$?
  "$T/jsonl-n" --check "$T/one.jsonl" >/dev/null 2>&1; nat=$?
  [ "$ref" = 0 ] && valid=$((valid + 1))
  if [ "$ref" != "$mine" ] || [ "$mine" != "$nat" ]; then
    mism=$((mism + 1)); [ $mism -le 3 ] && echo "jsonl verdict differs on line $n: json=$ref jsonl(c)=$mine jsonl(native)=$nat: $line"
  fi
done <"$T/corpus.jsonl"
[ $mism = 0 ] || { echo "FAIL jsonl differs from examples/real/json on $mism of $n lines"; fail=1; }
"$T/jsonl-c" "$T/corpus.jsonl" >"$T/c1.out" 2>/dev/null; sc=$?
"$T/jsonl-n" "$T/corpus.jsonl" >"$T/n1.out" 2>/dev/null; sn=$?
[ "$sc" = "$sn" ] && cmp -s "$T/c1.out" "$T/n1.out" || { echo "FAIL jsonl corpus output differs between backends"; fail=1; }
"$T/jsonl-c" "$T/c1.out" >"$T/c2.out" 2>"$T/c2.err"; [ $? = 0 ] && cmp -s "$T/c1.out" "$T/c2.out" || { echo "FAIL jsonl canonical output is not a fixed point"; fail=1; }
[ $fail = 0 ] && echo "jsonl: ok ($n corpus lines, $valid valid, all verdicts match examples/real/json)"
exit $fail
