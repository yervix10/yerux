#!/usr/bin/env bash
# The VS Code extension's static files are valid JSON, its grammar's regexes compile, and its client parses.
set -euo pipefail
cd "$(dirname "$0")/../editors/vscode"
node -e '
const fs = require("fs");
for (const f of ["package.json", "language-configuration.json", "syntaxes/yerux.tmLanguage.json"]) JSON.parse(fs.readFileSync(f, "utf8"));
const g = require("./syntaxes/yerux.tmLanguage.json");
let n = 0;
(function walk(o) {
  if (o && typeof o === "object") for (const k in o) {
    if (["match", "begin", "end"].includes(k) && typeof o[k] === "string") { new RegExp(o[k]); n++; } else walk(o[k]);
  }
})(g);
if (n < 20) throw new Error("grammar looks empty");
// every keyword of the compiler is highlighted by some rule
const text = JSON.stringify(g);
for (const kw of ["fn","let","var","if","else","while","return","move","view","mut","true","false","struct","enum","match","defer","trait","dyn","impl","for","in","break","continue","unsafe","spawn","parallel","scope","import","pub"])
  if (!new RegExp("\\b" + kw + "\\b").test(text)) throw new Error("keyword not highlighted: " + kw);
const p = require("./package.json");
if (p.main !== "./src/extension.js" || !p.contributes.grammars.length) throw new Error("bad package.json");
'
node --check src/extension.js
# the client must not append `--stdio` to the command (older yerux binaries reject it), and `yerux lsp --stdio` itself works
! grep -q "TransportKind" src/extension.js
Y="${Y:-../../target/release/yerux}"
a="$("$Y" lsp </dev/null 2>&1; echo $?)"; b="$("$Y" lsp --stdio </dev/null 2>&1; echo $?)"
[ "$a" = "$b" ] || { echo "yerux lsp --stdio differs from yerux lsp: [$a] vs [$b]"; exit 1; }
# store metadata: license, repository link, and the probe that tells a missing `yerux` from a real one
node -e '
const p = require("./package.json");
if (p.license !== "MIT OR Apache-2.0") throw new Error("license must match the project: " + p.license);
if (!p.repository || !/github\.com\/yervix10\/yerux/.test(p.repository.url)) throw new Error("repository link missing");
if (!p.bugs || !p.homepage || !p.icon) throw new Error("bugs, homepage and icon are required");
const fs = require("fs");
for (const f of [p.icon, "LICENSE", "LICENSE-APACHE", "README.md"]) if (!fs.existsSync(f)) throw new Error("missing file: " + f);
const { probe } = require("./src/server");
if (probe("/no/such/yerux-binary").reason !== "missing") throw new Error("a missing binary must be reported as missing");
const real = probe(process.env.Y || "../../target/release/yerux");
if (!real.ok || !/^yerux /.test(real.version)) throw new Error("probe cannot recognise yerux: " + JSON.stringify(real));
'
echo "vscode extension files ok"
