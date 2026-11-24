#!/usr/bin/env bash
# examples/real/btre: unit tests on both backends; a fixed corpus whose report and replacement outputs are compared
# exactly (and between the C and the native backend); exit statuses for usage, pattern and limit errors; a seeded
# corpus checked against Python's re for the subset both engines share (first match and its groups per line).
# Exit 0 when everything passes.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
D=examples/real/btre
fail=0
for flag in "" --native; do
  "$Y" test $D/tests.yx $flag >"$T/t.out" 2>&1 || { echo "FAIL btre unit tests ($flag)"; tail -5 "$T/t.out"; fail=1; }
done
"$Y" build $D/main.yx -o "$T/btre-c" >"$T/b1.out" 2>&1 || { echo "FAIL btre build (c)"; cat "$T/b1.out"; fail=1; }
"$Y" build $D/main.yx --native -o "$T/btre-n" >"$T/b2.out" 2>&1 || { echo "FAIL btre build (native)"; cat "$T/b2.out"; fail=1; }
[ $fail = 0 ] || exit 1

printf '%s\n' 'name=ann age=31' 'nothing here' 'id=7;id=88' '' 'last: (x)(y)' 'k=v;k2=v2' >"$T/in.txt"
check() { # name, expected status, expected stdout file, args...
  local name=$1 want=$2 wout=$3; shift 3
  for b in c n; do
    "$T/btre-$b" "$@" "$T/in.txt" >"$T/o.$b" 2>"$T/e.$b"; local st=$?
    [ "$st" = "$want" ] || { echo "FAIL btre $name status ($b): $st, want $want"; cat "$T/e.$b"; fail=1; }
    [ -z "$wout" ] || cmp -s "$wout" "$T/o.$b" || { echo "FAIL btre $name output ($b)"; diff "$wout" "$T/o.$b"; fail=1; }
  done
  cmp -s "$T/o.c" "$T/o.n" || { echo "FAIL btre $name differs between backends"; fail=1; }
}
cat >"$T/report.out" <<'OUT'
1:1	name=ann	1:name	2:ann
3:1	id=7	1:id	2:7
6:1	k=v	1:k	2:v
OUT
check report 0 "$T/report.out" '(\w+)=(\w+)'
cat >"$T/repl.out" <<'OUT'
<name|ann> <age|31>
nothing here
<id|7>;<id|88>

last: (x)(y)
<k|v>;<k2|v2>
OUT
check replace 0 "$T/repl.out" --replace '<\1|\2>' '(\w+)=(\w+)'
printf '1\n' >"$T/count.out"
check count 0 "$T/count.out" -c 'id'
check nomatch 1 "" 'zzz'
check usage 2 "" --bogus 'a'
check badpattern 2 "" '(a'
check badrepl 2 "" --replace '\3' '(a)'
check limit 3 "" --limit 50 '(\w|\w\w)*=!'

# Empty matches advance one byte, as sed s///g does.
printf 'abc\n' >"$T/abc.txt"
for b in c n; do
  "$T/btre-$b" --replace '-' 'x*' "$T/abc.txt" >"$T/x.$b" 2>&1
  printf -- '-a-b-c-\n' | cmp -s - "$T/x.$b" || { echo "FAIL btre empty matches ($b)"; cat "$T/x.$b"; fail=1; }
done

# Seeded corpus: the first match of each pattern of the subset both engines share (position and groups) must agree
# with Python's re on every line. Patterns re rejects, or btre rejects as possibly empty in a loop, are skipped.
mkdir -p "$T/ref"
python3 - "$T" 120 <<'PY'
import os, random, re, sys
T, N = sys.argv[1], int(sys.argv[2])
rng = random.Random(7)
atoms = ["a", "b", "c", ".", "[ab]", "[^a]", "\\d", "\\w", "1"]
def rx(depth):
    k = rng.randrange(6 if depth < 2 else 3)
    if k == 0: return rng.choice(atoms)
    if k == 1: return "(" + rx(depth + 1) + ")" + rng.choice(["*", "+", "?", "*?", "{2}", "{1,2}"])
    if k == 2: return rx(depth + 1) + rx(depth + 1)
    if k == 3: return "(" + rx(depth + 1) + ")"
    if k == 4: return rx(depth + 1) + "|" + rx(depth + 1)
    return "^" + rx(depth + 1) if rng.random() < 0.3 else rx(depth + 1)
lines = ["".join(rng.choice("abc1 2") for _ in range(rng.randrange(0, 12))) for _ in range(120)]
open(os.path.join(T, "corpus.txt"), "w").write("\n".join(lines) + "\n")
pats = []
for i in range(N):
    p = rx(0)
    try:
        r = re.compile(p)
    except re.error:
        continue
    out = []
    for n, line in enumerate(lines, 1):
        m = r.search(line)
        if m:
            out.append(f"{n}:{m.start()+1}\t{m.group(0)}" + "".join(f"\t{g}:{'-' if m.group(g) is None else m.group(g)}" for g in range(1, r.groups + 1)))
    k = len(pats)
    pats.append(p)
    with open(os.path.join(T, "ref", f"{k:03d}.txt"), "w") as f:
        f.write("\n".join(out) + "\n" if out else "")
open(os.path.join(T, "corpus.pat"), "w").write("\n".join(pats) + "\n")
PY
n=0; mism=0; agree=0; skip=0
while IFS= read -r pat; do
  k=$(printf '%03d' $n); n=$((n + 1))
  "$T/btre-c" "$pat" "$T/corpus.txt" >"$T/cp.c" 2>"$T/ce.c"; st=$?
  if [ $st = 2 ] && grep -q "empty string" "$T/ce.c"; then skip=$((skip + 1)); continue; fi
  agree=$((agree + 1))
  if [ -s "$T/ref/$k.txt" ]; then want=0; else want=1; fi
  [ "$st" = "$want" ] || { mism=$((mism + 1)); echo "btre corpus: /$pat/ (c) status $st, want $want"; }
  cmp -s "$T/ref/$k.txt" "$T/cp.c" || { mism=$((mism + 1)); [ $mism -le 3 ] && echo "btre corpus differs on /$pat/ (c)"; }
  "$T/btre-n" "$pat" "$T/corpus.txt" >"$T/cp.n" 2>/dev/null
  cmp -s "$T/cp.c" "$T/cp.n" || { mism=$((mism + 1)); echo "btre corpus differs on /$pat/ between backends"; }
done <"$T/corpus.pat"
[ $mism = 0 ] || { echo "FAIL btre corpus: $mism mismatches over $agree patterns"; fail=1; }
[ $fail = 0 ] && echo "btre: ok ($agree corpus patterns agree with re, $skip skipped as possibly empty in a loop)"
exit $fail
