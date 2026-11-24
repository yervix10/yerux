#!/usr/bin/env bash
# examples/real/chat: unit tests, then the server built on both backends. A fixed two-client session must match the
# golden transcript inline below; 20 seeded random three-client sessions must match a Python model of the protocol;
# the C and native transcripts must be identical; the client mode must match its golden. Prints "FAIL chat: ..." on
# failure. Only PIDs this script started are signalled.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"
PIDS=()
SRV=""
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
  rm -rf "$T"
}
trap cleanup EXIT
fail() { echo "FAIL chat: $*"; exit 1; }

proc_alive() { [ -r "/proc/$1/status" ] && ! grep -q '^State:[[:space:]]*Z' "/proc/$1/status"; }

# start_server BIN PORT MAX OUTFILE: sets SRV to the PID of the server this script started
start_server() {
  setsid nohup "$1" serve "$2" "$3" >"$4" 2>&1 &
  SRV=$!
  PIDS+=("$SRV")
  python3 "$T/drive.py" waitlisten "$2" || fail "server did not listen on port $2: $(cat "$4")"
}

# finish_server PID OUTFILE: the server must exit by itself (status 0) and print nothing
finish_server() {
  local i
  for ((i = 0; i < 100; i++)); do
    if ! proc_alive "$1"; then break; fi
    sleep 0.1
  done
  if proc_alive "$1"; then
    kill "$1" 2>/dev/null
    wait "$1" 2>/dev/null
    fail "server did not exit after its $3 connections closed"
  fi
  wait "$1"
  local rc=$?
  [ "$rc" = 0 ] || fail "server exit status $rc: $(cat "$2")"
  [ -s "$2" ] && fail "server printed on its output: $(cat "$2")"
  return 0
}

cat >"$T/drive.py" <<'PY'
# Session driver and protocol model for tests/chat.sh. The model is written from the protocol description only.
import random
import re
import socket
import sys
import time

NAMES = ["ann", "bob", "carl", "Dee", "x_1", "Q9", "_", "bad-name", "a b", "", "abcdefghijklmnop", "abcdefghijklmnopq"]
TEXTS = ["hi", "hello there", "  spaced  ", "ok?", "tab\tend", "12 34", "bye now", "x"]
JUNK = ["", "who", "WHO now", "nick ann", "MSG", "HELLO", "NICK", "QUIT now", "msg hi", "WHO\tx"]
VALID = re.compile(r"[A-Za-z0-9_]{1,16}\Z")

GOLDEN_STEPS = [
    ("A", "LINE", "BOGUS", "\n"),
    ("A", "LINE", "NICK ann", "\n"),
    ("B", "LINE", "NICK ann", "\n"),
    ("B", "LINE", "NICK bob", "\n"),
    ("A", "LINE", "MSG hello there", "\n"),
    ("B", "LINE", "WHO", "\n"),
    ("B", "LINE", "MSG reply", "\n"),
    ("A", "LINE", "WHO", "\n"),
    ("A", "QUIT", "QUIT", "\n"),
    ("B", "LINE", "WHO", "\n"),
    ("B", "QUIT", "QUIT", "\n"),
]


def gen(seed):
    rnd = random.Random(seed)
    names = ["A", "B", "C"]
    open_ = list(names)
    steps = []

    def ending():
        return "\r\n" if rnd.random() < 0.3 else "\n"

    for _ in range(rnd.randint(40, 70)):
        if not open_:
            break
        c = rnd.choice(open_)
        r = rnd.random()
        if r < 0.03:
            steps.append((c, "QUIT", "QUIT", ending()))
            open_.remove(c)
            continue
        if r < 0.30:
            text = "NICK " + rnd.choice(NAMES)
        elif r < 0.60:
            text = "MSG " + rnd.choice(TEXTS)
        elif r < 0.75:
            text = "WHO"
        else:
            text = rnd.choice(JUNK)
        steps.append((c, "LINE", text, ending()))
    for c in list(open_):
        steps.append((c, "QUIT", "QUIT", "\n"))
        open_.remove(c)
    return names, steps


def model(names, steps):
    joined = {}
    got = {c: [] for c in names}
    for c, kind, text, _ in steps:
        bcast = []
        if kind == "QUIT":
            reply = "OK bye"
            joined.pop(c, None)
        elif text == "WHO":
            if joined:
                reply = "OK " + ",".join(sorted(joined.values()))
            else:
                reply = "OK"
        elif text.startswith("NICK "):
            name = text[5:]
            if not VALID.match(name):
                reply = "ERR bad name"
            elif any(v == name for k, v in joined.items() if k != c):
                reply = "ERR name taken"
            else:
                joined[c] = name
                reply = "OK " + name
        elif text.startswith("MSG "):
            body = text[4:]
            if body == "":
                reply = "ERR empty message"
            elif c not in joined:
                reply = "ERR join with NICK first"
            else:
                reply = "OK sent"
                bcast = [(k, "MSG " + joined[c] + " " + body) for k in joined if k != c]
        else:
            reply = "ERR unknown command"
        got[c].append(reply)
        for k, m in bcast:
            got[k].append(m)
    return got


class Conn:
    def __init__(self, port):
        self.s = socket.create_connection(("127.0.0.1", port), timeout=15)
        self.buf = b""

    def line(self):
        while b"\n" not in self.buf:
            chunk = self.s.recv(4096)
            if not chunk:
                return None
            self.buf += chunk
        i = self.buf.index(b"\n")
        out, self.buf = self.buf[:i], self.buf[i + 1:]
        return out.decode("ascii")

    def send(self, text, ending):
        self.s.sendall((text + ending).encode("ascii"))


def drive(port, names, steps):
    conns = {c: Conn(port) for c in names}
    got = {c: [] for c in names}
    for c, kind, text, ending in steps:
        conn = conns[c]
        conn.send(text, ending)
        if kind == "QUIT":
            while True:
                line = conn.line()
                if line is None:
                    break
                got[c].append(line)
            conn.s.close()
            continue
        while True:
            line = conn.line()
            if line is None:
                raise SystemExit("FAIL chat: %s: connection closed before the reply to %r" % (c, text))
            got[c].append(line)
            if not line.startswith("MSG "):
                break
    for c in names:
        conns[c].s.close()
    return got


def write_transcript(path, names, got):
    with open(path, "w") as f:
        for c in names:
            for line in got[c]:
                f.write("%s| %s\n" % (c, line))


def check(label, names, steps, got):
    want = model(names, steps)
    if got != want:
        for c in names:
            if got[c] != want[c]:
                print("FAIL chat: %s: client %s transcript differs from the model" % (label, c))
                print("  server: %r" % got[c])
                print("  model:  %r" % want[c])
                break
        raise SystemExit(1)


def listening(port):
    want = "%04X" % port
    for path in ("/proc/net/tcp", "/proc/net/tcp6"):
        try:
            rows = open(path).read().splitlines()[1:]
        except OSError:
            continue
        for row in rows:
            parts = row.split()
            if len(parts) > 3 and parts[1].endswith(":" + want) and parts[3] == "0A":
                return True
    return False


def main(argv):
    cmd = argv[1]
    if cmd == "freeport":
        s = socket.socket()
        s.bind(("127.0.0.1", 0))
        print(s.getsockname()[1])
        s.close()
        return 0
    if cmd == "waitlisten":
        port = int(argv[2])
        for _ in range(200):
            if listening(port):
                return 0
            time.sleep(0.05)
        return 1
    if cmd == "golden":
        names = ["A", "B"]
        got = drive(int(argv[2]), names, GOLDEN_STEPS)
        check("scripted", names, GOLDEN_STEPS, got)
        write_transcript(argv[3], names, got)
        return 0
    if cmd == "run":
        seed = int(argv[3])
        names, steps = gen(seed)
        got = drive(int(argv[2]), names, steps)
        check("seed %d" % seed, names, steps, got)
        write_transcript(argv[4], names, got)
        return 0
    raise SystemExit("usage: drive.py freeport | waitlisten PORT | golden PORT OUT | run PORT SEED OUT")


if __name__ == "__main__":
    sys.exit(main(sys.argv))
PY

cat >"$T/golden_scripted.txt" <<'EOF'
A| ERR unknown command
A| OK ann
A| OK sent
A| MSG bob reply
A| OK ann,bob
A| OK bye
B| ERR name taken
B| OK bob
B| MSG ann hello there
B| OK ann,bob
B| OK sent
B| OK bob
B| OK bye
EOF

cat >"$T/golden_client.txt" <<'EOF'
OK zed
OK zed
OK sent
ERR unknown command
OK bye
EOF

CB="$T/chat_c"; CN="$T/chat_n"
"$Y" test examples/real/chat/tests.yx >"$T/t.out" 2>&1 || { tail -5 "$T/t.out"; fail "unit tests (C)"; }
"$Y" test examples/real/chat/tests.yx --native >"$T/tn.out" 2>&1 || { tail -5 "$T/tn.out"; fail "unit tests (native)"; }
"$Y" build examples/real/chat/main.yx -o "$CB" >"$T/bc.out" 2>&1 || { cat "$T/bc.out"; fail "build (C)"; }
"$Y" build examples/real/chat/main.yx --native -o "$CN" >"$T/bn.out" 2>&1 || { cat "$T/bn.out"; fail "build (native)"; }

"$CB" bogus >/dev/null 2>&1; [ $? -eq 2 ] || fail "usage error must exit 2"
"$CB" client 127.0.0.1 notaport </dev/null >/dev/null 2>&1; [ $? -eq 2 ] || fail "bad port must exit 2"

for mode in c native; do
  bin="$CB"; [ "$mode" = native ] && bin="$CN"

  # the fixed two-client session, golden transcript
  port=$(python3 "$T/drive.py" freeport)
  start_server "$bin" "$port" 2 "$T/srv_s_$mode.out"
  srv=$SRV
  python3 "$T/drive.py" golden "$port" "$T/scripted_$mode.tr" || fail "scripted session ($mode)"
  finish_server "$srv" "$T/srv_s_$mode.out" 2
  cmp -s "$T/scripted_$mode.tr" "$T/golden_scripted.txt" || {
    diff "$T/scripted_$mode.tr" "$T/golden_scripted.txt"
    fail "scripted transcript ($mode) differs from the golden"
  }

  # the client mode, one connection, stdin to replies
  port=$(python3 "$T/drive.py" freeport)
  start_server "$bin" "$port" 1 "$T/srv_c_$mode.out"
  srv=$SRV
  printf 'NICK zed\nWHO\nMSG hi\nFOO\nQUIT\n' | timeout 10 "$bin" client 127.0.0.1 "$port" >"$T/client_$mode.out" 2>&1
  [ $? -eq 0 ] || fail "client mode ($mode) exit status: $(cat "$T/client_$mode.out")"
  finish_server "$srv" "$T/srv_c_$mode.out" 1
  cmp -s "$T/client_$mode.out" "$T/golden_client.txt" || {
    diff "$T/client_$mode.out" "$T/golden_client.txt"
    fail "client transcript ($mode) differs from the golden"
  }
done

for seed in $(seq 1 20); do
  for mode in c native; do
    bin="$CB"; [ "$mode" = native ] && bin="$CN"
    port=$(python3 "$T/drive.py" freeport)
    start_server "$bin" "$port" 3 "$T/srv_r_${mode}_$seed.out"
    srv=$SRV
    python3 "$T/drive.py" run "$port" "$seed" "$T/rand_${mode}_$seed.tr" || fail "random session seed $seed ($mode)"
    finish_server "$srv" "$T/srv_r_${mode}_$seed.out" 3
  done
  cmp -s "$T/rand_c_$seed.tr" "$T/rand_native_$seed.tr" || {
    diff "$T/rand_c_$seed.tr" "$T/rand_native_$seed.tr" | head
    fail "seed $seed: C and native transcripts differ"
  }
done

echo "chat: ok"
exit 0
