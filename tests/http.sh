#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
Y="$PWD/target/release/yerux"
root=examples/real/http
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
for backend in c native asan; do
  for app in server client; do
    exe="$tmp/$app"
    case "$backend" in
      c) "$Y" build "$root/$app.yx" -o "$exe" >/dev/null ;;
      native) "$Y" build "$root/$app.yx" --native-debug-alloc -o "$exe" >/dev/null ;;
      asan)
        "$Y" c "$root/$app.yx" >"$tmp/$app.c"
        cc -g -O1 -w -fsanitize=address,undefined -fno-sanitize-recover=undefined \
          -fno-math-errno -ffp-contract=off -pthread -o "$exe" "$tmp/$app.c" -lm
        ;;
    esac
  done
  flags=()
  if [ "$backend" = native ]; then flags+=(--check-alloc); fi
  python3 tests/net_http.py "$tmp/server" "$tmp/client" "$root/public/index.html" "${flags[@]}" >"$tmp/out"
  diff -u tests/expected/http_loopback.out "$tmp/out"
done
echo "HTTP loopback checks passed (C, native allocation tracking, ASan/UBSan)"
