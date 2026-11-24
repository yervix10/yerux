#!/usr/bin/env bash
# examples/real/tarlite: unit tests on both backends; main built on both; a fixed file set whose archive bytes and list
# output are checked against an independent Python model (zlib CRCs); C and native identical for every command; the
# extract round trip; a corruption sweep (one byte flipped every 13 bytes); unsafe names refused by create, list and
# extract; a seeded corpus of 60 random file sets checked on both backends. Exit 0 when everything passes.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
D=examples/real/tarlite
F="$T/fixed"
N=60   # corpus file sets (the model writes them)
fail=0
SECONDS=0

for flag in "" --native; do
  "$Y" test $D/tests.yx $flag >"$T/t.out" 2>&1 || { echo "FAIL tarlite unit tests ($flag)"; tail -5 "$T/t.out"; fail=1; }
done
"$Y" build $D/main.yx -o "$T/tl-c" >"$T/b1.out" 2>&1 || { echo "FAIL tarlite build (c)"; cat "$T/b1.out"; fail=1; }
"$Y" build $D/main.yx --native -o "$T/tl-n" >"$T/b2.out" 2>&1 || { echo "FAIL tarlite build (native)"; cat "$T/b2.out"; fail=1; }
[ $fail = 0 ] || exit 1

# The independent model: writes the fixed files, the golden archive, the corrupted copies, the hand-made archives with
# unsafe names and a seeded corpus (file sets with random contents, names with spaces and UTF-8, duplicated slashes,
# "./" prefixes). It computes the archive bytes and the list output itself; tarlite is not used.
python3 - "$T" "$N" <<'PY'
import os, random, struct, sys, zlib
T, N = sys.argv[1], int(sys.argv[2])

def archive(entries):  # entries: (name, mode, data) in archive order
    body = b"TLA1"
    for name, mode, data in entries:
        nb = name.encode("utf-8")
        body += struct.pack(">H", len(nb)) + nb + struct.pack(">IQI", mode, len(data), zlib.crc32(data)) + data
    return body + b"TEND" + struct.pack(">II", len(entries), zlib.crc32(body))

def listing(entries):
    return "".join("%04o %d %08x %s\n" % (m, len(d), zlib.crc32(d), n) for n, m, d in entries)

def put(path, data, mode=None):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data)
    if mode is not None:
        os.chmod(path, mode)

F = os.path.join(T, "fixed")
fixed = [("a.txt", 0o644, b"hello\n"), ("empty", 0o600, b""),
         ("dir/b.bin", 0o755, bytes((i * 37) % 256 for i in range(300))),
         ("sp ace.txt", 0o640, b"x y"), ("dir/sub/c", 0o644, bytes([0, 1, 2]))]
for name, mode, data in fixed:
    put(os.path.join(F, "src", name), data, mode)
golden = archive(fixed)
put(os.path.join(F, "golden.tla"), golden)
with open(os.path.join(F, "golden.list"), "w") as f:
    f.write(listing(fixed))

for p in range(0, len(golden), 13):  # every 13th byte, inverted
    b = bytearray(golden)
    b[p] ^= 0xFF
    put(os.path.join(F, "flip", "%03d.tla" % p), bytes(b))

# Hand-made archives whose names must be refused (in the order of UNSAFE_MSG in the script).
for i, name in enumerate(["../x", "/abs", "a/../x", "a//b", "./a", ""]):
    put(os.path.join(F, "unsafe", "%d.tla" % i), archive([(name, 0o644, b"x")]))

# Seeded corpus.
rng = random.Random(20260)
POOL = ["a", "b c", "x.y", ".h", "dir", "sub d", "\u00e9", "n-1", "z_9"]
MODES = [0o644, 0o600, 0o755, 0o640, 0o444, 0o700, 0o4755]
seen = []
for i in range(N):
    c = os.path.join(T, "corpus", "c%d" % i)
    entries, args = [], []
    while len(entries) < rng.randrange(1, 7):
        parts = [rng.choice(POOL) for _ in range(rng.randrange(1, 4))]
        canon = "/".join(parts)
        if any(s == canon or s.startswith(canon + "/") or canon.startswith(s + "/") for s in seen):
            continue
        seen.append(canon)
        arg = canon
        if rng.random() < 0.2:
            arg = arg.replace("/", "//")
        if rng.random() < 0.2:
            arg = "./" + arg
        mode = rng.choice(MODES)
        data = b"" if rng.random() < 0.2 else rng.randbytes(rng.randrange(1, 3000))
        put(os.path.join(c, "src", canon), data, mode)
        entries.append((canon, mode, data))
        args.append(arg)
    put(os.path.join(c, "args"), ("\n".join(args) + "\n").encode("utf-8"))
    put(os.path.join(c, "golden.tla"), archive(entries))
    put(os.path.join(c, "golden.list"), listing(entries).encode("utf-8"))
PY

# ---- the fixed set: exact list and bytes, extraction, the error paths ----
cat >"$F/list.want" <<'OUT'
0644 6 363a3020 a.txt
0600 0 00000000 empty
0755 300 f9e65611 dir/b.bin
0640 3 19fe0dc0 sp ace.txt
0644 3 0854897f dir/sub/c
OUT
cmp -s "$F/list.want" "$F/golden.list" || { echo "FAIL tarlite model disagrees with the literal list"; fail=1; }

for b in c n; do
  (cd "$F/src" && "$T/tl-$b" create "$F/$b.tla" ./a.txt empty dir//b.bin "sp ace.txt" dir/sub/c) >"$F/cr.$b.out" 2>"$F/cr.$b.err"; st=$?
  [ $st = 0 ] || { echo "FAIL tarlite create ($b): status $st"; cat "$F/cr.$b.err"; fail=1; }
  cmp -s "$F/golden.tla" "$F/$b.tla" || {
    echo "FAIL tarlite archive bytes differ from the golden ($b)"
    diff <(od -An -tx1 "$F/golden.tla") <(od -An -tx1 "$F/$b.tla") | head -6; fail=1; }
done
for b in c n; do
  "$T/tl-$b" list "$F/golden.tla" >"$F/list.$b" 2>"$F/list.$b.err"; st=$?
  [ $st = 0 ] && cmp -s "$F/list.want" "$F/list.$b" || { echo "FAIL tarlite list ($b): status $st"; diff "$F/list.want" "$F/list.$b"; cat "$F/list.$b.err"; fail=1; }
  "$T/tl-$b" verify "$F/golden.tla" >"$F/verify.$b" 2>&1; st=$?
  { [ $st = 0 ] && [ "$(cat "$F/verify.$b")" = "ok 5 entries" ]; } || { echo "FAIL tarlite verify ($b): status $st"; cat "$F/verify.$b"; fail=1; }
  "$T/tl-$b" extract "$F/golden.tla" "$F/x-$b" >"$F/ex.$b.out" 2>"$F/ex.$b.err"; st=$?
  [ $st = 0 ] || { echo "FAIL tarlite extract ($b): status $st"; cat "$F/ex.$b.err"; fail=1; }
  diff -r "$F/src" "$F/x-$b" >/dev/null || { echo "FAIL tarlite extract round trip ($b)"; diff -r "$F/src" "$F/x-$b" | head -5; fail=1; }
  # Re-creating from the extracted tree must give back the golden bytes, modes included.
  (cd "$F/x-$b" && "$T/tl-$b" create "$F/re-$b.tla" ./a.txt empty dir//b.bin "sp ace.txt" dir/sub/c) 2>/dev/null
  cmp -s "$F/golden.tla" "$F/re-$b.tla" || { echo "FAIL tarlite re-create after extract ($b)"; fail=1; }
  # Existing paths are never overwritten, and the message names the first one.
  "$T/tl-$b" extract "$F/golden.tla" "$F/x-$b" >/dev/null 2>"$F/again.$b.err"; st=$?
  { [ $st = 1 ] && [ "$(cat "$F/again.$b.err")" = "tarlite: $F/x-$b/a.txt: already exists" ]; } || { echo "FAIL tarlite overwrite refusal ($b): status $st"; cat "$F/again.$b.err"; fail=1; }
done
cmp -s "$F/list.c" "$F/list.n" && cmp -s "$F/ex.c.out" "$F/ex.n.out" && cmp -s "$F/ex.c.err" "$F/ex.n.err" || { echo "FAIL tarlite C and native differ on list/extract"; fail=1; }

# Usage errors (status 2) and the messages.
usage() { # want message, args...
  local want=$1; shift
  for b in c n; do
    "$T/tl-$b" "$@" >"$T/u.out" 2>"$T/u.err"; st=$?
    { [ $st = 2 ] && [ "$(cat "$T/u.err")" = "$want" ]; } || { echo "FAIL tarlite usage ($b): $* -> status $st: $(cat "$T/u.err")"; fail=1; }
  done
}
usage "tarlite: usage: tarlite create|list|verify|extract ..."
usage "tarlite: usage: tarlite create ARCHIVE FILE..." create "$T/none.tla"
usage "tarlite: usage: tarlite list ARCHIVE" list
usage "tarlite: usage: tarlite verify ARCHIVE" verify "$F/golden.tla" extra
usage "tarlite: usage: tarlite extract ARCHIVE DIR" extract "$F/golden.tla"
usage "tarlite: usage: tarlite create|list|verify|extract ..." bogus "$F/golden.tla"

# Unsafe names: create refuses them before writing, and list, verify and extract refuse hand-made archives with them.
for b in c n; do
  for arg in "../x" "/abs" "a/../b" "." "" ; do
    (cd "$F/src" && "$T/tl-$b" create "$F/u.$b.tla" "$arg") >/dev/null 2>"$T/u.err"; st=$?
    [ $st = 1 ] && [ ! -e "$F/u.$b.tla" ] || { echo "FAIL tarlite create accepted '$arg' ($b): status $st"; fail=1; }
  done
  (cd "$F/src" && "$T/tl-$b" create "$F/u.$b.tla" "../x") 2>"$T/u.err"
  [ "$(cat "$T/u.err")" = "tarlite: ../x: '..' component" ] || { echo "FAIL tarlite create message ($b)"; cat "$T/u.err"; fail=1; }
done
UNSAFE_MSG=(
  "entry '../x': '..' component"
  "entry '/abs': absolute path"
  "entry 'a/../x': '..' component"
  "entry 'a//b': name is not canonical"
  "entry './a': name is not canonical"
  "entry '': empty name"
)
for i in "${!UNSAFE_MSG[@]}"; do
  arc="$F/unsafe/$i.tla"
  for b in c n; do
    for cmd in list verify; do
      "$T/tl-$b" $cmd "$arc" >"$T/u.out" 2>"$T/u.err"; st=$?
      [ $st = 1 ] && [ "$(cat "$T/u.err")" = "tarlite: $arc: ${UNSAFE_MSG[$i]}" ] || { echo "FAIL tarlite $cmd unsafe #$i ($b): status $st: $(cat "$T/u.err")"; fail=1; }
    done
    "$T/tl-$b" extract "$arc" "$F/ux-$b" >"$T/u.out" 2>"$T/u.err"; st=$?
    [ $st = 1 ] && [ "$(cat "$T/u.err")" = "tarlite: $arc: ${UNSAFE_MSG[$i]}" ] || { echo "FAIL tarlite extract unsafe #$i ($b): status $st: $(cat "$T/u.err")"; fail=1; }
  done
done
[ ! -e "$F/x" ] && [ ! -e "$F/ux-c" ] && [ ! -e "$F/ux-n" ] || { echo "FAIL tarlite unsafe extract wrote something"; fail=1; }

# Corruption sweep: each copy has one byte inverted. verify must fail for every one of them (the trailer CRC covers
# the whole body), every command must exit 1 (never 0 or a crash), and the outputs must be identical on both backends.
python3 - "$F/golden.tla" "$F/flip" 13 <<'PY'
import os, sys
src, out, step = sys.argv[1], sys.argv[2], int(sys.argv[3])
data = open(src, "rb").read()
os.makedirs(out, exist_ok=True)
for p in range(0, len(data), step):
    b = bytearray(data)
    b[p] ^= 0xFF
    with open(os.path.join(out, "%03d.tla" % p), "wb") as f:
        f.write(bytes(b))
PY
flips=0
for arc in "$F"/flip/*.tla; do
  flips=$((flips + 1))
  for cmd in verify list; do
    for b in c n; do
      "$T/tl-$b" $cmd "$arc" >"$T/f.$b.out" 2>"$T/f.$b.err"; echo $? >"$T/f.$b.st"
    done
    [ "$(cat "$T/f.c.st")" = 1 ] && [ "$(cat "$T/f.n.st")" = 1 ] || { echo "FAIL tarlite $cmd corrupt $(basename "$arc"): status $(cat "$T/f.c.st")/$(cat "$T/f.n.st")"; fail=1; }
    cmp -s "$T/f.c.out" "$T/f.n.out" && cmp -s "$T/f.c.err" "$T/f.n.err" || { echo "FAIL tarlite corrupt $(basename "$arc"): C and native differ"; fail=1; }
    grep -q '^tarlite: ' "$T/f.c.err" || { echo "FAIL tarlite corrupt $(basename "$arc"): no message"; fail=1; }
  done
  for b in c n; do
    "$T/tl-$b" extract "$arc" "$F/fx-$b" >/dev/null 2>&1; st=$?
    [ $st = 1 ] && [ ! -e "$F/fx-$b" ] || { echo "FAIL tarlite extract corrupt $(basename "$arc") ($b): status $st or partial output"; fail=1; }
  done
done
[ $flips -ge 20 ] || { echo "FAIL tarlite corruption sweep too short ($flips)"; fail=1; }

# Seeded corpus: each file set is created on both backends (archive bytes = the model's), listed, verified, extracted
# (tree = the source tree) and re-created from the extracted tree (archive bytes = the model's again).
bad=0
cases=("$T"/corpus/c*)
for c in "${cases[@]}"; do
  mapfile -t args <"$c/args"
  for b in c n; do
    (cd "$c/src" && "$T/tl-$b" create "$c/$b.tla" "${args[@]}") >/dev/null 2>"$c/err.$b"; st=$?
    if [ $st != 0 ] || ! cmp -s "$c/golden.tla" "$c/$b.tla"; then
      bad=$((bad + 1)); [ $bad -le 3 ] && echo "FAIL tarlite corpus create ($(basename "$c"), $b): status $st $(cat "$c/err.$b")"; continue
    fi
    "$T/tl-$b" list "$c/golden.tla" >"$c/l.$b" 2>&1 && cmp -s "$c/golden.list" "$c/l.$b" || { bad=$((bad + 1)); [ $bad -le 3 ] && echo "FAIL tarlite corpus list ($(basename "$c"), $b)"; }
    [ "$("$T/tl-$b" verify "$c/golden.tla")" = "ok ${#args[@]} entries" ] || { bad=$((bad + 1)); [ $bad -le 3 ] && echo "FAIL tarlite corpus verify ($(basename "$c"), $b)"; }
    "$T/tl-$b" extract "$c/golden.tla" "$c/x-$b" 2>"$c/xerr.$b" || { bad=$((bad + 1)); [ $bad -le 3 ] && echo "FAIL tarlite corpus extract ($(basename "$c"), $b): $(cat "$c/xerr.$b")"; continue; }
    diff -r "$c/src" "$c/x-$b" >/dev/null || { bad=$((bad + 1)); [ $bad -le 3 ] && echo "FAIL tarlite corpus round trip ($(basename "$c"), $b)"; }
    (cd "$c/x-$b" && "$T/tl-$b" create "$c/re-$b.tla" "${args[@]}") 2>/dev/null
    cmp -s "$c/golden.tla" "$c/re-$b.tla" || { bad=$((bad + 1)); [ $bad -le 3 ] && echo "FAIL tarlite corpus re-create ($(basename "$c"), $b)"; }
  done
done
[ $bad = 0 ] || { echo "FAIL tarlite corpus: $bad problems"; fail=1; }
[ ${#cases[@]} = "$N" ] || { echo "FAIL tarlite corpus: ${#cases[@]} cases, want $N"; fail=1; }

[ $fail = 0 ] && echo "tarlite: ok (unit tests on both backends, $flips corrupted copies, ${#cases[@]} corpus sets, C and native identical; ${SECONDS}s)"
exit $fail
