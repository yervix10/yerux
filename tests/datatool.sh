#!/usr/bin/env bash
# examples/real/datatool: golden outputs (tests/datatool/expected), python3 oracles on a generated
# 3000-row CSV, and error paths, on both the C and native backends.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="$ROOT/target/release/yerux"
mkdir -p "$ROOT/.tmp"
TMP="$(mktemp -d "$ROOT/.tmp/datatool.XXXXXX")"; trap 'rm -rf "$TMP"' EXIT
fails=0
E=tests/datatool/expected
fail() { echo "FAIL datatool ($be) $*"; fails=$((fails+1)); }

# Generated input: 3000 records, unusual regions, quoted commas, mixed-width amounts (seeded).
python3 - "$TMP" <<'PY'
import csv, random, sys, os
d = sys.argv[1]
random.seed(20261123)
regions = ["north", "south", "east", "west", "nordic"]
with open(os.path.join(d, "big.csv"), "w", newline="") as f:
    w = csv.writer(f)
    w.writerow(["id", "region", "note", "units", "amount"])
    for i in range(3000):
        note = random.choice(["plain", "a, b", 'say "hi"', "x"])
        cents = random.randint(0, 500000)
        w.writerow([i, random.choice(regions), note, random.randint(1, 9), "%d.%02d" % divmod(cents, 100)])
with open(os.path.join(d, "big_group.toml"), "w") as f:
    f.write('[input]\nfile = "%s/big.csv"\n[group]\nby = "region"\nvalue = "amount"\n[output]\nformat = "csv"\n' % d)
with open(os.path.join(d, "big_query.toml"), "w") as f:
    f.write('[input]\nfile = "%s/big.csv"\n[filter]\ncolumn = "region"\npattern = "^[ns]"\n'
            '[select]\ncolumns = ["id", "region", "note", "amount"]\n'
            '[sort]\ncolumn = "amount"\nnumeric = true\ndescending = true\n[output]\nformat = "csv"\n' % d)
PY

for be in c native; do
  flags=(); [ $be = native ] && flags=(--native)
  "$Y" build examples/real/datatool/main.yx "${flags[@]}" -o "$TMP/dt" >/dev/null 2>&1 || { echo "FAIL datatool build ($be)"; exit 1; }
  Dt="$TMP/dt"

  # Golden outputs for the fixtures: stdout and exit status.
  golden() { # name, args...
    local name="$1"; shift
    "$Dt" "$@" >"$TMP/got" 2>"$TMP/err"; local rc=$?
    if [ "$rc" != "$(cat "$E/$name.code")" ] || ! cmp -s "$E/$name.out" "$TMP/got"; then
      fail "$name rc=$rc"; diff "$E/$name.out" "$TMP/got" | head -6
    fi
  }
  golden check check tests/datatool/sales.csv
  golden sort run tests/datatool/sort.toml
  golden report run tests/datatool/report.toml
  golden group run tests/datatool/group.toml

  # Oracle 1: group statistics over the 3000-row file. count/sum/min/max exact; mean within half a cent.
  "$Dt" run "$TMP/big_group.toml" >"$TMP/groups" || fail "big group rc"
  python3 - "$TMP/big.csv" "$TMP/groups" <<'PY' || fail "group oracle"

import csv, sys
rows = list(csv.DictReader(open(sys.argv[1], newline="")))
acc = {}
for r in rows:
    acc.setdefault(r["region"], []).append(int(r["amount"].replace(".", "")))
got = list(csv.reader(open(sys.argv[2], newline="")))
ok = got[0] == ["region", "count", "sum", "min", "max", "mean"] and len(got) - 1 == len(acc)
for k, line in enumerate(got[1:]):
    cents = acc[line[0]]
    want = ["%d" % len(cents), "%d.%02d" % divmod(sum(cents), 100),
            "%d.%02d" % divmod(min(cents), 100), "%d.%02d" % divmod(max(cents), 100)]
    ok = ok and line[1:5] == want and abs(float(line[5]) - sum(cents) / len(cents) / 100) <= 0.005 + 1e-9
    ok = ok and line[0] == sorted(acc)[k]
if not ok:
    print("oracle mismatch:", got)
    sys.exit(1)
PY


  # Oracle 2: filter (regex) + select + stable numeric sort, CSV out, byte-identical to python's csv writer.
  "$Dt" run "$TMP/big_query.toml" >"$TMP/query" || fail "big query rc"
  python3 - "$TMP/big.csv" "$TMP/want.query" <<'PY'
import csv, sys
rows = [r for r in csv.DictReader(open(sys.argv[1], newline="")) if r["region"][0] in "ns"]
rows.sort(key=lambda r: int(r["amount"].replace(".", "")), reverse=True)  # stable, like sort.stable_by
with open(sys.argv[2], "w", newline="") as f:
    w = csv.writer(f)
    w.writerow(["id", "region", "note", "amount"])
    for r in rows:
        w.writerow([r["id"], r["region"], r["note"], r["amount"]])
PY
  cmp -s "$TMP/want.query" "$TMP/query" || { fail "query oracle"; diff "$TMP/want.query" "$TMP/query" | head -4; }

  # Error paths: exit status and a message fragment on stderr.
  bad() { # want-rc, fragment, args...
    local want="$1" sub="$2"; shift 2
    local out; out="$("$Dt" "$@" 2>&1)"; local rc=$?
    if [ "$rc" != "$want" ] || ! grep -qF -- "$sub" <<<"$out"; then fail "$* : rc=$rc want $want '$sub' got: $out"; fi
  }
  printf 'a,b\n1,2\n3\n' > "$TMP/ragged.csv"
  printf 'a,a\n1,2\n' > "$TMP/dup.csv"
  printf 'a,,c\n1,2,3\n' > "$TMP/emptyhdr.csv"
  printf '' > "$TMP/empty.csv"
  bad 1 "record 3: expected 2 fields, found 1" check "$TMP/ragged.csv"
  bad 1 "duplicate column 'a'" check "$TMP/dup.csv"
  bad 1 "record 1: field 2 of the header is empty" check "$TMP/emptyhdr.csv"
  bad 1 "no header row" check "$TMP/empty.csv"
  bad 1 "No such file or directory" check "$TMP/nope.csv"
  bad 1 "config: input.file is required" run tests/datatool/sales.csv
  printf '[input]\nfile = "tests/datatool/sales.csv"\n[output]\nformat = "xml"\n' > "$TMP/badfmt.toml"
  bad 1 'output.format must be "text" or "csv", not "xml"' run "$TMP/badfmt.toml"
  printf '[input]\nfile = "tests/datatool/sales.csv"\n[group]\nby = "region"\n' > "$TMP/half.toml"
  bad 1 "group.by and group.value must be given together" run "$TMP/half.toml"
  printf '[input]\nfile = "tests/datatool/sales.csv"\n[filter]\ncolumn = "nope"\npattern = "x"\n' > "$TMP/col.toml"
  bad 1 "unknown column 'nope' in filter" run "$TMP/col.toml"
  printf '[input]\nfile = "tests/datatool/sales.csv"\n[group]\nby = "region"\nvalue = "product"\n' > "$TMP/text.toml"
  bad 1 "data row 1: 'widget' is not a number" run "$TMP/text.toml"
  printf '[input]\nfile = "%s"\n' "$TMP/ragged.csv" > "$TMP/rag.toml"
  bad 1 "record 3: expected 2 fields" run "$TMP/rag.toml"
  bad 2 "unknown command 'frob'" frob x
  bad 2 "usage: datatool" check
  bad 0 "usage: datatool" --help
done
[ $fails -eq 0 ] && echo "datatool: ok"
exit $fails
