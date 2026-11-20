#include "common.h"
DEFLIST(FL, double)
int main(void) {
  i64 n = 1000000;
  FL a = {0, 0, 0}, b = {0, 0, 0};
  for (i64 i = 0; i < n; i++) {
    FL_push(&a, (double)(i % 100) * 0.5);
    FL_push(&b, (double)((i * 7) % 13) * 0.25);
  }
  double grand = 0.0;
  for (i64 r = 0; r < 200; r++) {
    FL_set(&a, 0, (double)r);
    double acc = 0.0;
    for (i64 i = 0; i < n; i++) acc = acc + FL_get(&a, i) * FL_get(&b, i);
    grand = grand + acc * 0.001;
  }
  printf("%lld\n", (i64)grand);
  return 0;
}
