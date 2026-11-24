#!/usr/bin/env bash
# examples/real/forth: unit tests on both backends; a fixed program with exact output; compile and runtime errors with
# exact messages and statuses; a seeded corpus of random stack programs whose output is computed by an independent
# model in Python (64-bit wrapping, truncating division), checked on both backends. Exit 0 when everything passes.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
D=examples/real/forth
fail=0
for flag in "" --native; do
  "$Y" test $D/tests.yx $flag >"$T/t.out" 2>&1 || { echo "FAIL forth unit tests ($flag)"; tail -5 "$T/t.out"; fail=1; }
done
"$Y" build $D/main.yx -o "$T/forth-c" >"$T/b1.out" 2>&1 || { echo "FAIL forth build (c)"; cat "$T/b1.out"; fail=1; }
"$Y" build $D/main.yx --native -o "$T/forth-n" >"$T/b2.out" 2>&1 || { echo "FAIL forth build (native)"; cat "$T/b2.out"; fail=1; }
[ $fail = 0 ] || exit 1

# A program with definitions, variables, loops and a constant: exact output.
cat >"$T/fizz.fs" <<'FS'
\ FizzBuzz to 15, with a word per decision, a counter variable and a constant
variable count
15 constant limit
: tick count @ 1 + count ! ;
: fb ( n -- )
  tick
  dup 15 mod 0= if drop ." FizzBuzz" else
  dup 3 mod 0= if drop ." Fizz" else
  dup 5 mod 0= if drop ." Buzz" else . then then then
  space ;
limit 1 do i fb loop cr
count @ . cr
FS
printf '%s\n' "1  2  Fizz 4  Buzz Fizz 7  8  Fizz Buzz 11  Fizz 13  14  " "14 " >"$T/fizz.want"
for b in c n; do
  "$T/forth-$b" "$T/fizz.fs" >"$T/fizz.$b" 2>&1; st=$?
  [ $st = 0 ] || { echo "FAIL forth fizz status ($b): $st"; cat "$T/fizz.$b"; fail=1; }
  cmp -s "$T/fizz.want" "$T/fizz.$b" || { echo "FAIL forth fizz output ($b)"; diff "$T/fizz.want" "$T/fizz.$b"; fail=1; }
done

# Errors: stderr is checked exactly; status 1 for compile and runtime errors, 2 for usage.
printf '1 2 +\n: x 1 ;\nfoo\n' >"$T/e1.fs"
printf '1 .\n0 0 /\n' >"$T/e2.fs"
err() { # name, want status, want stderr, args (stdin is /dev/null)
  local name=$1 want=$2 wanterr=$3; shift 3
  for b in c n; do
    "$T/forth-$b" "$@" </dev/null >"$T/eo.$b" 2>"$T/ee.$b"; local st=$?
    [ "$st" = "$want" ] || { echo "FAIL forth $name status ($b): $st, want $want"; cat "$T/ee.$b"; fail=1; }
    [ "$(cat "$T/ee.$b")" = "$wanterr" ] || { echo "FAIL forth $name message ($b)"; cat "$T/ee.$b"; fail=1; }
  done
}
err unknown 1 "forth: $T/e1.fs:3: unknown word 'foo'" "$T/e1.fs"
err runtime 1 "forth: runtime error: division by zero" "$T/e2.fs"
err usage 2 "forth: usage: forth [FILE]" "$T/e1.fs" extra
err nofile 1 "forth: cannot read $T/none.fs: open '$T/none.fs': No such file or directory (os error 2)" "$T/none.fs"
"$T/forth-c" "$T/e2.fs" >"$T/e2.out" 2>/dev/null
cmp -s "$T/e2.out" <(printf '1 ') || { echo "FAIL forth output before a runtime error"; fail=1; }

# Seeded corpus: random stack programs (generated with a model of the stack, so they never underflow or divide by zero)
# and the output the model predicts; both backends must print exactly that.
python3 - "$T" 120 <<'PY'
import os, random, sys
T, N = sys.argv[1], int(sys.argv[2])
rng = random.Random(11)
M = 1 << 64
def wrap(x):
    return ((x + (1 << 63)) % M) - (1 << 63)
def tdiv(a, b):
    q = abs(a) // abs(b)
    return q if (a < 0) == (b < 0) else -q
def flag(b):
    return -1 if b else 0
for i in range(N):
    st, out, src = [], [], []
    for _ in range(rng.randrange(8, 40)):
        k = rng.randrange(14)
        if k <= 2 or len(st) < 2 and k < 6:
            v = rng.choice([rng.randrange(-20, 21), rng.randrange(-2**40, 2**40), 2**63 - 1, -2**63])
            st.append(v); src.append(str(v)); continue
        if k == 3 and st:
            st.append(st[-1]); src.append("dup"); continue
        if k == 4 and st:
            out.append(str(st.pop())); src.append("."); continue
        if k == 5 and st:
            st.pop(); src.append("drop"); continue
        if len(st) < 2:
            continue
        op = rng.choice(["+", "-", "*", "/", "mod", "min", "max", "=", "<", "and", "or", "xor", "swap", "over"])
        b, a = st.pop(), st.pop()
        if op in ("/", "mod") and b == 0:
            st.append(a); st.append(b); continue
        if op == "+": r = [wrap(a + b)]
        elif op == "-": r = [wrap(a - b)]
        elif op == "*": r = [wrap(a * b)]
        elif op == "/": r = [wrap(tdiv(a, b))]
        elif op == "mod": r = [a - b * tdiv(a, b)]
        elif op == "min": r = [min(a, b)]
        elif op == "max": r = [max(a, b)]
        elif op == "=": r = [flag(a == b)]
        elif op == "<": r = [flag(a < b)]
        elif op == "and": r = [a & b]
        elif op == "or": r = [a | b]
        elif op == "xor": r = [a ^ b]
        elif op == "swap": r = [b, a]
        elif op == "over": r = [a, b, a]
        if len(r) == 1 and op in ("+", "-", "*", "/", "mod", "min", "max", "=", "<", "and", "or", "xor"):
            st.append(r[0])
        else:
            st.extend(r)
        src.append(op)
    # print what is left, so the final stack is checked too
    while st:
        out.append(str(st.pop())); src.append(".")
    with open(os.path.join(T, f"p{i}.fs"), "w") as f:
        f.write(" ".join(src) + "\n")
    with open(os.path.join(T, f"p{i}.want"), "w") as f:
        f.write("".join(s + " " for s in out))
PY
mism=0; ok=0
for i in $(seq 0 119); do
  for b in c n; do
    "$T/forth-$b" "$T/p$i.fs" >"$T/po.$b" 2>"$T/pe.$b"; st=$?
    if [ $st != 0 ] || ! cmp -s "$T/p$i.want" "$T/po.$b"; then
      mism=$((mism + 1)); [ $mism -le 3 ] && { echo "forth corpus p$i ($b): status $st"; cat "$T/pe.$b"; diff "$T/p$i.want" "$T/po.$b" | head -4; }
    fi
  done
  ok=$((ok + 1))
done
[ $mism = 0 ] || { echo "FAIL forth corpus: $mism mismatches"; fail=1; }
[ $fail = 0 ] && echo "forth: ok ($ok random programs match the model on both backends)"
exit $fail
