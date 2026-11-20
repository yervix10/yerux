#include "common.h"
int main(void) {
  i64 n = 10000000; LB f = {0}; i64 i = 0;
  while (i <= n) { LB_push(&f, 1); i = ADD(i, 1); }
  LB_set(&f, 0, 0); LB_set(&f, 1, 0);
  i = 2;
  while (MUL(i, i) <= n) {
    if (LB_get(&f, i)) { i64 j = MUL(i, i); while (j <= n) { LB_set(&f, j, 0); j = ADD(j, i); } }
    i = ADD(i, 1);
  }
  i64 count = 0, last = 0; i = 0;
  while (i <= n) { if (LB_get(&f, i)) { count = ADD(count, 1); last = i; } i = ADD(i, 1); }
  printf("%lld\n%lld\n", count, last);
}
