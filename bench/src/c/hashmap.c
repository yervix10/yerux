#include <stdio.h>
#include <stdlib.h>
int main(void) {
  const int n = 200000, mod = 2147483647;
  int cap = 8, len = 0;
  int *keys = malloc((size_t)cap * sizeof(int));
  int *vals = malloc((size_t)cap * sizeof(int));
  int *hashes = malloc((size_t)cap * sizeof(int));
  int *slots = malloc((size_t)cap * sizeof(int));
  for (int i = 0; i < cap; ++i) slots[i] = -1;
  for (int i = 0; i < n; ++i) {
    int h = (int)(((long long)(i + 1) * 1103515245) % mod);
    if ((len + 1) * 5 > cap * 3) {
      int old_cap = cap; cap *= 2;
      int *next = malloc((size_t)cap * sizeof(int));
      for (int j = 0; j < cap; ++j) next[j] = -1;
      for (int e = 0; e < len; ++e) {
        int j = hashes[e] % cap;
        while (next[j] >= 0) { if (++j == cap) j = 0; }
        next[j] = e;
      }
      free(slots); slots = next;
      keys = realloc(keys, (size_t)cap * sizeof(int));
      vals = realloc(vals, (size_t)cap * sizeof(int));
      hashes = realloc(hashes, (size_t)cap * sizeof(int));
      (void)old_cap;
    }
    int j = h % cap;
    while (slots[j] >= 0) { if (++j == cap) j = 0; }
    slots[j] = len; keys[len] = i; vals[len] = i * 3 + 1; hashes[len] = h; ++len;
  }
  long long sum = 0;
  for (int i = 0; i < n; ++i) {
    int h = (int)(((long long)(i + 1) * 1103515245) % mod);
    int j = h % cap;
    while (slots[j] >= 0 && !(hashes[slots[j]] == h && keys[slots[j]] == i)) { if (++j == cap) j = 0; }
    if (slots[j] >= 0) sum += vals[slots[j]];
  }
  printf("%lld\n", sum);
  free(keys); free(vals); free(hashes); free(slots);
}
