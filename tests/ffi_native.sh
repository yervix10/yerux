#!/usr/bin/env bash
# Smoke tests for Linux dynamic ELF imports and Windows target diagnostics.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
Y="$ROOT/target/release/yerux"
TMP="$(mktemp -d /tmp/yerux_ffi.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

"$Y" build tests/ffi_basic.yx --native -o "$TMP/ffi_basic"
readelf -lW "$TMP/ffi_basic" | grep -q 'GNU_STACK.*RW '
readelf -lW "$TMP/ffi_basic" | awk '$1 == "LOAD" { n++; if ($0 ~ /RWE|RWX/) bad=1 } END { exit (n == 2 && !bad) ? 0 : 1 }'
"$TMP/ffi_basic" >"$TMP/out"
cat >"$TMP/want" <<'EOF'
5
19
true
9.0
1024.0
EOF
cmp "$TMP/want" "$TMP/out"

# The AArch64 image uses its own interpreter and ABI. Execution is covered
# by the QEMU FFI fixture group in tests/a64_emu.py.
"$Y" build tests/ffi_basic.yx --target aarch64 -o "$TMP/ffi_a64"
readelf -h "$TMP/ffi_a64" | grep -q 'Machine:.*AArch64'
readelf -lW "$TMP/ffi_a64" | grep -q 'Requesting program interpreter: /lib/ld-linux-aarch64.so.1'
readelf -lW "$TMP/ffi_a64" | awk '$1 == "LOAD" { n++; if ($0 ~ /RWE|RWX/) bad=1 } END { exit (n == 2 && !bad) ? 0 : 1 }'

if "$Y" build tests/ffi_basic.yx --native --target windows -o "$TMP/unsupported" >"$TMP/unsupported.log" 2>&1; then
  echo "ffi_native: expected Windows extern rejection" >&2
  exit 1
fi
grep -Fq "extern C library 'libm.so.6' is not supported on the windows target" "$TMP/unsupported.log"

for mode in --release --no-opt; do
  "$Y" build tests/pe_ffi.yx --target windows "$mode" -o "$TMP/ffi.exe"
done

cat >"$TMP/write.yx" <<'EOF'
extern "C" {
  fn write(fd: Int32, text: Text, count: UInt64) -> Int
}
fn main() { unsafe { write(1, "write-ok\n", 9) } }
EOF
"$Y" build "$TMP/write.yx" --native -o "$TMP/write"
"$TMP/write" >"$TMP/out"
test "$(cat "$TMP/out")" = "write-ok"
if "$Y" build "$TMP/write.yx" --target windows -o "$TMP/unsupported" >"$TMP/unsupported.log" 2>&1; then
  echo "ffi_native: expected Windows to reject an unavailable CRT symbol" >&2
  exit 1
fi
grep -Fq "extern C symbol 'write' is not supported on the windows target" "$TMP/unsupported.log"

cat >"$TMP/custom.c" <<'EOF'
long ffi_roundtrip(long value, const char *text) {
    return value + (text[0] == 'y' ? 40 : 0);
}
unsigned char *ffi_identity(const char *text) { return (unsigned char *)text; }
long ffi_stack8(long a, long b, long c, long d, long e, long f, long g, long h) {
    return a + b + c + d + e + f + g + h;
}
double ffi_float9(double a, double b, double c, double d, double e,
                  double f, double g, double h, double i) {
    return a + b + c + d + e + f + g + h + i;
}
EOF
cc -shared -fPIC -Wl,-soname,libyeruxffi.so -o "$TMP/libyeruxffi.so" "$TMP/custom.c"
cat >"$TMP/custom.yx" <<'EOF'
link "libyeruxffi.so"
extern "C" {
  fn ffi_roundtrip(value: Int, text: Text) -> Int
  fn ffi_identity(text: Text) -> Ptr<UInt8>
  fn strlen(text: Ptr<UInt8>) -> UInt64
  fn ffi_stack8(a: Int, b: Int, c: Int, d: Int, e: Int, f: Int, g: Int, h: Int) -> Int
  fn ffi_float9(a: Float, b: Float, c: Float, d: Float, e: Float, f: Float, g: Float, h: Float, i: Float) -> Float
}
fn main() { unsafe { print(ffi_roundtrip(2, "yes")); print(strlen(ffi_identity("pointer"))); print(ffi_stack8(1, 2, 3, 4, 5, 6, 7, 8)); print(ffi_float9(1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0)) } }
EOF
"$Y" build "$TMP/custom.yx" --native -o "$TMP/custom"
LD_LIBRARY_PATH="$TMP${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$TMP/custom" >"$TMP/out"
cat >"$TMP/want" <<'EOF'
42
7
36
45.0
EOF
cmp "$TMP/want" "$TMP/out"

# Variadic calls: the callee reads its arguments with va_arg, so the register/stack split, AL (the number
# of vector registers used) and Float32 promotion are all observable.
cat >"$TMP/var.c" <<'EOF'
#include <stdarg.h>
/* fmt: one char per argument, 'i' = long, 'd' = double; returns the sum, ints and doubles together */
double var_mix(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    double total = 0;
    for (; *fmt; fmt++) {
        if (*fmt == 'i') total += (double)va_arg(ap, long);
        else total += va_arg(ap, double);
    }
    va_end(ap);
    return total;
}
long var_count(long first, ...) {
    va_list ap;
    va_start(ap, first);
    long n = 0, v;
    while ((v = va_arg(ap, long)) != 0) n += v * first;
    va_end(ap);
    return n;
}
EOF
cc -shared -fPIC -Wl,-soname,libyeruxvar.so -o "$TMP/libyeruxvar.so" "$TMP/var.c"
cat >"$TMP/var.yx" <<'EOF'
link "libyeruxvar.so"
extern "C" {
  fn var_mix(fmt: Text, ...) -> Float
  fn var_count(first: Int, ...) -> Int
}
fn main() {
  unsafe {
    print(var_mix(""))
    print(var_mix("i", 5))
    print(var_mix("d", 2.5))
    print(var_mix("idid", 1, 0.5, 2, 0.25))
    let h: Float32 = 1.5
    print(var_mix("dd", h, 2.0))
    print(var_mix("iiiiiiii", 1, 2, 3, 4, 5, 6, 7, 8))
    print(var_mix("dddddddddd", 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0))
    print(var_mix("iddiddiddiddiddidd", 1, 1.0, 1.0, 2, 2.0, 2.0, 3, 3.0, 3.0, 4, 4.0, 4.0, 5, 5.0, 5.0, 6, 6.0, 6.0))
    print(var_count(3, 1, 2, 3, 4, 5, 6, 7, 8, 0))
  }
}
EOF
"$Y" build "$TMP/var.yx" --native -o "$TMP/var"
LD_LIBRARY_PATH="$TMP${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$TMP/var" >"$TMP/out"
cat >"$TMP/want" <<'EOF'
0.0
5.0
2.5
3.75
3.5
36.0
55.0
63.0
108
EOF
cmp "$TMP/want" "$TMP/out"
# the C backend must agree
LIBRARY_PATH="$TMP" "$Y" build "$TMP/var.yx" -o "$TMP/var_c"
LD_LIBRARY_PATH="$TMP${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$TMP/var_c" >"$TMP/out_c"
cmp "$TMP/want" "$TMP/out_c"

# snprintf/printf: native output must match the C backend byte for byte.
"$Y" build tests/ffi_variadic.yx -o "$TMP/fv_c"
"$Y" build tests/ffi_variadic.yx --native -o "$TMP/fv_n"
"$TMP/fv_c" >"$TMP/fv_c.out"
"$TMP/fv_n" >"$TMP/fv_n.out"
cmp "$TMP/fv_c.out" "$TMP/fv_n.out"
echo "ffi_native: libc/libm, custom shared library and variadic calls passed"
