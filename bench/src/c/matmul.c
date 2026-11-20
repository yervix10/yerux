#include "common.h"
int main(void) {
  i64 n = 300; LI a = {0}, b = {0}, c = {0}; i64 i = 0;
  while (i < n) { i64 j = 0; while (j < n) {
    LI_push(&a, ADD(MUL(i, j), 1) % 10); LI_push(&b, ADD(i, j) % 10); LI_push(&c, 0); j = ADD(j, 1); } i = ADD(i, 1); }
  i64 grand = 0, r = 0;
  while (r < 5) {
    LI_set(&a, 0, r);
    i = 0;
    while (i < n) { i64 j = 0; while (j < n) {
      i64 acc = 0, k = 0;
      while (k < n) { acc = ADD(acc, MUL(LI_get(&a, ADD(MUL(i, n), k)), LI_get(&b, ADD(MUL(k, n), j)))); k = ADD(k, 1); }
      LI_set(&c, ADD(MUL(i, n), j), acc); j = ADD(j, 1); } i = ADD(i, 1); }
    i = 0;
    while (i < MUL(n, n)) { grand = ADD(grand, MUL(LI_get(&c, i), i % 7)); i = ADD(i, 1); }
    r = ADD(r, 1);
  }
  printf("%lld\n", grand);
}
