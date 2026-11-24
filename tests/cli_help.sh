#!/usr/bin/env bash
# yerux --help, help COMMAND and COMMAND --help agree with docs/CLI.md; wrong command lines exit 2 with a usage on stderr.
cd "$(dirname "$0")/.." || exit 2
Y="$PWD/${YERUX:-target/release/yerux}"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fails=0
note() { echo "$1"; fails=$((fails+1)); }

out=$("$Y" --help 2>"$T/err"); [ $? = 0 ] || note "FAIL --help should exit 0"
[ -s "$T/err" ] && note "FAIL --help wrote to stderr"
"$Y" -h >/dev/null 2>&1 || note "FAIL -h"
"$Y" help >/dev/null 2>&1 || note "FAIL help"
cmds=$(echo "$out" | sed -n '/^commands:/,/^$/p' | sed '1d;$d' | awk '{print $1}')
[ "$(echo "$cmds" | wc -l)" -ge 15 ] || note "FAIL --help lists too few commands: $cmds"
for c in $cmds; do
  h=$("$Y" "$c" --help 2>"$T/err") || note "FAIL $c --help exit status"
  echo "$h" | grep -q "^usage: yerux" || note "FAIL $c --help has no usage line"
  [ "$h" = "$("$Y" help "$c")" ] || note "FAIL help $c differs from $c --help"
  [ -s "$T/err" ] && note "FAIL $c --help wrote to stderr"
  grep -q "^## \`$c\`" docs/CLI.md || note "FAIL docs/CLI.md has no section for $c"
done
# docs/CLI.md is the generated page
python3 scripts/gen_cli_doc.py "$T/CLI.md" && cmp -s "$T/CLI.md" docs/CLI.md || note "FAIL docs/CLI.md is not what scripts/gen_cli_doc.py writes (run it and commit)"

"$Y" >/dev/null 2>"$T/err"; [ $? = 2 ] || note "FAIL no arguments should exit 2"; grep -q '^usage:' "$T/err" || note "FAIL no usage on stderr"
"$Y" frobnicate >/dev/null 2>"$T/err"; [ $? = 2 ] || note "FAIL unknown command should exit 2"; grep -q "unknown command 'frobnicate'" "$T/err" || note "FAIL unknown command message"
"$Y" help frobnicate >/dev/null 2>&1; [ $? = 2 ] || note "FAIL help of an unknown command should exit 2"
"$Y" check >/dev/null 2>"$T/err"; [ $? = 2 ] || note "FAIL check without a file should exit 2"; grep -q '^usage: yerux check' "$T/err" || note "FAIL check usage"
"$Y" check /nonexistent.yx >/dev/null 2>&1; [ $? = 2 ] || note "FAIL unreadable file should exit 2"
for c in fmt doc test bench; do "$Y" $c --bogus-flag >/dev/null 2>"$T/err"; [ $? = 2 ] || note "FAIL $c with an unknown option should exit 2"; grep -q '^usage: yerux' "$T/err" || note "FAIL $c unknown option without a usage"; done

# `--help` after `--` belongs to the program
printf 'import std.env\nfn main() {\n  print(env.arg_count())\n  print(env.arg(1).unwrap())\n}\n' > "$T/p.yx"
r=$("$Y" run "$T/p.yx" -- --help 2>&1) && [ "$(echo "$r" | tail -1)" = "--help" ] || note "FAIL run -- --help did not reach the program: $r"
echo "cli help checks: failures $fails"
[ "$fails" = 0 ]
