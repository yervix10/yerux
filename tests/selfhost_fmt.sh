#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
TMP="$(mktemp -d /tmp/yerux_selfhost_fmt.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT
cargo build -q
target/debug/yerux build selfhost/fmt.yx -o "$TMP/fmt" >/dev/null
target/debug/yerux fmt --selfhost --stdout tests/e1009_array_len_mismatch.yx >"$TMP/cli"
target/debug/yerux fmt --stdout tests/e1009_array_len_mismatch.yx >"$TMP/oracle"
cmp "$TMP/cli" "$TMP/oracle"
# Freeze the Rust dirty-file set from a throwaway corpus copy. Never run in-place fmt in ROOT.
mkdir "$TMP/corpus"
cp -r examples tests "$TMP/corpus/"
(cd "$TMP/corpus" && "$ROOT/target/debug/yerux" fmt --check examples tests >"$TMP/targets" 2>"$TMP/oracle-errors" || true)
total=0; equal=0; idem=0; mismatch=0
while IFS= read -r rel; do
  [ -n "$rel" ] || continue
  total=$((total + 1))
  original="$TMP/corpus/$rel"
  "$ROOT/target/debug/yerux" fmt --stdout "$original" >"$TMP/rust"
  "$TMP/fmt" "$original" >"$TMP/first" 2>/dev/null || { mismatch=$((mismatch + 1)); continue; }
  cp "$TMP/first" "$TMP/roundtrip.yx"
  "$TMP/fmt" "$TMP/roundtrip.yx" >"$TMP/second" 2>/dev/null || true
  if cmp -s "$TMP/first" "$TMP/second"; then idem=$((idem + 1)); fi
  if cmp -s "$TMP/rust" "$TMP/first"; then equal=$((equal + 1)); else mismatch=$((mismatch + 1)); fi
done < "$TMP/targets"
printf 'selfhost fmt dirty corpus: %d/%d identical (%d%%); idempotent %d/%d; mismatches %d\n' "$equal" "$total" "$((equal * 100 / (total ? total : 1)))" "$idem" "$total" "$mismatch"
test "$equal" -eq "$total" && test "$idem" -eq "$total"
