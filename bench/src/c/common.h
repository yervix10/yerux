// Shared helpers for the C benchmarks. Compile with -DCHECKED for a variant that has
// the same safety checks as Yerux (overflow on + - *, bounds on every index).
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
typedef long long i64;
static void die(const char *m) { fprintf(stderr, "panic: %s\n", m); exit(101); }
#ifdef CHECKED
static inline i64 ck_add(i64 a, i64 b) { i64 r; if (__builtin_add_overflow(a, b, &r)) die("overflow"); return r; }
static inline i64 ck_sub(i64 a, i64 b) { i64 r; if (__builtin_sub_overflow(a, b, &r)) die("overflow"); return r; }
static inline i64 ck_mul(i64 a, i64 b) { i64 r; if (__builtin_mul_overflow(a, b, &r)) die("overflow"); return r; }
static inline i64 ck_idx(i64 i, i64 n) { if ((unsigned long long)i >= (unsigned long long)n) die("index out of range"); return i; }
#define ADD(a, b) ck_add(a, b)
#define SUB(a, b) ck_sub(a, b)
#define MUL(a, b) ck_mul(a, b)
#define IDX(i, n) ck_idx(i, n)
#else
#define ADD(a, b) ((a) + (b))
#define SUB(a, b) ((a) - (b))
#define MUL(a, b) ((a) * (b))
#define IDX(i, n) (i)
#endif
// growable list of T (doubling, like Vec / append)
#define DEFLIST(NAME, T) \
  typedef struct { T *p; i64 len, cap; } NAME; \
  static inline void NAME##_push(NAME *l, T v) { \
    if (l->len == l->cap) { l->cap = l->cap ? l->cap * 2 : 4; l->p = realloc(l->p, l->cap * sizeof(T)); if (!l->p) die("out of memory"); } \
    l->p[l->len++] = v; } \
  static inline T NAME##_get(const NAME *l, i64 i) { return l->p[IDX(i, l->len)]; } \
  static inline void NAME##_set(NAME *l, i64 i, T v) { l->p[IDX(i, l->len)] = v; }
DEFLIST(LI, i64)
DEFLIST(LB, char)
// append decimal representation of n to byte list
static inline void LB_push_int(LB *s, i64 n) {
  char buf[24]; int k = 0; unsigned long long u = n < 0 ? 0ULL - (unsigned long long)n : (unsigned long long)n;
  do { buf[k++] = '0' + u % 10; u /= 10; } while (u);
  if (n < 0) LB_push(s, '-');
  while (k) LB_push(s, buf[--k]);
}
static inline void LB_push_str(LB *s, const char *t) { while (*t) LB_push(s, *t++); }
#include <emmintrin.h>
static inline double fsqrt(double x) { return _mm_cvtsd_f64(_mm_sqrt_sd(_mm_setzero_pd(), _mm_set_sd(x))); }
