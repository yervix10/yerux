// Finding the `yerux` executable; kept free of the `vscode` module so tests can load it.
const { spawnSync } = require("child_process");

// Runs `<command> --version` and says whether it is a usable yerux.
//   { ok: true, version: "yerux 1.0.0" }
//   { ok: false, reason: "missing" }   the executable does not exist or cannot be run
//   { ok: false, reason: "other", detail } it ran but did not look like yerux
function probe(command) {
  const r = spawnSync(command, ["--version"], { encoding: "utf8", timeout: 10000 });
  if (r.error) {
    if (r.error.code === "ENOENT" || r.error.code === "EACCES") return { ok: false, reason: "missing" };
    return { ok: false, reason: "other", detail: r.error.message };
  }
  const out = (r.stdout || "").trim();
  if (r.status === 0 && /^yerux\b/.test(out)) return { ok: true, version: out };
  return { ok: false, reason: "other", detail: out.split("\n")[0] || `exit status ${r.status}` };
}

module.exports = { probe };
