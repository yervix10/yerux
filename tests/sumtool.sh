#!/usr/bin/env bash
# examples/real/sumtool.yx against coreutils/python oracles, plus its error paths (C and native backends).
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="$ROOT/target/release/yerux"
TMP="$(mktemp -d /tmp/yerux_sumtool.XXXXXX)"; trap 'rm -rf "$TMP"' EXIT
fails=0
head -c 300000 /dev/urandom > "$TMP/big.bin"          # several SHA-256 blocks, arbitrary bytes
printf 'SGVsbG8gYmFzZTY0IQ==\n' > "$TMP/b64.txt"
F="tests/sumtool/hello.txt tests/sumtool/empty.txt tests/sumtool/blob.bin $TMP/big.bin"
for be in c native; do
  flags=(); [ $be = native ] && flags=(--native)
  "$Y" build examples/real/sumtool.yx "${flags[@]}" -o "$TMP/st" >/dev/null 2>&1 || { echo "FAIL sumtool build ($be)"; exit 1; }
  same() { # label expected-file actual-cmd...
    local label="$1" want="$2"; shift 2
    "$@" >"$TMP/got" 2>"$TMP/err"; local rc=$?
    if [ $rc -ne 0 ] || ! cmp -s "$want" "$TMP/got"; then echo "FAIL sumtool ($be) $label (rc=$rc)"; diff "$want" "$TMP/got" | head -4; fails=$((fails+1)); fi
  }
  sha256sum $F > "$TMP/want.sha"; same sha256 "$TMP/want.sha" "$TMP/st" sha256 $F
  python3 - $F > "$TMP/want.crc" <<'PY'
import sys, zlib
for f in sys.argv[1:]:
    d = open(f, 'rb').read()
    print('%08x %10d %s' % (zlib.crc32(d), len(d), f))
PY
  same crc32 "$TMP/want.crc" "$TMP/st" crc32 $F
  base64 -w0 "$TMP/big.bin" > "$TMP/want.b64"; echo >> "$TMP/want.b64"; same base64 "$TMP/want.b64" "$TMP/st" base64 "$TMP/big.bin"
  xxd -p "$TMP/big.bin" | tr -d '\n' > "$TMP/want.hex"; echo >> "$TMP/want.hex"; same hex "$TMP/want.hex" "$TMP/st" hex "$TMP/big.bin"
  printf 'Hello base64!' > "$TMP/want.dec"; same decode "$TMP/want.dec" "$TMP/st" base64 --decode "$TMP/b64.txt"
  same check-ok <(sha256sum $F | sed 's/.*/&/' >/dev/null; { sha256sum $F | awk '{print $2": OK"}'; echo "4 files verified"; }) "$TMP/st" check "$TMP/want.sha"
  # failures
  printf 'x' >> "$TMP/big.bin"
  "$TMP/st" check -q "$TMP/want.sha" >"$TMP/out" 2>"$TMP/err"; rc=$?
  { [ $rc = 1 ] && grep -q "big.bin: FAILED" "$TMP/out" && ! grep -q "OK" "$TMP/out" && grep -q "1 computed checksums did NOT match" "$TMP/err"; } || { echo "FAIL sumtool ($be) check mismatch rc=$rc"; fails=$((fails+1)); }
  bad() { local want="$1" sub="$2"; shift 2; out="$("$TMP/st" "$@" 2>&1)"; rc=$?; if [ $rc != "$want" ] || ! grep -qF -- "$sub" <<<"$out"; then echo "FAIL sumtool ($be) $*: rc=$rc want $want '$sub' got: $out"; fails=$((fails+1)); fi; }
  bad 1 "No such file or directory" sha256 "$TMP/nope"
  bad 2 "unknown command 'frobnicate'" frobnicate
  bad 2 "sha256 needs at least one FILE" sha256
  bad 2 "check takes exactly one SUMFILE" check a b
  bad 2 "Usage: sumtool" 
  bad 0 "commands:" --help
  bad 1 "invalid base64" base64 --decode tests/sumtool/hello.txt
  printf 'not a sum line\n' > "$TMP/badsum"; bad 1 "improperly formatted line" check "$TMP/badsum"
done
[ $fails -eq 0 ] && echo "sumtool: ok"
exit $fails
