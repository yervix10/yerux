#!/usr/bin/env bash
# examples/real/sudoku: unit tests on both backends; main built on both backends; every command on fixed grids and on a
# seeded corpus of 40 `gen` runs is checked against an independent Python model (its own solver, LCG and generator):
# the exit status, standard output and standard error must equal the model's, and C and native must agree. Exit 0 when
# everything passes.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
D=examples/real/sudoku
fail=0
SECONDS=0

for flag in "" --native; do
  "$Y" test $D/tests.yx $flag >"$T/t.out" 2>&1 || { echo "FAIL sudoku unit tests ($flag)"; tail -5 "$T/t.out"; fail=1; }
done
"$Y" build $D/main.yx -o "$T/sd-c" >"$T/b1.out" 2>&1 || { echo "FAIL sudoku build (c)"; cat "$T/b1.out"; fail=1; }
"$Y" build $D/main.yx --native -o "$T/sd-n" >"$T/b2.out" 2>&1 || { echo "FAIL sudoku build (native)"; cat "$T/b2.out"; fail=1; }
[ $fail = 0 ] || exit 1

# The independent model. Cases are written to $T/case: NNN.args (one argument per line), NNN.st, NNN.out, NNN.err, and
# the number of cases in count. Nothing here is shared with the Yerux sources.
python3 - "$T" <<'PY'
import os, random, sys
T = sys.argv[1]
CASE = os.path.join(T, "case")
os.makedirs(CASE, exist_ok=True)
A_MUL, C_ADD, M64 = 6364136223846793005, 1442695040888963407, 1 << 64
MAX_ATTEMPTS = 40

# ---- the grid model: candidates, conflicts, counting ----
def cands(g, i):
    r, c = divmod(i, 9)
    used = set()
    for k in range(9):
        used.add(g[r * 9 + k])
        used.add(g[k * 9 + c])
    br, bc = r - r % 3, c - c % 3
    for a in range(3):
        for b in range(3):
            used.add(g[(br + a) * 9 + bc + b])
    return [d for d in range(1, 10) if d not in used]

def units(i):
    r, c = divmod(i, 9)
    return r, c, (r // 3) * 3 + c // 3

def conflict(g):
    # the first cell (row order) whose digit equals that of an earlier cell in its row, column or box
    seen = [[0] * 9, [0] * 9, [0] * 9]  # per row, column, box: the set of digits so far, as a bitmask
    for i in range(81):
        if g[i] == 0:
            continue
        r, c, b = units(i)
        bit = 1 << g[i]
        if (seen[0][r] | seen[1][c] | seen[2][b]) & bit:
            return i
        seen[0][r] |= bit
        seen[1][c] |= bit
        seen[2][b] |= bit
    return None

def count(g, limit=2):
    if conflict(g) is not None:
        return 0
    g = list(g)
    used = [[0] * 9, [0] * 9, [0] * 9]
    for i in range(81):
        if g[i]:
            r, c, b = units(i)
            used[0][r] |= 1 << g[i]
            used[1][c] |= 1 << g[i]
            used[2][b] |= 1 << g[i]
    def free(i):
        r, c, b = units(i)
        return [d for d in range(1, 10) if not (used[0][r] | used[1][c] | used[2][b]) >> d & 1]
    def rec():
        best = None
        for i in range(81):
            if g[i] == 0:
                r, c, b = units(i)
                m = ~(used[0][r] | used[1][c] | used[2][b]) & 0x3FE
                n = bin(m).count("1")
                if n == 0:
                    return 0
                if best is None or n < best[1]:
                    best = (i, n)
        if best is None:
            return 1
        i = best[0]
        r, c, b = units(i)
        total = 0
        for d in free(i):
            g[i] = d
            used[0][r] |= 1 << d
            used[1][c] |= 1 << d
            used[2][b] |= 1 << d
            total += rec()
            used[0][r] &= ~(1 << d)
            used[1][c] &= ~(1 << d)
            used[2][b] &= ~(1 << d)
            g[i] = 0
            if total >= limit:
                return total
        return total
    return rec()

def first_solution(g):
    # only used when count is 1: the unique solution, by plain row-order backtracking
    g = list(g)
    def rec(i):
        while i < 81 and g[i] != 0:
            i += 1
        if i == 81:
            return True
        for d in cands(g, i):
            g[i] = d
            if rec(i + 1):
                return True
            g[i] = 0
        return False
    assert rec(0)
    return g

def text(g):
    return "\n".join("".join("." if g[r * 9 + c] == 0 else str(g[r * 9 + c]) for c in range(9)) for r in range(9)) + "\n"

def parse(s):
    rows = [l.strip() for l in s.splitlines() if l.strip()]
    assert len(rows) == 9 and all(len(r) == 9 for r in rows), s
    return [0 if ch in ".0" else int(ch) for r in rows for ch in r]

# ---- the generator model: LCG, shuffle, fill, removal ----
def draw(st):
    st[0] = (st[0] * A_MUL + C_ADD) % M64
    return st[0] >> 32

def shuffle(st, xs):
    i = len(xs) - 1
    while i > 0:
        j = draw(st) % (i + 1)
        xs[i], xs[j] = xs[j], xs[i]
        i -= 1

def fill(g, st):
    best = None
    for i in range(81):
        if g[i] == 0:
            n = len(cands(g, i))
            if n == 0:
                return False
            if best is None or n < best[1]:
                best = (i, n)
    if best is None:
        return True
    i = best[0]
    perm = list(range(1, 10))
    shuffle(st, perm)
    for d in perm:
        if d in cands(g, i):
            g[i] = d
            if fill(g, st):
                return True
            g[i] = 0
    return False

def gen(seed, holes):
    st = [seed]
    for _ in range(MAX_ATTEMPTS):
        g = [0] * 81
        assert fill(g, st)
        order = list(range(81))
        shuffle(st, order)
        removed, k = 0, 0
        while k < 81 and removed < holes:
            i = order[k]
            d = g[i]
            g[i] = 0
            if count(g, 2) == 1:
                removed += 1
            else:
                g[i] = d
            k += 1
        if removed == holes:
            return g
    return None

# ---- the fixed grids ----
SOL = "534678912\n672195348\n198342567\n859761423\n426853791\n713924856\n961537284\n287419635\n345286179\n"
P1 = "53..7....\n6..195...\n.98....6.\n8...6...3\n4..8.3..1\n7...2...6\n.6....28.\n...419..5\n....8..79\n"
P2 = "534..8912\n672195348\n198342567\n859..1423\n426853791\n713924856\n961537284\n287419635\n345286179\n"
BLANK = "\n".join(["........."] * 9) + "\n"
UNSAT = "12345678.\n.........\n.........\n.........\n.........\n.........\n.........\n.........\n........9\n"
CONFLICT = "5.5......\n.........\n.........\n.........\n.........\n.........\n.........\n.........\n.........\n"
CRLF = "  53..7....\r\n\r\n6..195...\r\n.98....6.\n8...6...3\n4..8.3..1\n7...2...6\n.6....28.\n...419..5\n....8..79   \r\n"
SHORT = "\n".join(P1.split("\n")[:8]) + "\n"
BADCHAR = P1.replace("6..195...", "6x.195...")
files = {"p1.txt": P1, "p2.txt": P2, "blank.txt": BLANK, "unsat.txt": UNSAT, "conflict.txt": CONFLICT,
         "solved.txt": SOL, "crlf.txt": CRLF, "short.txt": SHORT, "badchar.txt": BADCHAR, "empty.txt": ""}
for name, body in files.items():
    with open(os.path.join(T, name), "w", newline="") as f:
        f.write(body)
os.makedirs(os.path.join(T, "fixed"), exist_ok=True)

cases = []  # (args, status, stdout, stderr)
def P(name):
    return os.path.join(T, name)

def run_grid(cmd, name, body):
    # the model of solve / count / check for a grid file body (valid grids only)
    g = parse(body) if name != "empty.txt" else None
    if cmd == "check":
        i = conflict(g)
        if i is None:
            return 0, "ok\n", ""
        return 1, f"conflict at {i // 9 + 1},{i % 9 + 1}\n", ""
    n = count(g, 2)
    if cmd == "count":
        return 0, ("2+" if n == 2 else str(n)) + "\n", ""
    if n == 0:
        return 1, "no solution\n", ""
    if n == 2:
        return 1, "multiple solutions\n", ""
    return 0, text(first_solution(g)), ""

VALID = ["p1.txt", "p2.txt", "blank.txt", "unsat.txt", "conflict.txt", "solved.txt", "crlf.txt"]
for name in VALID:
    for cmd in ("solve", "count", "check"):
        st, out, err = run_grid(cmd, name, files[name])
        cases.append(([cmd, P(name)], st, out, err))

bad = {"short.txt": f"sudoku: {P('short.txt')}: expected 9 rows, found 8\n",
       "badchar.txt": f"sudoku: {P('badchar.txt')}: row 2, column 2: bad character 'x'\n",
       "empty.txt": f"sudoku: {P('empty.txt')}: expected 9 rows, found 0\n"}
for name, err in bad.items():
    for cmd in ("solve", "count", "check"):
        cases.append(([cmd, P(name)], 2, "", err))
cases.append((["solve", P("missing.txt")], 2, "",
               f"sudoku: cannot read {P('missing.txt')}: open '{P('missing.txt')}': No such file or directory (os error 2)\n"))

USAGE = {"solve": "sudoku: usage: sudoku solve FILE\n", "count": "sudoku: usage: sudoku count FILE\n",
         "check": "sudoku: usage: sudoku check FILE\n"}
for cmd in ("solve", "count", "check"):
    cases.append(([cmd], 2, "", USAGE[cmd]))
    cases.append(([cmd, P("p1.txt"), "extra"], 2, "", USAGE[cmd]))
cases.append(([], 2, "", "sudoku: usage: sudoku solve|count|gen|check ...\n"))
cases.append((["bogus"], 2, "", "sudoku: usage: sudoku solve|count|gen|check ...\n"))

GEN_USAGE = "sudoku: usage: sudoku gen SEED HOLES\n"
cases.append((["gen", "5"], 2, "", GEN_USAGE))
cases.append((["gen", "5", "6", "7"], 2, "", GEN_USAGE))
cases.append((["gen", "-1", "5"], 2, "", "sudoku: SEED must be a decimal integer from 0 to 2^63-1, got '-1'\n"))
cases.append((["gen", "x", "5"], 2, "", "sudoku: SEED must be a decimal integer from 0 to 2^63-1, got 'x'\n"))
cases.append((["gen", "9223372036854775808", "5"], 2, "",
              "sudoku: SEED must be a decimal integer from 0 to 2^63-1, got '9223372036854775808'\n"))
cases.append((["gen", "5", "65"], 2, "", "sudoku: HOLES must be an integer from 0 to 64, got '65'\n"))
cases.append((["gen", "5", "-1"], 2, "", "sudoku: HOLES must be an integer from 0 to 64, got '-1'\n"))
cases.append((["gen", "5", "x"], 2, "", "sudoku: HOLES must be an integer from 0 to 64, got 'x'\n"))

for seed, holes in ((42, 40), (0, 0), (9223372036854775807, 30), (7, 52)):
    g = gen(seed, holes)
    assert g is not None
    cases.append((["gen", str(seed), str(holes)], 0, text(g), ""))

# gen 64 holes: the model gives up after MAX_ATTEMPTS, and the generator reports it (status 1)
g = gen(3, 64)
if g is None:
    cases.append((["gen", "3", "64"], 1, "", f"sudoku: gen: gave up after {MAX_ATTEMPTS} attempts\n"))
else:
    cases.append((["gen", "3", "64"], 0, text(g), ""))

# Seeded corpus: 40 generator runs, random seeds up to 2^63-1 and random hole counts.
rng = random.Random(20260624)
for i in range(40):
    seed = rng.randrange(0, 1 << 63)
    holes = rng.choice([20, 30, 40, 45, 50, 55, 58])
    g = gen(seed, holes)
    if g is None:
        cases.append((["gen", str(seed), str(holes)], 1, "",
                      f"sudoku: gen: gave up after {MAX_ATTEMPTS} attempts\n"))
    else:
        assert count(g, 2) == 1 and sum(1 for v in g if v == 0) == holes
        cases.append((["gen", str(seed), str(holes)], 0, text(g), ""))

for k, (args, st, out, err) in enumerate(cases):
    base = os.path.join(CASE, "%03d" % k)
    with open(base + ".args", "w") as f:
        f.write("".join(a + "\n" for a in args))
    with open(base + ".st", "w") as f:
        f.write(str(st) + "\n")
    with open(base + ".out", "w", newline="") as f:
        f.write(out)
    with open(base + ".err", "w", newline="") as f:
        f.write(err)
with open(os.path.join(CASE, "count"), "w") as f:
    f.write(str(len(cases)) + "\n")
PY

# ---- every case: exact status, stdout and stderr, on both backends; C and native identical ----
n=$(cat "$T/case/count")
bad=0
for ((k = 0; k < n; k++)); do
  id=$(printf '%03d' "$k")
  mapfile -t A <"$T/case/$id.args"
  want_st=$(cat "$T/case/$id.st")
  label="sudoku ${A[*]}"
  label=${label//$T/TMP}
  for b in c n; do
    "$T/sd-$b" "${A[@]}" >"$T/o.$b" 2>"$T/e.$b"; st=$?
    if [ "$st" != "$want_st" ] || ! cmp -s "$T/case/$id.out" "$T/o.$b" || ! cmp -s "$T/case/$id.err" "$T/e.$b"; then
      bad=$((bad + 1))
      [ $bad -le 5 ] && { echo "FAIL sudoku ($b): $label: status $st want $want_st"; diff "$T/case/$id.out" "$T/o.$b" | head -4; diff "$T/case/$id.err" "$T/e.$b" | head -4; }
    fi
  done
  cmp -s "$T/o.c" "$T/o.n" && cmp -s "$T/e.c" "$T/e.n" || { echo "FAIL sudoku: C and native differ: $label"; fail=1; }
done
[ $bad = 0 ] || { echo "FAIL sudoku: $bad outputs differ from the model"; fail=1; }

[ $fail = 0 ] && echo "sudoku: ok (unit tests on both backends, $n commands x 2 backends, 40 generator runs; ${SECONDS}s)"
exit $fail
