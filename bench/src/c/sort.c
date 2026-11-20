#include "common.h"
static void pass(const LI *src, LI *dst, i64 n, i64 w) {
  i64 lo = 0;
  while (lo < n) {
    i64 mid = ADD(lo, w); if (mid > n) mid = n;
    i64 hi = ADD(mid, w); if (hi > n) hi = n;
    i64 i = lo, j = mid, k = lo;
    while (i < mid && j < hi) {
      i64 a = LI_get(src, i), b = LI_get(src, j);
      if (a <= b) { LI_set(dst, k, a); i = ADD(i, 1); } else { LI_set(dst, k, b); j = ADD(j, 1); }
      k = ADD(k, 1);
    }
    while (i < mid) { LI_set(dst, k, LI_get(src, i)); i = ADD(i, 1); k = ADD(k, 1); }
    while (j < hi) { LI_set(dst, k, LI_get(src, j)); j = ADD(j, 1); k = ADD(k, 1); }
    lo = ADD(ADD(lo, w), w);
  }
}
static i64 checksum(const LI *l) {
  i64 s = 0, i = 0;
  while (i < l->len) { s = ADD(s, MUL(LI_get(l, i), i % 10)); i = ADD(i, 1); }
  return s;
}
int main(void) {
  i64 n = 200000; LI a = {0}, b = {0}; i64 x = 12345, grand = 0, round = 0;
  while (round < 10) {
    a.len = 0; b.len = 0; i64 i = 0;
    while (i < n) { x = ADD(MUL(x, 1103515245), 12345) % 2147483648LL; LI_push(&a, x); LI_push(&b, 0); i = ADD(i, 1); }
    i64 w = 1; int flip = 0;
    while (w < n) {
      if (flip == 0) { pass(&a, &b, n, w); flip = 1; } else { pass(&b, &a, n, w); flip = 0; }
      w = MUL(w, 2);
    }
    LI *r = flip == 0 ? &a : &b;
    grand = ADD(ADD(grand, LI_get(r, 0)), ADD(LI_get(r, SUB(n, 1)), checksum(r))) % 1000000007;
    round = ADD(round, 1);
  }
  printf("%lld\n", grand);
}
