#include "common.h"
int main(void) {
  i64 i = 0, acc = 0;
  while (i < 100000000) { acc = ADD(acc, MUL(i, 3)) % 1000003; i = ADD(i, 1); }
  printf("%lld\n", acc);
}
