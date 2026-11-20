#include "common.h"
static void *volatile sink;  // keeps gcc from deleting the malloc/free pairs
int main(void) {
  i64 total = 0, i = 0;
  while (i < 6000000) {
    LI l = {0}; i64 k = 0;
    while (k < 8) { LI_push(&l, ADD(i, k)); k = ADD(k, 1); }
    sink = l.p;
    LB s = {0}; LB_push_str(&s, "item-"); LB_push_int(&s, i);
    sink = s.p;
    total = ADD(ADD(total, LI_get(&l, 7)), ADD(ADD(l.len, s.len), LB_get(&s, 5))) % 1000000007;
    free(l.p); free(s.p);
    i = ADD(i, 1);
  }
  printf("%lld\n", total);
}
