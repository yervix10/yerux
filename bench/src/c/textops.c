#include "common.h"

typedef struct { char *p; i64 n, cap; } Buf;
static void append(Buf *b, const char *p, i64 n) {
  if (n <= 0) return;
  if (b->n + n > b->cap) {
    i64 c = b->cap ? b->cap : 16;
    while (c < b->n + n) c *= 2;
    char *q = realloc(b->p, (size_t)c);
    if (!q) abort();
    b->p = q; b->cap = c;
  }
  memcpy(b->p + b->n, p, (size_t)n); b->n += n;
}
static i64 find(const char *s, i64 n, const char *p, i64 m) {
  for (i64 i = 0; i + m <= n; ++i) if (!memcmp(s + i, p, (size_t)m)) return i;
  return -1;
}
int main(void) {
  i64 checksum = 0;
  for (i64 i = 0; i < 200000; ++i) {
    Buf line = {0}, joined = {0};
    append(&line, "item=", 5); LB_push_int((LB *)&line, i);
    append(&line, ";value=", 7); LB_push_int((LB *)&line, i % 97);
    append(&line, ";tail", 5);
    i64 found = find(line.p, line.n, ";value=", 7);
    i64 a = 0;
    for (i64 k = 0; k <= line.n; ++k) {
      if (k == line.n || line.p[k] == ';') {
        if (a != 0) append(&joined, "|", 1);
        append(&joined, line.p + a, k - a);
        a = k + 1;
      }
    }
    checksum += found + joined.n;
    free(line.p); free(joined.p);
  }
  printf("%lld\n", checksum);
}
