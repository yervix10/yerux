#include <stdio.h>
#include <stdlib.h>
int main(void) {
  const int n = 10;
  int *perm1 = malloc((size_t)n * sizeof(int));
  int *perm = malloc((size_t)n * sizeof(int));
  int *count = calloc((size_t)n, sizeof(int));
  for (int i = 0; i < n; ++i) perm1[i] = i;
  int r = n, checksum = 0, max_flips = 0, parity = 0;
  for (;;) {
    while (r != 1) { count[r - 1] = r; --r; }
    for (int i = 0; i < n; ++i) perm[i] = perm1[i];
    int flips = 0;
    if (perm[0] != 0) while (perm[0] != 0) {
      for (int lo = 0, hi = perm[0]; lo < hi; ++lo, --hi) {
        int t = perm[lo]; perm[lo] = perm[hi]; perm[hi] = t;
      }
      ++flips;
    }
    if (flips > max_flips) max_flips = flips;
    checksum += parity ? -flips : flips;
    parity = 1 - parity;
    for (;;) {
      if (r == n) { printf("%d\n%d\n", checksum, max_flips); free(perm1); free(perm); free(count); return 0; }
      int first = perm1[0];
      for (int j = 0; j < r; ++j) perm1[j] = perm1[j + 1];
      perm1[r] = first;
      if (--count[r] > 0) break;
      ++r;
    }
  }
}
