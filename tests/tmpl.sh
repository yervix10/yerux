#!/usr/bin/env bash
# examples/real/tmpl: unit tests on both backends; a page template and its JSON data rendered to an exact expected
# output on both backends; the error messages and exit statuses; a seeded corpus of random templates and documents
# rendered by both backends (identical output, status 0 or 1). Exit 0 when everything passes.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
D=examples/real/tmpl
fail=0
for flag in "" --native; do
  "$Y" test $D/tests.yx $flag >"$T/t.out" 2>&1 || { echo "FAIL tmpl unit tests ($flag)"; tail -5 "$T/t.out"; fail=1; }
done
"$Y" build $D/main.yx -o "$T/tmpl-c" >"$T/b1.out" 2>&1 || { echo "FAIL tmpl build (c)"; cat "$T/b1.out"; fail=1; }
"$Y" build $D/main.yx --native -o "$T/tmpl-n" >"$T/b2.out" 2>&1 || { echo "FAIL tmpl build (native)"; cat "$T/b2.out"; fail=1; }
[ $fail = 0 ] || exit 1

cat >"$T/page.html" <<'TPL'
<h1>{{ title }}</h1>
{{! the list is optional }}
<ul>
{{#items}}  <li>{{name}}: {{price}} {{{raw}}} {{tags}}</li>
{{/items}}</ul>
{{^items}}no items{{/items}}
{{#owner}}by {{first}} {{last}} ({{.}}){{/owner}}
{{#flag}}flag is {{title}}{{/flag}}
TPL
cat >"$T/data.json" <<'JSON'
{"title": "Shop & <Co>", "items": [{"name": "pen \"fine\"", "price": 1.5, "raw": "<b>", "tags": ["a", "b"]}, {"name": "ink", "price": 3, "raw": "x", "tags": []}], "owner": {"first": "Ann", "last": "Lee"}, "flag": true}
JSON
cat >"$T/page.want" <<'OUT'
<h1>Shop &amp; &lt;Co&gt;</h1>

<ul>
  <li>pen &quot;fine&quot;: 1.5 <b> [&quot;a&quot;,&quot;b&quot;]</li>
  <li>ink: 3 x []</li>
</ul>

by Ann Lee ({&quot;first&quot;:&quot;Ann&quot;,&quot;last&quot;:&quot;Lee&quot;})
flag is Shop &amp; &lt;Co&gt;
OUT
for b in c n; do
  "$T/tmpl-$b" "$T/page.html" "$T/data.json" >"$T/page.$b" 2>"$T/page.$b.err"; st=$?
  [ $st = 0 ] || { echo "FAIL tmpl page status ($b): $st"; cat "$T/page.$b.err"; fail=1; }
  cmp -s "$T/page.want" "$T/page.$b" || { echo "FAIL tmpl page output ($b)"; diff "$T/page.want" "$T/page.$b"; fail=1; }
done

# Errors: stderr is checked exactly, status 1 (2 for usage).
printf '{{#a}}x\n{{/b}}' >"$T/mismatch.tpl"
printf 'ok\n{{#a}}open' >"$T/open.tpl"
printf 'hi {{who}}' >"$T/who.tpl"
printf '{"a": 1,\n  "b" 2}' >"$T/bad.json"
err() { # name, want status, want stderr (printed with the template path), args...
  local name=$1 want=$2 wanterr=$3; shift 3
  for b in c n; do
    "$T/tmpl-$b" "$@" >"$T/e.out.$b" 2>"$T/e.err.$b"; local st=$?
    [ "$st" = "$want" ] || { echo "FAIL tmpl $name status ($b): $st, want $want"; cat "$T/e.err.$b"; fail=1; }
    [ "$(cat "$T/e.err.$b")" = "$wanterr" ] || { echo "FAIL tmpl $name message ($b)"; cat "$T/e.err.$b"; fail=1; }
  done
}
err mismatch 1 "tmpl: $T/mismatch.tpl: line 2: closing tag 'b' does not match section 'a'" "$T/mismatch.tpl" "$T/data.json"
err open 1 "tmpl: $T/open.tpl: line 2: section 'a' is never closed" "$T/open.tpl" "$T/data.json"
printf '{}' >"$T/empty.json"
err unknown 1 "tmpl: $T/who.tpl: line 1: unknown name 'who'" "$T/who.tpl" "$T/empty.json"
err badjson 1 "tmpl: $T/bad.json:2:7: expected ':' after the key" "$T/who.tpl" "$T/bad.json"
err usage 2 "tmpl: usage: tmpl TEMPLATE [DATA]"
err nofile 1 "tmpl: cannot read $T/none.tpl: open '$T/none.tpl': No such file or directory (os error 2)" "$T/none.tpl" "$T/data.json"

# Seeded corpus: random templates over random documents; both backends agree and never fail otherwise.
python3 - "$T" 150 <<'PY'
import json, os, random, sys
T, N = sys.argv[1], int(sys.argv[2])
rng = random.Random(3)
names = ["a", "b", "c.d", "e"]
def doc(depth):
    k = rng.randrange(7 if depth < 3 else 4)
    if k == 0: return None
    if k == 1: return rng.random() < 0.5
    if k == 2: return rng.randrange(-3, 4)
    if k == 3: return rng.choice(["", "x<y", "q\"t", "&amp;"])
    if k == 4: return [doc(depth + 1) for _ in range(rng.randrange(3))]
    return {n: doc(depth + 1) for n in rng.sample(["a", "b", "c", "e"], rng.randrange(4))}
def tpl(depth):
    out = []
    for _ in range(rng.randrange(1, 5)):
        k = rng.randrange(6 if depth < 2 else 4)
        if k == 0: out.append("t ")
        elif k == 1: out.append("{{" + rng.choice(names) + "}}")
        elif k == 2: out.append("{{{" + rng.choice(names) + "}}}")
        elif k == 3: out.append("{{.}}")
        elif k == 4:
            n = rng.choice(names[:4]); out.append("{{#" + n + "}}" + tpl(depth + 1) + "{{/" + n + "}}")
        else:
            n = rng.choice(names[:4]); out.append("{{^" + n + "}}" + tpl(depth + 1) + "{{/" + n + "}}")
    return "".join(out)
for i in range(N):
    with open(os.path.join(T, f"r{i}.tpl"), "w") as f: f.write(tpl(0))
    root = {n: doc(1) for n in ["a", "b", "c", "e"]}
    with open(os.path.join(T, f"r{i}.json"), "w") as f: json.dump(root, f)
PY
mism=0; ok=0; err1=0
for i in $(seq 0 149); do
  "$T/tmpl-c" "$T/r$i.tpl" "$T/r$i.json" >"$T/rc.out" 2>"$T/rc.err"; sc=$?
  "$T/tmpl-n" "$T/r$i.tpl" "$T/r$i.json" >"$T/rn.out" 2>"$T/rn.err"; sn=$?
  if [ "$sc" != "$sn" ] || ! cmp -s "$T/rc.out" "$T/rn.out" || ! cmp -s "$T/rc.err" "$T/rn.err"; then
    mism=$((mism + 1)); [ $mism -le 3 ] && echo "tmpl corpus differs on r$i ($sc/$sn)"; continue
  fi
  case $sc in 0) ok=$((ok + 1));; 1) err1=$((err1 + 1));; *) mism=$((mism + 1)); echo "tmpl corpus status $sc on r$i";; esac
done
[ $mism = 0 ] || { echo "FAIL tmpl corpus: $mism problems"; fail=1; }
[ $fail = 0 ] && echo "tmpl: ok ($ok rendered, $err1 template or data errors, both backends identical)"
exit $fail
