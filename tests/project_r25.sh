#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

mkdir -p "$T/project/src/models" "$T/project/tests"
cat >"$T/project/yerux.toml" <<'TOML'
[package]
name = "empty_deps"
version = "0.1.0"

[dependencies]
TOML
cat >"$T/project/src/models/task.yx" <<'SOURCE'
pub fn answer() -> Int { return 42 }
SOURCE
cat >"$T/project/src/main.yx" <<'SOURCE'
import models.task
fn main() { print(task.answer()) }
SOURCE
cat >"$T/project/tests/smoke.yx" <<'SOURCE'
import models.task
test "project import" { assert(task.answer() == 42) }
SOURCE

cd "$T/project"
"$Y" check src/main.yx >/dev/null
printf 'project mode without dependencies: check ok\n' >>"$T/result.out"
"$Y" build src/main.yx -o "$T/project-bin" >/dev/null
[ "$("$T/project-bin")" = 42 ]
printf 'project mode: build and run ok\n' >>"$T/result.out"
"$Y" test >"$T/test.out"
printf 'project mode: test ok\n' >>"$T/result.out"

python3 - "$Y" "$T/project" >>"$T/result.out" <<'PY'
import json
import queue
import subprocess
import sys
import threading
import time
from pathlib import Path

yerux, root = sys.argv[1:]
main = Path(root, "src/main.yx")
proc = subprocess.Popen(
    [yerux, "lsp"], cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
)
messages = queue.Queue()

def reader():
    while True:
        headers = {}
        while True:
            line = proc.stdout.readline()
            if not line:
                messages.put(None)
                return
            if line in (b"\r\n", b"\n"):
                break
            key, value = line.decode().split(":", 1)
            headers[key.lower()] = value.strip()
        messages.put(json.loads(proc.stdout.read(int(headers["content-length"]))))

threading.Thread(target=reader, daemon=True).start()

def send(message):
    body = json.dumps(message).encode()
    proc.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    proc.stdin.flush()

def receive_id(want, timeout=10):
    end = time.time() + timeout
    while time.time() < end:
        try:
            message = messages.get(timeout=0.25)
        except queue.Empty:
            continue
        if message is None:
            raise RuntimeError("LSP exited")
        if message.get("id") == want:
            return message
    raise TimeoutError("LSP request " + str(want))

try:
    send({"jsonrpc": "2.0", "id": 1, "method": "initialize",
          "params": {"processId": None, "rootUri": Path(root).as_uri(), "capabilities": {}}})
    receive_id(1)
    send({"jsonrpc": "2.0", "method": "initialized", "params": {}})
    uri = main.as_uri()
    send({"jsonrpc": "2.0", "method": "textDocument/didOpen",
          "params": {"textDocument": {"uri": uri, "languageId": "yerux", "version": 1,
                                      "text": main.read_text()}}})
    end = time.time() + 10
    diagnostics = None
    while time.time() < end:
        try:
            message = messages.get(timeout=0.25)
        except queue.Empty:
            continue
        if message is None:
            raise RuntimeError("LSP exited")
        if (message.get("method") == "textDocument/publishDiagnostics"
                and message.get("params", {}).get("uri") == uri):
            diagnostics = message["params"]["diagnostics"]
            break
    if diagnostics is None:
        raise TimeoutError("no LSP diagnostics")
    errors = [d["message"] for d in diagnostics if d.get("severity") == 1]
    if errors:
        raise RuntimeError("; ".join(errors))
    print("project mode without dependencies: LSP diagnostics clean")
finally:
    try:
        send({"jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": None})
        receive_id(2, 3)
        send({"jsonrpc": "2.0", "method": "exit", "params": None})
        proc.wait(timeout=3)
    except Exception:
        proc.kill()
PY

cmp "$T/result.out" "$ROOT/tests/expected/project_r25.out"
echo "project_r25: ok"
