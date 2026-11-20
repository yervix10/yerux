#include "common.h"
int main(void) {
  LB s = {0}; i64 i = 0;
  while (i < 8000000) { LB_push_str(&s, "ab"); LB_push_int(&s, i); i = ADD(i, 1); }
  i64 sum = 0, j = 0;
  while (j < s.len) { sum = ADD(sum, (unsigned char)LB_get(&s, j)); j = ADD(j, 1); }
  printf("%lld\n%lld\n", s.len, sum);
}
