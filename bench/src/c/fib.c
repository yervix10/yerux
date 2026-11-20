#include "common.h"
static i64 fib(i64 n) { if (n < 2) return n; return ADD(fib(SUB(n, 1)), fib(SUB(n, 2))); }
int main(void) { printf("%lld\n", fib(40)); }
