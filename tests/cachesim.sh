#!/usr/bin/env bash
# examples/real/cachesim: unit tests on both backends; main built on both backends; exact output of run and sweep on
# fixed traces against goldens from an independent Python LRU model; C and native identical (status, stdout, stderr)
# for every command; gen equal to the Python generator; a seeded corpus of 80 random traces; the error messages and
# exit statuses. Exit 0 when everything passes.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
D=examples/real/cachesim
fail=0

# The Python model: the oracle for every golden file. It has its own trace parser, its own LRU model (one list per
# set, most recent first) and its own table and generator code; nothing is shared with the Yerux sources.
python3 - "$T" <<'PY' || { echo "FAIL cachesim: python oracle"; exit 1; }
import os, random, sys
T = sys.argv[1]
M = 1 << 64

def parse_trace(text):
    accs = []
    for line in text.split('\n'):
        body = line.strip()
        if not body or body.startswith('#'):
            continue
        op, num = body.split()
        assert op in ('R', 'W'), body
        accs.append((op == 'W', int(num, 0)))
    return accs

def simulate(sets, ways, line, accs):
    st = {'accesses': 0, 'reads': 0, 'writes': 0, 'hits': 0, 'misses': 0, 'evictions': 0, 'writebacks': 0}
    lists = [[] for _ in range(sets)]  # per set: [tag, dirty], most recently used first
    for write, addr in accs:
        st['accesses'] += 1
        st['writes' if write else 'reads'] += 1
        block = addr // line
        lst = lists[block % sets]
        tag = block // sets
        pos = None
        for i, e in enumerate(lst):
            if e[0] == tag:
                pos = i
                break
        if pos is not None:
            st['hits'] += 1
            e = lst.pop(pos)
            e[1] = e[1] or write
            lst.insert(0, e)
        else:
            st['misses'] += 1
            if len(lst) == ways:
                victim = lst.pop()
                st['evictions'] += 1
                if victim[1]:
                    st['writebacks'] += 1
            lst.insert(0, [tag, write])
    return st

def rate4(st):
    a = st['accesses']
    return 0 if a == 0 else (st['misses'] * 20000 + a) // (2 * a)

def rate_text(r4):
    return f"{r4 // 10000}.{r4 % 10000:04d}"

def run_text(st):
    rows = [('accesses', st['accesses']), ('reads', st['reads']), ('writes', st['writes']), ('hits', st['hits']),
            ('misses', st['misses']), ('miss rate', rate_text(rate4(st))), ('evictions', st['evictions']),
            ('writebacks', st['writebacks'])]
    return ''.join(f"{k} {v}\n" for k, v in rows)

WIDTHS = [8, 4, 4, 8, 8, 9, 9, 10]
def table_line(cells):
    first = [cells[0].ljust(WIDTHS[0])]
    rest = [c.rjust(w) for c, w in zip(cells[1:], WIDTHS[1:])]
    return ' '.join(first + rest) + '\n'

def sweep_text(accs, line):
    rows = []
    for ways in (1, 2, 4, 8):
        for sets in (4, 16, 64):
            st = simulate(sets, ways, line, accs)
            rows.append((rate4(st), f"S{sets:03d}-W{ways}", sets, ways, st))
    rows.sort(key=lambda r: (r[0], r[1]))
    text = table_line(['config', 'sets', 'ways', 'accesses', 'misses', 'miss rate', 'evictions', 'writebacks'])
    for r4, name, sets, ways, st in rows:
        text += table_line([name, str(sets), str(ways), str(st['accesses']), str(st['misses']), rate_text(r4),
                            str(st['evictions']), str(st['writebacks'])])
    return text

def gen(seed, n):
    box = [seed]
    def step():
        box[0] = (box[0] * 6364136223846793005 + 1442695040888963407) % M
    def top32():
        return box[0] >> 32
    streams = [1073741824 + j * 16777216 for j in range(4)]
    out = []
    for _ in range(n):
        step(); kind = top32() % 10
        step(); write = top32() % 4 == 0
        if kind < 5:
            step(); addr = 65536 + 64 * (top32() % 64)
        elif kind < 8:
            step(); s = top32() % 4
            addr = streams[s]
            streams[s] += 64
        else:
            step(); addr = box[0]
        out.append(('W' if write else 'R') + ' 0x' + format(addr, 'x') + '\n')
    return ''.join(out)

def write(name, text):
    with open(os.path.join(T, name), 'w', newline='') as f:
        f.write(text)

FIXED = """# fixed trace: every number form, comments, blank lines
R 0x0
R 64
W 0x40
R 0X80
R 192
W 0xC0
R 0x100

# a hot pair and a stream
R 0x1000
R 4096
W 0x1040
R 0x1080
R 0x10c0
R 0x1100
W 0x1000
R 0x2000
R 0x3000
R 0x4000
W 0x2000
R 0x5000
R 0x0
R 0x40
R 0x80
W 0x6000
R 0x7000
R 0x8000
R 0x1000
R 0x9000
   # indented comment
R 0xa000
W 9223372036854775807
R 9223372036854775808
R 18446744073709551615
R 0xfffffffffffffff0
W 0x40
R 0x100
R 0x200
R 0x300
R 0x80
"""
write("fixed.trc", FIXED)
fixed = parse_trace(FIXED)

gen42 = gen(42, 3000)
write("gen42.trc", gen42)
write("gen42.want", gen42)
write("genhex.want", gen(0xdeadbeef, 300))
write("genmax.want", gen(M - 1, 200))
write("gen42_check.want", f"ok {len(parse_trace(gen42))} accesses\n")
write("fixed_check.want", f"ok {len(fixed)} accesses\n")
write("fixed_run.want", run_text(simulate(16, 4, 64, fixed)))
write("fixed_run_custom.want", run_text(simulate(8, 3, 32, fixed)))
write("fixed_sweep.want", sweep_text(fixed, 64))
write("gen42_run.want", run_text(simulate(16, 4, 64, parse_trace(gen42))))
write("gen42_sweep.want", sweep_text(parse_trace(gen42), 16))

# Seeded corpus: 80 random traces with random geometry, both number forms, CRLF and blank/comment lines, and addresses
# up to 2^64-1. Each has its run output and, for every trace, its sweep table.
rng = random.Random(20240611)
manifest = []
for i in range(80):
    sets = 1 << rng.randrange(0, 7)
    ways = rng.choice([1, 2, 3, 4, 5, 6, 8, 16])
    line = 1 << rng.randrange(0, 8)
    n = rng.choice([0, 1, rng.randrange(2, 60), rng.randrange(60, 400)])
    pool = [rng.randrange(0, 1 << 16) for _ in range(max(2, sets * ways))]
    eol = '\r\n' if rng.random() < 0.2 else '\n'
    lines = []
    if rng.random() < 0.5:
        lines.append(f'# corpus trace {i}')
    for _ in range(n):
        r = rng.random()
        if r < 0.45:
            addr = rng.choice(pool)
        elif r < 0.6:
            addr = rng.randrange(0, 4096)
        elif r < 0.75:
            addr = M - 1 - rng.randrange(0, 64)
        elif r < 0.85:
            addr = rng.randrange(0, M)
        else:
            addr = (1 << 63) - 16 + rng.randrange(0, 32)
        op = 'W' if rng.random() < 0.35 else 'R'
        k = rng.random()
        if k < 0.4:
            num = '0x%x' % addr
        elif k < 0.6:
            num = '0X%X' % addr
        else:
            num = str(addr)
        body = op + rng.choice([' ', '\t', '  ']) + num
        lines.append(rng.choice(['', '  ', '\t']) + body + rng.choice(['', ' ', '\t  ']))
        r2 = rng.random()
        if r2 < 0.05:
            lines.append('# note')
        elif r2 < 0.08:
            lines.append('')
        elif r2 < 0.10:
            lines.append('   \t')
        elif r2 < 0.12:
            lines.append('   # indented comment')
    text = (eol.join(lines) + eol) if lines else ''
    write(f"c{i}.trc", text)
    accs = parse_trace(text)
    write(f"c{i}.run", run_text(simulate(sets, ways, line, accs)))
    write(f"c{i}.sweep", sweep_text(accs, line))
    manifest.append(f"{i} {sets} {ways} {line}")
write("manifest", '\n'.join(manifest) + '\n')
PY

# The unit tests (both backends) and both builds run in parallel.
"$Y" test $D/tests.yx >"$T/unit-c.out" 2>&1 & p1=$!
"$Y" test $D/tests.yx --native >"$T/unit-n.out" 2>&1 & p2=$!
"$Y" build $D/main.yx -o "$T/cs-c" >"$T/build-c.out" 2>&1 & p3=$!
"$Y" build $D/main.yx --native -o "$T/cs-n" >"$T/build-n.out" 2>&1 & p4=$!
wait $p1 || { echo "FAIL cachesim unit tests (c)"; tail -5 "$T/unit-c.out"; fail=1; }
wait $p2 || { echo "FAIL cachesim unit tests (native)"; tail -5 "$T/unit-n.out"; fail=1; }
wait $p3 || { echo "FAIL cachesim build (c)"; cat "$T/build-c.out"; fail=1; }
wait $p4 || { echo "FAIL cachesim build (native)"; cat "$T/build-n.out"; fail=1; }
[ $fail = 0 ] || exit 1

IN=/dev/null
# run_both NAME ARGS...: both backends with the same arguments and standard input ($IN). Their status, stdout and
# stderr must agree. Leaves the status in SC, the outputs in $T/out.c and $T/err.c.
run_both() {
  local name=$1; shift
  "$T/cs-c" "$@" <"$IN" >"$T/out.c" 2>"$T/err.c"; SC=$?
  "$T/cs-n" "$@" <"$IN" >"$T/out.n" 2>"$T/err.n"; local sn=$?
  if [ "$SC" != "$sn" ] || ! cmp -s "$T/out.c" "$T/out.n" || ! cmp -s "$T/err.c" "$T/err.n"; then
    echo "FAIL cachesim $name: backends differ (status $SC/$sn)"
    diff "$T/out.c" "$T/out.n" | head -5; diff "$T/err.c" "$T/err.n" | head -5
    fail=1
  fi
}
# want_out NAME GOLDEN ARGS...: status 0, empty stderr, stdout equal to the GOLDEN file.
want_out() {
  local name=$1 golden=$2; shift 2
  run_both "$name" "$@"
  if [ "$SC" = 0 ] && [ ! -s "$T/err.c" ] && cmp -s "$golden" "$T/out.c"; then return; fi
  echo "FAIL cachesim $name: status $SC"; head -3 "$T/err.c"; diff "$golden" "$T/out.c" | head -10; fail=1
}
# want_err NAME STATUS MESSAGE ARGS...: status STATUS, empty stdout, stderr exactly MESSAGE (one line).
want_err() {
  local name=$1 want=$2 msg=$3; shift 3
  run_both "$name" "$@"
  printf '%s\n' "$msg" >"$T/want.err"
  if [ "$SC" = "$want" ] && [ ! -s "$T/out.c" ] && cmp -s "$T/want.err" "$T/err.c"; then return; fi
  echo "FAIL cachesim $name: status $SC, want $want"; cat "$T/err.c"; fail=1
}

# Golden output of run, sweep, check and gen on the fixed trace and on the generated trace.
want_out run-fixed "$T/fixed_run.want" run "$T/fixed.trc"
want_out run-fixed-custom "$T/fixed_run_custom.want" run --sets 8 --ways 3 --line 32 "$T/fixed.trc"
want_out sweep-fixed "$T/fixed_sweep.want" sweep "$T/fixed.trc"
want_out check-fixed "$T/fixed_check.want" check "$T/fixed.trc"
IN="$T/fixed.trc" want_out run-stdin "$T/fixed_run.want" run -
IN="$T/fixed.trc" want_out check-stdin "$T/fixed_check.want" check -
want_out run-gen42 "$T/gen42_run.want" run "$T/gen42.trc"
want_out sweep-gen42 "$T/gen42_sweep.want" sweep --line 16 "$T/gen42.trc"
want_out check-gen42 "$T/gen42_check.want" check "$T/gen42.trc"
want_out gen-42 "$T/gen42.want" gen 42 3000
want_out gen-hex-seed "$T/genhex.want" gen 0xdeadbeef 300
want_out gen-max-seed "$T/genmax.want" gen 18446744073709551615 200
want_out gen-zero /dev/null gen 5 0

# Errors: exact stderr and status; stdout stays empty.
printf 'R 0x10\nW 0x1g\n' >"$T/badhex.trc"
printf 'R 1\nX 2\n' >"$T/badop.trc"
printf 'R 1\n\nR 0x1g\n' >"$T/stdin.bad"
want_err bad-hex 1 "cachesim: $T/badhex.trc:2: bad hex digit 'g'" run "$T/badhex.trc"
want_err bad-op 1 "cachesim: $T/badop.trc:2: unknown operation 'X', expected R or W" check "$T/badop.trc"
IN="$T/stdin.bad" want_err stdin-bad 1 "cachesim: -:3: bad hex digit 'g'" check -
want_err missing-file 1 "cachesim: cannot read $T/none.trc: open '$T/none.trc': No such file or directory (os error 2)" run "$T/none.trc"
want_err non-pow2-sets 2 "cachesim: usage: --sets must be a power of two up to 2^20, got 3" run --sets 3 "$T/fixed.trc"
want_err zero-ways 2 "cachesim: usage: --ways must be a positive integer up to 4096, got 0" run --ways 0 "$T/fixed.trc"
want_err non-pow2-line 2 "cachesim: usage: --line must be a power of two up to 2^30, got 6" sweep --line 6 "$T/fixed.trc"
want_err sweep-sets 2 "cachesim: usage: sweep takes only --line L: sweep [--line L] TRACE" sweep --sets 4 "$T/fixed.trc"
want_err unknown-cmd 2 "cachesim: usage: unknown command 'frob', expected run, sweep, gen or check" frob
want_err no-cmd 2 "cachesim: usage: expected a command: run, sweep, gen or check"
want_err missing-value 2 "cachesim: usage: --sets needs a value" run --sets
want_err unknown-option 2 "cachesim: usage: unknown option --frob" run --frob "$T/fixed.trc"
want_err gen-args 2 "cachesim: usage: gen SEED N" gen 1
want_err gen-neg 2 "cachesim: usage: unknown option -5" gen 1 -5
want_err gen-seed 2 "cachesim: usage: SEED must be an unsigned 64-bit number: bad hex digit 'z'" gen 0xz 5

# Seeded corpus: run and sweep of each random trace, identical on both backends and equal to the Python model.
bad=0
while read -r i s w l; do
  run_both "corpus-$i" run --sets "$s" --ways "$w" --line "$l" "$T/c$i.trc"
  if [ "$SC" != 0 ] || ! cmp -s "$T/c$i.run" "$T/out.c"; then bad=$((bad + 1)); echo "FAIL cachesim corpus $i: run"; fi
  run_both "corpus-sweep-$i" sweep --line "$l" "$T/c$i.trc"
  if [ "$SC" != 0 ] || ! cmp -s "$T/c$i.sweep" "$T/out.c"; then bad=$((bad + 1)); echo "FAIL cachesim corpus $i: sweep"; fi
done <"$T/manifest"
[ $bad = 0 ] || fail=1

[ $fail = 0 ] && echo "cachesim: ok (unit tests and corpus on both backends, goldens, errors)"
exit $fail
