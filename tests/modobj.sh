#!/usr/bin/env bash
# Module objects (docs/MODULE_OBJECT.md): build an object, build an app from it WITHOUT the module sources (both
# backends), staleness checks, corrupt and conflicting objects. usage: tests/modobj.sh
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="$ROOT/target/release/yerux"
T="$(mktemp -d /tmp/yerux_modobj.XXXXXX)"; trap 'rm -rf "$T"' EXIT
fails=0
note() { echo "FAIL: $*"; fails=$((fails+1)); }
expect() { # name want got
  [ "$2" = "$3" ] || { note "$1: want [$2] got [$3]"; }
}
S="$T/src"; A="$T/app"; mkdir -p "$S" "$A"
cp tests/modobj/geom.yx tests/modobj/util.yx "$S/"; cp tests/modobj/app.yx "$A/"

"$Y" obj build "$S/geom.yx" -o "$T/geom.yxo" >/dev/null || note "obj build"
info="$("$Y" obj info "$T/geom.yxo")"
case "$info" in *"fn area(w: i64, h: i64) -> i64"*"fn twice(x: i64) -> i64"*) ;; *) note "obj info lacks the interface: $info" ;; esac

# the app builds from the object alone (no geom.yx / util.yx next to it), C and native
expect "run c" "42" "$("$Y" run "$A/app.yx" --obj "$T/geom.yxo" 2>&1)"
"$Y" build "$A/app.yx" --obj "$T/geom.yxo" --native -o "$T/app_native" >/dev/null 2>&1 || note "native build"
expect "run native" "42" "$("$T/app_native" 2>&1)"
# without the object the import fails
"$Y" check "$A/app.yx" 2>&1 | grep -q "cannot find module 'geom'" || note "missing module not reported"

# check: fresh, body change (interface same), interface change
"$Y" obj check "$T/geom.yxo" --root "$S" >"$T/c1"; expect "check fresh rc" 0 $?
grep -q "geom.yx: up to date" "$T/c1" || note "check fresh text"
sed -i 's/x + x/2 * x/' "$S/util.yx"
"$Y" obj check "$T/geom.yxo" --root "$S" >"$T/c2"; expect "check body rc" 1 $?
grep -q "util.yx: stale (body changed, interface the same" "$T/c2" || note "check body text: $(cat "$T/c2")"
sed -i 's/fn twice(x: i64)/fn twice(x: i64, y: i64)/;s/2 \* x/2 * x + y/' "$S/util.yx"
"$Y" obj check "$T/geom.yxo" --root "$S" >"$T/c3"; expect "check iface rc" 1 $?
grep -q "util.yx: stale (interface changed" "$T/c3" || note "check iface text"

# corrupt object: flip one byte of a source / truncate / wrong magic
sed '0,/x + x/s//x - x/' "$T/geom.yxo" >"$T/bad.yxo"
"$Y" run "$A/app.yx" --obj "$T/bad.yxo" 2>&1 | grep -q "does not match its hash" || note "corrupt object accepted"
head -c 100 "$T/geom.yxo" >"$T/trunc.yxo"
"$Y" run "$A/app.yx" --obj "$T/trunc.yxo" 2>&1 | grep -q "truncated\|bad file header" || note "truncated object accepted"
echo hello >"$T/magic.yxo"
"$Y" run "$A/app.yx" --obj "$T/magic.yxo" 2>&1 | grep -q "bad magic" || note "bad magic accepted"

# two objects with the same module name but different bytes conflict
mkdir -p "$T/s2"; cp tests/modobj/util.yx "$T/s2/"; echo '// other' >>"$T/s2/util.yx"
"$Y" obj build "$T/s2/util.yx" -o "$T/util.yxo" >/dev/null
cp tests/modobj/util.yx "$S/util.yx"
"$Y" obj build "$S/geom.yx" -o "$T/geom2.yxo" >/dev/null
"$Y" run "$A/app.yx" --obj "$T/geom2.yxo" --obj "$T/util.yxo" 2>&1 | grep -q "differs between" || note "conflict not reported"

# an object with a type error is not built
mkdir -p "$T/s3"; printf 'pub fn f() -> i64 {\n  return "x"\n}\n' >"$T/s3/bad.yx"
"$Y" obj build "$T/s3/bad.yx" -o "$T/bad_src.yxo" >/dev/null 2>&1 && note "ill-typed module built"
[ -e "$T/bad_src.yxo" ] && note "object written for an ill-typed module"

[ $fails = 0 ] && echo "modobj: ok" || echo "modobj: $fails failure(s)"
exit $fails
