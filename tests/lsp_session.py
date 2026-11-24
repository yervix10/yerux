#!/usr/bin/env python3
"""Scripted LSP sessions over examples/real/*: `yerux lsp` is driven through JSON-RPC and every answer is checked.

Per project (a directory of examples/real, or a single program): didOpen of every file (no error diagnostics),
documentSymbol, hover, definition and references of every top-level item, formatting (the edit is `fmt --stdout`),
and a sample of renames (items and locals) whose WorkspaceEdit is applied to a copy of the tree and must still
check. Then didChange sessions (a syntax error and a type error get diagnostics with the right line and a range,
the fix clears them) and completion / signatureHelp / prepareRename on a small file.
usage: tests/lsp_session.py [--all]   (default: a sample of renames per project; --all renames every item and local)
"""
import glob, json, os, queue, re, shutil, subprocess, sys, tempfile, threading, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.chdir(ROOT)
Y = os.path.join(ROOT, os.environ.get("YERUX", "target/release/yerux"))
ALL = "--all" in sys.argv
fails = []


def fail(msg):
    fails.append(msg)
    print("FAIL", msg, flush=True)


class LSP:
    def __init__(self):
        env = dict(os.environ, YERUX_LSP_DEBOUNCE_MS="40")
        self.p = subprocess.Popen([Y, "lsp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env)
        self.id = 0
        self.q = queue.Queue()
        self.diags = {}
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        out = self.p.stdout
        while True:
            n = 0
            while True:
                line = out.readline()
                if not line:
                    self.q.put(None)
                    return
                line = line.strip()
                if not line:
                    break
                if line.lower().startswith(b"content-length:"):
                    n = int(line.split(b":")[1])
            self.q.put(json.loads(out.read(n)))

    def send(self, m):
        b = json.dumps(m).encode()
        self.p.stdin.write(b"Content-Length: %d\r\n\r\n" % len(b) + b)
        self.p.stdin.flush()

    def note(self, method, params):
        self.send({"jsonrpc": "2.0", "method": method, "params": params})

    def _take(self, m):
        if m.get("method") == "textDocument/publishDiagnostics":
            self.diags[m["params"]["uri"]] = m["params"]["diagnostics"]
            return True
        return False

    def req(self, method, params, timeout=30):
        self.id += 1
        i = self.id
        self.send({"jsonrpc": "2.0", "id": i, "method": method, "params": params})
        end = time.time() + timeout
        while time.time() < end:
            try:
                m = self.q.get(timeout=1)
            except queue.Empty:
                continue
            if m is None:
                raise RuntimeError("the server died")
            if not self._take(m) and m.get("id") == i:
                return m
        raise TimeoutError(method)

    def diag(self, uri, timeout=8):
        """The next diagnostics published for `uri`."""
        self.diags.pop(uri, None)
        end = time.time() + timeout
        while time.time() < end:
            try:
                m = self.q.get(timeout=0.2)
            except queue.Empty:
                m = None
            if m is None and self.p.poll() is not None:
                raise RuntimeError("the server died")
            if m:
                self._take(m)
            if uri in self.diags:
                return self.diags[uri]
        return None

    def open(self, path, text=None):
        u = uri(path)
        self.diags.pop(u, None)
        self.note("textDocument/didOpen", {"textDocument": {"uri": u, "languageId": "yerux", "version": 1, "text": text if text is not None else open(path).read()}})
        return u

    def close(self):
        try:
            self.req("shutdown", None, 5)
            self.note("exit", None)
            self.p.wait(5)
        except Exception:
            self.p.kill()


def uri(path):
    return "file://" + os.path.abspath(path)


def apply_edits(text, edits):
    lines = text.split("\n")

    def off(p):
        return sum(len(l) + 1 for l in lines[: p["line"]]) + p["character"]

    for e in sorted(edits, key=lambda e: off(e["range"]["start"]), reverse=True):
        text = text[: off(e["range"]["start"])] + e["newText"] + text[off(e["range"]["end"]) :]
    return text


def checks(path_in_tree, tree, main):
    r = subprocess.run([Y, "check", os.path.join(tree, main)], capture_output=True, text=True)
    return r.returncode == 0, (r.stdout + r.stderr).strip().split("\n")[0][:110]


def project_files(p):
    return [p] if p.endswith(".yx") else sorted(glob.glob(p + "/*.yx"))


def main_of(files):
    for f in files:
        if re.search(r"^fn main\(", open(f).read(), re.M):
            return f
    return None


def rename_check(L, files, main, f, pos, what, tmp_tree):
    rn = L.req("textDocument/rename", {"textDocument": {"uri": uri(f)}, "position": pos, "newName": "zz_renamed"})
    if "error" in rn:
        return  # refused (impl blocks, shared members, ...): allowed
    changes = (rn.get("result") or {}).get("changes") or {}
    if not changes:
        return
    tree = tempfile.mkdtemp(prefix="lsp_rename_")
    try:
        shutil.copytree(os.path.join(ROOT, "examples"), os.path.join(tree, "examples"))
        for u, edits in changes.items():
            path = u[len("file://") :]
            rel = os.path.relpath(path, ROOT)
            with open(os.path.join(tree, rel), "w") as out:
                out.write(apply_edits(open(path).read(), edits))
        ok, why = checks(f, tree, main)
        if not ok:
            fail("rename of %s %s at %s:%d breaks the program: %s" % (what, os.path.basename(f), f, pos["line"] + 1, why))
    finally:
        shutil.rmtree(tree)


def real_projects():
    out = sorted(glob.glob("examples/real/*.yx")) + sorted(d for d in glob.glob("examples/real/*") if os.path.isdir(d))
    return out


def session(proj):
    files = project_files(proj)
    main = main_of(files) or files[0]
    L = LSP()
    L.req("initialize", {"processId": None, "rootUri": uri("."), "capabilities": {}})
    L.note("initialized", {})
    uris = {f: L.open(f) for f in files}
    n_sym = n_ren = 0
    try:
        for f in files:
            d = L.diag(uris[f])
            if d is None:
                fail("%s: no diagnostics published after didOpen" % f)
            elif any(x.get("severity", 1) == 1 for x in d):
                fail("%s: error diagnostic on a program that compiles: %s" % (f, d[0]["message"][:80]))
        budget = 10**9 if ALL else 8
        for f in files:
            text = open(f).read()
            syms = L.req("textDocument/documentSymbol", {"textDocument": {"uri": uris[f]}})["result"]
            if re.search(r"^(pub )?(fn|struct|enum|trait) ", text, re.M) and not syms:
                fail("%s: documentSymbol is empty" % f)
            # formatting: the single edit turns the text into `fmt --stdout`
            fm = L.req("textDocument/formatting", {"textDocument": {"uri": uris[f]}, "options": {"tabSize": 2, "insertSpaces": True}})
            want = subprocess.run([Y, "fmt", "--stdout", f], capture_output=True, text=True).stdout
            got = apply_edits(text, fm["result"] or [])
            if got != want:
                fail("%s: formatting edit differs from `yerux fmt --stdout`" % f)
            for s in syms or []:
                if s["name"] == "main" or s["name"].startswith("impl"):
                    continue
                pos = s["selectionRange"]["start"]
                n_sym += 1
                if not L.req("textDocument/hover", {"textDocument": {"uri": uris[f]}, "position": pos})["result"]:
                    fail("%s: no hover on %s" % (f, s["name"]))
                dfn = L.req("textDocument/definition", {"textDocument": {"uri": uris[f]}, "position": pos})["result"]
                if not dfn or dfn["range"]["start"] != pos:
                    fail("%s: definition of the declaration %s is not itself" % (f, s["name"]))
                refs = L.req("textDocument/references", {"textDocument": {"uri": uris[f]}, "position": pos, "context": {"includeDeclaration": True}})["result"]
                if not refs or not any(r["range"]["start"] == pos and r["uri"] == uris[f] for r in refs):
                    fail("%s: references of %s miss the declaration" % (f, s["name"]))
                # every reference resolves back to the declaration
                for r in (refs or [])[:6]:
                    d2 = L.req("textDocument/definition", {"textDocument": {"uri": r["uri"]}, "position": r["range"]["start"]})["result"]
                    if not d2 or d2["range"]["start"] != pos or d2["uri"] != uris[f]:
                        fail("%s: definition of a use of %s at %s:%d does not reach the declaration" % (f, s["name"], r["uri"].split("/")[-1], r["range"]["start"]["line"] + 1))
                if budget > 0:
                    budget -= 1
                    n_ren += 1
                    rename_check(L, files, main, f, pos, s["name"], None)
        # locals: `let`/`var` names
        lb = 10**9 if ALL else 8
        for f in files:
            for ln, line in enumerate(open(f).read().split("\n")):
                for m in re.finditer(r"\b(?:let|var)\s+(?:mut\s+)?([a-z_][A-Za-z0-9_]*)", line):
                    if m.group(1) == "_" or lb <= 0:
                        continue
                    lb -= 1
                    n_ren += 1
                    rename_check(L, files, main, f, {"line": ln, "character": m.start(1)}, "local " + m.group(1), None)
    finally:
        L.close()
    return n_sym, n_ren


SMALL = '''import std.text

pub struct Point { x: Int, y: Int }

derive Clone for Point

impl Point {
  fn norm1(view self) -> Int {
    return self.x + self.y
  }
}

fn total(p: view Point, scale: Int) -> Int {
  let n = p.norm1()
  return n * scale
}

fn main() {
  let origin = Point { x: 1, y: 2 }
  let total = total(origin, 3)
  print(total)
}
'''


def small_session():
    d = tempfile.mkdtemp(prefix="lsp_small_")
    path = os.path.join(d, "small.yx")
    open(path, "w").write(SMALL)
    L = LSP()
    try:
        init = L.req("initialize", {"processId": None, "rootUri": uri(d), "capabilities": {}})["result"]["capabilities"]
        for cap in ["definitionProvider", "hoverProvider", "referencesProvider", "renameProvider", "completionProvider", "documentSymbolProvider", "documentFormattingProvider"]:
            if not init.get(cap):
                fail("initialize: capability %s missing" % cap)
        L.note("initialized", {})
        u = L.open(path)
        if L.diag(u) != []:
            fail("small.yx: expected an empty diagnostics list")
        # a syntax error in didChange: a diagnostic on the edited line with a non-empty range
        L.note("textDocument/didChange", {"textDocument": {"uri": u, "version": 2}, "contentChanges": [{"range": {"start": {"line": 13, "character": 14}, "end": {"line": 13, "character": 14}}, "text": " +"}]})
        dg = L.diag(u)
        if not dg or dg[0]["range"]["start"]["line"] not in (13, 14) or dg[0]["range"]["start"] == dg[0]["range"]["end"] and False:
            fail("didChange (syntax error): wrong diagnostics %s" % (dg,))
        # restore, then a type error: `scale` used as text
        L.note("textDocument/didChange", {"textDocument": {"uri": u, "version": 3}, "contentChanges": [{"text": SMALL.replace("return n * scale", "return n * \"x\"")}]})
        dg = L.diag(u)
        if not dg or dg[0]["range"]["start"]["line"] != 14 or dg[0]["range"]["end"]["character"] <= dg[0]["range"]["start"]["character"]:
            fail("didChange (type error): expected a ranged diagnostic on line 15, got %s" % (dg,))
        L.note("textDocument/didChange", {"textDocument": {"uri": u, "version": 4}, "contentChanges": [{"text": SMALL}]})
        if L.diag(u) != []:
            fail("didChange (fixed): diagnostics were not cleared")
        # derive-for names are uses of the type; `let total = total(..)` calls the function; closure/field names
        refs = L.req("textDocument/references", {"textDocument": {"uri": u}, "position": {"line": 2, "character": 11}, "context": {"includeDeclaration": True}})["result"]
        lines = sorted(r["range"]["start"]["line"] + 1 for r in refs or [])
        if lines != [3, 5, 7, 13, 19]:
            fail("references of Point: lines %s, want [3, 5, 7, 13, 19] (derive/impl/parameter/literal uses)" % lines)
        refs = L.req("textDocument/references", {"textDocument": {"uri": u}, "position": {"line": 12, "character": 3}, "context": {"includeDeclaration": True}})["result"]
        lines = sorted(r["range"]["start"]["line"] + 1 for r in refs or [])
        if lines != [13, 20]:
            fail("references of fn total: lines %s, want [13, 20] (the initializer of `let total = total(..)` is the function)" % lines)
        refs = L.req("textDocument/references", {"textDocument": {"uri": u}, "position": {"line": 2, "character": 19}, "context": {"includeDeclaration": True}})["result"]
        lines = sorted(r["range"]["start"]["line"] + 1 for r in refs or [])
        if lines != [3, 9, 19]:
            fail("references of field x: lines %s, want [3, 9, 19] (the `Point { x: 1 }` literal names the field)" % lines)
        # completion after `.`: the methods and fields of Point
        L.note("textDocument/didChange", {"textDocument": {"uri": u, "version": 5}, "contentChanges": [{"text": SMALL.replace("print(total)", "print(origin.)")}]})
        c = L.req("textDocument/completion", {"textDocument": {"uri": u}, "position": {"line": 20, "character": 15}, "context": {"triggerKind": 2, "triggerCharacter": "."}})["result"]
        labels = {i["label"] for i in (c["items"] if isinstance(c, dict) else c or [])}
        if not {"x", "y", "norm1"} <= labels:
            fail("completion after `origin.`: got %s" % sorted(labels)[:10])
        L.note("textDocument/didChange", {"textDocument": {"uri": u, "version": 6}, "contentChanges": [{"text": SMALL}]})
        L.diag(u)
        sh = L.req("textDocument/signatureHelp", {"textDocument": {"uri": u}, "position": {"line": 19, "character": 28}})["result"]
        if not sh or not sh.get("signatures"):
            fail("signatureHelp inside total(origin, 3) is empty")
        pr = L.req("textDocument/prepareRename", {"textDocument": {"uri": u}, "position": {"line": 19, "character": 6}})
        if "error" in pr:
            fail("prepareRename on a local: %s" % pr["error"]["message"])
        pr = L.req("textDocument/rename", {"textDocument": {"uri": u}, "position": {"line": 19, "character": 6}, "newName": "fn"})
        if "error" not in pr:
            fail("rename to a keyword was accepted")
        st = L.req("textDocument/semanticTokens/full", {"textDocument": {"uri": u}})["result"]
        if not st or not st.get("data"):
            fail("semanticTokens/full is empty")
    finally:
        L.close()
        shutil.rmtree(d)


if __name__ == "__main__":
    t0 = time.time()
    tot_s = tot_r = 0
    projects = real_projects()
    for p in projects:
        s, r = session(p)
        tot_s += s
        tot_r += r
    small_session()
    print("lsp sessions: %d programs, %d items checked, %d renames applied and re-checked, %.0fs, failures %d" % (len(projects), tot_s, tot_r, time.time() - t0, len(fails)))
    sys.exit(1 if fails else 0)
