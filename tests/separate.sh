#!/usr/bin/env bash
# Separate compilation of the C backend (--separate, docs/MODULE_OBJECT.md stage 2): one object per user module,
# unchanged modules are reused from the cache, results equal the one-unit build. usage: tests/separate.sh
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="$ROOT/target/release/yerux"
T="$(mktemp -d /tmp/yerux_sep.XXXXXX)"; trap 'rm -rf "$T"' EXIT
fails=0
note() { echo "FAIL: $*"; fails=$((fails+1)); }
cp tests/modobj/geom.yx tests/modobj/util.yx tests/modobj/app.yx "$T/"
export YERUX_CACHE="$T/cache"
b() { "$Y" build "$T/app.yx" --separate -o "$T/app" 2>&1 | grep "^separate:"; }

r="$(b)"; case "$r" in *"3 unit(s), 0 reused"*) ;; *) note "first build: $r" ;; esac
[ "$("$T/app")" = 42 ] || note "first run"
r="$(b)"; case "$r" in *"3 unit(s), 3 reused"*) ;; *) note "no-change rebuild: $r" ;; esac
# body of one module changes: only its unit is compiled again
sed -i 's/return x + x/return 2 * x/' "$T/util.yx"
r="$(b)"; case "$r" in *"3 unit(s), 2 reused"*) ;; *) note "util changed: $r" ;; esac
[ "$("$T/app")" = 42 ] || note "run after util change"
# only main changes: both modules are reused
sed -i 's/geom.area(6, 7)/geom.area(7, 7)/' "$T/app.yx"
r="$(b)"; case "$r" in *"3 unit(s), 2 reused"*) ;; *) note "main changed: $r" ;; esac
[ "$("$T/app")" = 49 ] || note "run after main change: $("$T/app")"
# the separate build and the one-unit build agree
"$Y" build "$T/app.yx" -o "$T/app1" >/dev/null 2>&1
[ "$("$T/app")" = "$("$T/app1")" ] || note "separate and single-unit builds differ"
# with module objects instead of sources
"$Y" obj build "$T/geom.yx" -o "$T/geom.yxo" >/dev/null
mkdir "$T/o"; cp "$T/app.yx" "$T/o/"
r="$("$Y" build "$T/o/app.yx" --obj "$T/geom.yxo" --separate -o "$T/o/app" 2>&1 | grep "^separate:")"
case "$r" in *"unit(s)"*) ;; *) note "object + separate: $r" ;; esac
[ "$("$T/o/app")" = 49 ] || note "object + separate run"
# programs with generated helpers (drop glue, dyn tables) still build and run
"$Y" build tests/drop_basic.yx --separate -o "$T/d" >/dev/null 2>&1 && "$T/d" | diff -q - tests/expected/drop_basic.out >/dev/null || note "drop_basic"
"$Y" build tests/dyn_shapes.yx --separate -o "$T/d2" >/dev/null 2>&1 && "$T/d2" | diff -q - tests/expected/dyn_shapes.out >/dev/null || note "dyn_shapes"
[ $fails = 0 ] && echo "separate: ok" || echo "separate: $fails failure(s)"
exit $fails
