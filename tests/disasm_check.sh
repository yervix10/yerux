#!/usr/bin/env bash
# Encoder verification: disassemble the code region of every native binary with objdump and
# fail on invalid opcodes ("(bad)", ".byte"), on jump/call targets that do not land on an
# instruction boundary, and on rip-relative references that leave the data region.
#   tests/disasm_check.sh            all native-supported examples/*.yx and tests/*.yx
#   tests/disasm_check.sh --fuzz N   additionally N random programs from tests/fuzz.py
# Layout (see src/x64.rs): 64-byte ELF header + 2 x 56-byte program headers = 176 bytes, then code,
# padded with 0x90 to 16 bytes, then data blobs. Blob 0 is "panic: ", so the code region ends
# at the first occurrence of that string after the header.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT" || exit 2
FUZZ=0
[ "${1:-}" = "--fuzz" ] && FUZZ="${2:-100}"
cargo build --release 2>&1 | tail -n 1
Y="$ROOT/target/release/yerux"
TMP="$(mktemp -d /tmp/yerux_disasm.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT
command -v objdump >/dev/null || { echo "objdump not found"; exit 2; }

progs=()
for f in examples/*.yx tests/*.yx; do
  case "$(basename "$f")" in e[0-9]*) continue;; esac
  progs+=("$f")
done
if [ "$FUZZ" -gt 0 ]; then
  python3 - "$FUZZ" "$TMP" <<'PY'
import sys
sys.path.insert(0, "tests")
import fuzz
n, out = int(sys.argv[1]), sys.argv[2]
for i in range(n):
    open(f"{out}/fz{i}.yx", "w").write("\n".join(fuzz.Gen(777000 + i).program()) + "\n")
PY
  for f in "$TMP"/fz*.yx; do progs+=("$f"); done
fi

checked=0; skipped=0; failed=0; insns=0; refs=0
for f in "${progs[@]}"; do
  # The raw static ELF byte scanner below assumes the legacy single-segment
  # no-import image. Dynamic ELF images have ELF/program headers before code.
  if grep -q 'extern "C"' "$f"; then skipped=$((skipped+1)); continue; fi
  exe="$TMP/bin"
  if ! "$Y" build "$f" --native -o "$exe" >/dev/null 2>"$TMP/err"; then
    if grep -q "native backend:" "$TMP/err"; then skipped=$((skipped+1)); else
      echo "FAIL $f: native build error"; failed=$((failed+1)); fi
    continue
  fi
  size=$(stat -c %s "$exe")
  end=$(grep -obUa "panic: " "$exe" | head -n1 | cut -d: -f1)
  if [ -z "$end" ] || [ "$end" -le 176 ]; then
    echo "FAIL $f: cannot locate end of code region"; failed=$((failed+1)); continue
  fi
  objdump -D -b binary -mi386:x86-64 --start-address=176 --stop-address="$end" "$exe" >"$TMP/dis.txt" 2>&1
  res=$(python3 - "$TMP/dis.txt" "$end" "$size" <<'PY'
import re, sys
path, end, size = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
starts, targets, rip, bad = set(), [], [], []
n = 0
for line in open(path):
    m = re.match(r"\s*([0-9a-f]+):\t([0-9a-f ]+?)\s*(?:\t(.*))?$", line.rstrip("\n"))
    if not m:
        continue
    addr, text = int(m.group(1), 16), (m.group(3) or "").strip()
    if not text:                       # continuation line of a long instruction
        continue
    n += 1
    starts.add(addr)
    if "(bad)" in text or text.startswith(".byte") or "unknown" in text:
        bad.append(f"{addr:#x}: {text}")
    mm = re.match(r"(?:jmp|j[a-z]+|call)\s+0x([0-9a-f]+)\b", text)
    if mm:
        targets.append((addr, int(mm.group(1), 16), text))
    mr = re.search(r"#\s*0x([0-9a-f]+)", text)
    if mr and "(%rip)" in text:
        rip.append((addr, int(mr.group(1), 16), text))
for a, t, txt in targets:
    if t not in starts:
        bad.append(f"{a:#x}: branch target {t:#x} is not an instruction start ({txt})")
for a, t, txt in rip:
    if txt.split(None, 1)[-1].startswith("lea") or txt.lstrip().startswith("lea"):
        if t in starts:
            continue  # address of a function (closure / fn value)
    if not (end <= t < size):
        bad.append(f"{a:#x}: rip-relative target {t:#x} outside data region [{end:#x},{size:#x}) ({txt})")
print(n, len(bad), len(targets) + len(rip))
for b in bad[:8]:
    print(b)
PY
)
  cnt=$(echo "$res" | head -n1)
  nbad=$(echo "$cnt" | cut -d" " -f2)
  refs=$((refs + $(echo "$cnt" | cut -d" " -f3)))
  insns=$((insns + ${cnt%% *}))
  if [ "$nbad" != "0" ]; then
    echo "FAIL $f"; echo "$res" | tail -n +2; failed=$((failed+1))
  else
    checked=$((checked+1))
  fi
done
echo "disasm_check: $checked binaries OK, $failed failed, $skipped skipped (unsupported), $insns instructions decoded, $refs branch/rip targets validated"
[ $failed -eq 0 ]
