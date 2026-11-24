#!/usr/bin/env bash
# std.http.server: the parser/router/response unit tests (`yerux test`) on the C and the native backend, and a long
# fuzz run of the request parser and of a live server (the short one is the golden test http_server_fuzz).
cd "$(dirname "$0")/.." || exit 2
Y="$PWD/${YERUX:-target/release/yerux}"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fail=0
for flags in "" "--native"; do
  "$Y" test tests/http_server/parse.yx $flags >"$T/out" 2>&1 || { echo "FAIL parser tests ($flags)"; tail -15 "$T/out"; fail=1; }
done
"$Y" build tests/http_server_fuzz.yx --native -o "$T/fuzz" >/dev/null 2>"$T/err" || { echo "FAIL building the fuzzer"; cat "$T/err"; exit 1; }
out=$(timeout 300 "$T/fuzz" 15000 424242 2>&1) || { echo "FAIL long fuzz run exited with $?"; echo "$out" | tail -5; fail=1; }
echo "$out" | grep -q "invariant violations: 0" || { echo "FAIL fuzz invariants: $out"; fail=1; }
echo "$out" | grep -q "verdicts that changed with more bytes: 0" || { echo "FAIL fuzz verdicts changed: $out"; fail=1; }
echo "$out" | grep -q "differ from the parser's verdict 0, healthy afterwards 1" || { echo "FAIL fuzz socket phase: $out"; fail=1; }
[ $fail = 0 ] && echo "http_server: ok ($(echo "$out" | head -1))"
exit $fail
