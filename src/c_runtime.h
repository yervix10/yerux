#include <stdint.h>
#include <stdatomic.h>
#include <pthread.h>
#include <setjmp.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdbool.h>
#include <string.h>
#include <errno.h>
#include <unistd.h>
#include <sys/syscall.h>
#include <linux/futex.h>
extern char **environ;

typedef struct { char *d; int64_t len, cap; } String; /* owned UTF-8 bytes, no terminator */
typedef struct { const char *p; int64_t n; } Str;      /* read-only view of bytes (Text or borrowed String) */

static _Thread_local jmp_buf *nx_worker_panic;
typedef struct NxCleanup {
  struct NxCleanup *previous;
  bool *live;
  void *value;
  void (*drop)(void *);
} NxCleanup;
static _Thread_local NxCleanup *nx_cleanup_head;
static inline void nx_cleanup_push(NxCleanup *slot, bool *live, void *value, void (*drop)(void *)) {
  *slot = (NxCleanup){ nx_cleanup_head, live, value, drop };
  nx_cleanup_head = slot;
}
static inline void nx_cleanup_pop(void) { nx_cleanup_head = nx_cleanup_head->previous; }
static _Thread_local bool nx_unwinding;
static _Thread_local bool nx_in_destructor;
static void nx_try_worker_panic(void) {
  if (nx_unwinding) {
    fputs("panic: second panic during worker cleanup\n", stderr);
    exit(101);
  }
  if (!nx_worker_panic || nx_in_destructor) return;
  jmp_buf *target = nx_worker_panic;
  nx_worker_panic = NULL;
  nx_unwinding = true;
  while (nx_cleanup_head) {
    NxCleanup *slot = nx_cleanup_head;
    nx_cleanup_pop();
    if (*slot->live) {
      *slot->live = false;
      slot->drop(slot->value);
    }
  }
  longjmp(*target, 1);
}
__attribute__((noreturn)) static void nx_panic(const char *m) { nx_try_worker_panic(); fprintf(stderr, "panic: %s\n", m); exit(101); }
/* a function value: code pointer and environment (0, or a heap block whose first word is its drop function) */
typedef struct { void *code; void *env; } NxFn;
typedef void (*NxDynSlot)(void);
typedef struct { void *data; const NxDynSlot *table; } NxDyn;
/* dropping a closure: each environment drop frees its own block and hands the environments of the closures it
   captured to the worklist, so a chain of closures that capture each other needs no recursion */
typedef struct { void **d; size_t n, cap; } NxEnvWL;
static inline void nx_envpush(NxEnvWL *w, void *e) {
  if (!e) return;
  if (w->n == w->cap) {
    if (w->cap > SIZE_MAX / 2 / sizeof(void *)) nx_panic("too many captured closures");
    size_t nc = w->cap ? w->cap * 2 : 8;
    void **nd = (void **)realloc(w->d, nc * sizeof(void *));
    if (!nd) nx_panic("out of memory");
    w->d = nd; w->cap = nc;
  }
  w->d[w->n++] = e;
}
static inline void nx_drop_fn(NxFn *f) {
  void *e = f->env; f->env = 0;
  if (!e) return;
  NxEnvWL wl = {0, 0, 0};
  for (;;) {
    ((void (*)(void *, NxEnvWL *))(intptr_t)*(int64_t *)e)(e, &wl);
    if (!wl.n) break;
    e = wl.d[--wl.n];
  }
  free(wl.d);
}
typedef struct { pthread_t id; NxFn entry; int64_t result; bool recover; } NxThread;
static void *nx_thread_entry(void *arg) {
  NxThread *task = arg;
  jmp_buf recovery;
  task->result = 0;
  if (task->recover) nx_worker_panic = &recovery;
  if (task->recover) {
    if (setjmp(recovery) != 0) goto cleanup;
  }
  task->result = ((int64_t (*)(void *))task->entry.code)(task->entry.env);
cleanup:
  nx_worker_panic = NULL;
  nx_drop_fn(&task->entry);
  nx_unwinding = false;
  return NULL;
}
static inline int64_t nx_thread_start(NxFn entry, bool recover) {
  NxThread *task = malloc(sizeof(*task));
  if (!task) nx_panic("out of memory");
  task->entry = entry;
  task->recover = recover;
  if (pthread_create(&task->id, NULL, nx_thread_entry, task)) nx_panic("could not start thread");
  return (int64_t)(intptr_t)task;
}
static inline int64_t nx_thread_wait(int64_t handle) {
  NxThread *task = (NxThread *)(intptr_t)handle;
  if (pthread_join(task->id, NULL)) nx_panic("could not join thread");
  int64_t result = task->result;
  free(task);
  return result;
}
static inline int64_t nx_futex_park(int64_t address, int64_t expected) {
  long result = syscall(SYS_futex, (void *)(intptr_t)address, FUTEX_WAIT_PRIVATE,
                        (uint32_t)expected, NULL, NULL, 0);
  return result == -1 ? -errno : result;
}
static inline int64_t nx_futex_wake(int64_t address, int64_t count) {
  long result = syscall(SYS_futex, (void *)(intptr_t)address, FUTEX_WAKE_PRIVATE,
                        (int32_t)count, NULL, NULL, 0);
  return result == -1 ? -errno : result;
}
static inline void nx_parallel_call(int64_t address, int64_t index) {
  const NxFn *body = (const NxFn *)(intptr_t)address;
  ((void (*)(void *, int64_t))body->code)(body->env, index);
}
static inline void nx_drop_dyn(NxDyn *object) { ((void (*)(void *))object->table[0])(object->data); }
__attribute__((noreturn)) static void nx_panic_str(Str s) { nx_try_worker_panic(); fputs("panic: ", stderr); if (s.n > 0) fwrite(s.p, 1, (size_t)s.n, stderr); fputc('\n', stderr); exit(101); }
static inline Str nx_str_text(const char *s) { Str r = { s, (int64_t)strlen(s) }; return r; }
static inline Str nx_str_view(const String *s) { Str r = { s->d, s->len }; return r; }
static inline String nx_str_new(void) { String s = {0, 0, 0}; return s; }
static inline void nx_str_push_str(String *s, Str x) {
  if (x.n < 0) nx_panic("negative string length");
  if (x.n == 0) return;
  if (x.n > INT64_MAX - s->len) nx_panic("string too long");
  int64_t need = s->len + x.n;
  if (need > s->cap) {
    int64_t nc = s->cap ? s->cap : 16;
    while (nc < need) nc = nc > INT64_MAX / 2 ? need : nc * 2;
    if ((uint64_t)nc > SIZE_MAX) nx_panic("string too long");
    char *nd = (char *)realloc(s->d, (size_t)nc);
    if (!nd) nx_panic("out of memory");
    s->d = nd; s->cap = nc;
  }
  memcpy(s->d + s->len, x.p, (size_t)x.n);
  s->len += x.n;
}
static inline int64_t nx_str_raw_addr(String *s) { return (int64_t)(intptr_t)s->d; }
static inline void nx_str_push_raw(String *s, int64_t p, int64_t n) { Str x = { (const char *)(intptr_t)p, n }; nx_str_push_str(s, x); }
static inline String nx_str_from(Str x) { String s = {0, 0, 0}; nx_str_push_str(&s, x); return s; }
static inline String nx_str_concat(Str a, Str b) { String s = {0, 0, 0}; nx_str_push_str(&s, a); nx_str_push_str(&s, b); return s; }
static inline void nx_str_push_int(String *s, int64_t v) {
  /* Format into a stack buffer, then reserve/copy once.  snprintf is
     disproportionately expensive in tight append loops.  Form the
     magnitude unsigned so INT64_MIN is representable. */
  char buf[21];
  int n = 0;
  uint64_t magnitude = v < 0 ? UINT64_C(0) - (uint64_t)v : (uint64_t)v;
  do {
    buf[n++] = (char)('0' + magnitude % 10);
    magnitude /= 10;
  } while (magnitude);
  if (v < 0) buf[n++] = '-';
  for (int i = 0; i < n / 2; ++i) {
    char c = buf[i]; buf[i] = buf[n - i - 1]; buf[n - i - 1] = c;
  }
  Str x = { buf, n }; nx_str_push_str(s, x);
}
static inline int64_t nx_str_len(String *s) { return s->len; }
static inline bool nx_str_eq(String *s, Str x) { return s->len == x.n && (x.n == 0 || memcmp(s->d, x.p, (size_t)x.n) == 0); }
static inline int64_t nx_str_byte(String *s, int64_t i) {
  if (i < 0 || i >= s->len) nx_panic("index out of range");
  return (unsigned char)s->d[i];
}
static inline void nx_str_clear(String *s) { s->len = 0; }
static inline void nx_str_push_byte(String *s, int64_t b) {
  if (b < 0 || b > 255) nx_panic("byte value out of range (0..255)");
  char c = (char)b; Str x = { &c, 1 }; nx_str_push_str(s, x);
}
static inline void nx_str_push_slice(String *s, Str x, int64_t a, int64_t b) {
  if (b < 0 || b > x.n || a < 0 || a > b) nx_panic("index out of range");
  if (a == b) return;
  Str y = { x.p + a, b - a }; nx_str_push_str(s, y);
}
static inline bool nx_char_valid(int64_t i) { return i >= 0 && i <= 0x10FFFF && !(i >= 0xD800 && i <= 0xDFFF); }
static inline int64_t nx_char_from_int(int64_t i) { if (!nx_char_valid(i)) nx_panic("invalid Unicode scalar value"); return i; }
static inline int64_t nx_char_utf8_len(int64_t c) { return c < 0x80 ? 1 : c < 0x800 ? 2 : c < 0x10000 ? 3 : 4; }
static inline int nx_char_encode(int64_t c, char *b) {
  if (c < 0x80) { b[0] = (char)c; return 1; }
  if (c < 0x800) { b[0] = (char)(0xC0 + c / 64); b[1] = (char)(0x80 + c % 64); return 2; }
  if (c < 0x10000) { b[0] = (char)(0xE0 + c / 4096); b[1] = (char)(0x80 + (c / 64) % 64); b[2] = (char)(0x80 + c % 64); return 3; }
  b[0] = (char)(0xF0 + c / 262144); b[1] = (char)(0x80 + (c / 4096) % 64); b[2] = (char)(0x80 + (c / 64) % 64); b[3] = (char)(0x80 + c % 64); return 4;
}
static inline void nx_str_push_char(String *s, int64_t c) { char b[4]; Str x = { b, nx_char_encode(c, b) }; nx_str_push_str(s, x); }
static inline int64_t nx_str_char_size(String *s, int64_t i) {
  int64_t b = nx_str_byte(s, i);
  if (b < 0x80) return 1;
  int64_t n = b >= 0xC2 && b <= 0xDF ? 2 : b >= 0xE0 && b <= 0xEF ? 3 : b >= 0xF0 && b <= 0xF4 ? 4 : 0;
  if (!n || n > s->len - i) nx_panic("invalid UTF-8 sequence");
  int64_t c1 = (unsigned char)s->d[i + 1];
  if (c1 < 0x80 || c1 > 0xBF) nx_panic("invalid UTF-8 sequence");
  if (n == 3) {
    int64_t c2 = (unsigned char)s->d[i + 2];
    if (c2 < 0x80 || c2 > 0xBF || (b == 0xE0 && c1 < 0xA0) || (b == 0xED && c1 > 0x9F))
      nx_panic("invalid UTF-8 sequence");
  }
  if (n == 4) {
    int64_t c2 = (unsigned char)s->d[i + 2];
    int64_t c3 = (unsigned char)s->d[i + 3];
    if (c2 < 0x80 || c2 > 0xBF || c3 < 0x80 || c3 > 0xBF ||
        (b == 0xF0 && c1 < 0x90) || (b == 0xF4 && c1 > 0x8F))
      nx_panic("invalid UTF-8 sequence");
  }
  return n;
}
static inline int64_t nx_str_char_at(String *s, int64_t i) {
  int64_t n = nx_str_char_size(s, i);
  int64_t b = (unsigned char)s->d[i];
  if (n == 1) return b;
  int64_t c = n == 2 ? b - 0xC0 : n == 3 ? b - 0xE0 : b - 0xF0;
  for (int64_t k = 1; k < n; k++) c = c * 64 + ((unsigned char)s->d[i + k] - 0x80);
  return c;
}
static inline void nx_print_char(int64_t c) { char b[4]; int n = nx_char_encode(c, b); flockfile(stdout); fwrite(b, 1, (size_t)n, stdout); putchar('\n'); funlockfile(stdout); }
static inline void nx_str_free(String *s) { free(s->d); s->d = 0; s->len = s->cap = 0; }
static inline void nx_print_str(Str x) { flockfile(stdout); if (x.n > 0) fwrite(x.p, 1, (size_t)x.n, stdout); putchar('\n'); funlockfile(stdout); }
/* raw intrinsics (docs/STDLIB_IO.md); the C backend is only the oracle, so a raw syscall goes through libc */
static int64_t nx_globals[64];
static inline _Atomic int64_t *nx_atomic_word(int64_t p) { return (_Atomic int64_t *)(intptr_t)p; }
static inline int64_t nx_atomic_load(int64_t p) { return atomic_load(nx_atomic_word(p)); }
static inline void nx_atomic_store(int64_t p, int64_t x) { atomic_store(nx_atomic_word(p), x); }
static inline int64_t nx_atomic_cmpxchg(int64_t p, int64_t expected, int64_t desired) {
  atomic_compare_exchange_strong(nx_atomic_word(p), &expected, desired);
  return expected;
}
static inline int64_t nx_atomic_fetch_add(int64_t p, int64_t x) { return atomic_fetch_add(nx_atomic_word(p), x); }
static inline void nx_unlock(int64_t address) {
  int64_t old = atomic_exchange(nx_atomic_word(address), nx_unwinding ? 3 : 0);
  if (old == 2) nx_futex_wake(address, nx_unwinding ? INT32_MAX : 1);
}
static inline int64_t nx_syscall(int64_t n, int64_t a, int64_t b, int64_t c, int64_t d, int64_t e, int64_t f) {
  fflush(stdout); /* keep the order of print() and raw writes */
  long r = syscall((long)n, (long)a, (long)b, (long)c, (long)d, (long)e, (long)f);
  return r == -1 ? -(int64_t)errno : (int64_t)r;
}
static inline int64_t nx_alloc(int64_t n) { void *p = malloc(n > 0 ? (size_t)n : 1); if (!p) nx_panic("out of memory"); return (int64_t)(intptr_t)p; }
static inline void nx_free(int64_t p) { free((void *)(intptr_t)p); }
static inline int64_t nx_load8(int64_t p) { return *(uint8_t *)(intptr_t)p; }
static inline int64_t nx_load64(int64_t p) { return *(int64_t *)(intptr_t)p; }
static inline void nx_store8(int64_t p, int64_t v) { *(uint8_t *)(intptr_t)p = (uint8_t)v; }
static inline void nx_store64(int64_t p, int64_t v) { *(int64_t *)(intptr_t)p = v; }
static inline void nx_assert(bool c) { if (!c) nx_panic("assertion failed"); }
static inline int64_t nx_add(int64_t a, int64_t b) { int64_t r; if (__builtin_add_overflow(a, b, &r)) nx_panic("integer overflow in addition"); return r; }
static inline int64_t nx_sub(int64_t a, int64_t b) { int64_t r; if (__builtin_sub_overflow(a, b, &r)) nx_panic("integer overflow in subtraction"); return r; }
static inline int64_t nx_mul(int64_t a, int64_t b) { int64_t r; if (__builtin_mul_overflow(a, b, &r)) nx_panic("integer overflow in multiplication"); return r; }
static inline int64_t nx_div(int64_t a, int64_t b) { if (b == 0) nx_panic("division by zero"); if (a == INT64_MIN && b == -1) nx_panic("integer overflow in division"); return a / b; }
static inline int64_t nx_mod(int64_t a, int64_t b) { if (b == 0) nx_panic("modulo by zero"); if (a == INT64_MIN && b == -1) return 0; return a % b; }
static inline int64_t nx_neg(int64_t a) { if (a == INT64_MIN) nx_panic("integer overflow in negation"); return -a; }

/* ---- numbers: sized integers (checked like Int), floats (IEEE-754, no traps) ---- */
#define NX_INT(S, T, MINV, LO, HI, LOF, HIF) \
static inline T nx_add_##S(T a, T b) { T r; if (__builtin_add_overflow(a, b, &r)) nx_panic("integer overflow in addition"); return r; } \
static inline T nx_sub_##S(T a, T b) { T r; if (__builtin_sub_overflow(a, b, &r)) nx_panic("integer overflow in subtraction"); return r; } \
static inline T nx_mul_##S(T a, T b) { T r; if (__builtin_mul_overflow(a, b, &r)) nx_panic("integer overflow in multiplication"); return r; } \
static inline T nx_div_##S(T a, T b) { if (b == 0) nx_panic("division by zero"); if ((MINV) != 0 && a == (T)(MINV) && b == (T)-1) nx_panic("integer overflow in division"); return (T)(a / b); } \
static inline T nx_mod_##S(T a, T b) { if (b == 0) nx_panic("modulo by zero"); if ((MINV) != 0 && a == (T)(MINV) && b == (T)-1) return 0; return (T)(a % b); } \
static inline T nx_wadd_##S(T a, T b) { return (T)((uint64_t)a + (uint64_t)b); } \
static inline T nx_wsub_##S(T a, T b) { return (T)((uint64_t)a - (uint64_t)b); } \
static inline T nx_wmul_##S(T a, T b) { return (T)((uint64_t)a * (uint64_t)b); } \
static inline T nx_neg_##S(T a) { if ((MINV) != 0 && a == (T)(MINV)) nx_panic("integer overflow in negation"); return (T)(-a); } \
static inline T nx_to_##S(__int128 v) { if (v < (__int128)(LO) || v > (__int128)(HI)) nx_panic("integer conversion out of range"); return (T)v; } \
static inline T nx_f2i_##S(double v) { if (!(LOF && v < (HIF))) nx_panic("float is NaN or out of range for the integer type"); return (T)v; }
NX_INT(i8, int8_t, INT8_MIN, INT8_MIN, INT8_MAX, v > -129.0, 128.0)
NX_INT(i16, int16_t, INT16_MIN, INT16_MIN, INT16_MAX, v > -32769.0, 32768.0)
NX_INT(i32, int32_t, INT32_MIN, INT32_MIN, INT32_MAX, v > -2147483649.0, 2147483648.0)
NX_INT(u8, uint8_t, 0, 0, UINT8_MAX, v > -1.0, 256.0)
NX_INT(u16, uint16_t, 0, 0, UINT16_MAX, v > -1.0, 65536.0)
NX_INT(u32, uint32_t, 0, 0, UINT32_MAX, v > -1.0, 4294967296.0)
NX_INT(u64, uint64_t, 0, 0, UINT64_MAX, v > -1.0, 18446744073709551616.0)
static inline int64_t nx_wadd(int64_t a, int64_t b) { return (int64_t)((uint64_t)a + (uint64_t)b); }
static inline int64_t nx_wsub(int64_t a, int64_t b) { return (int64_t)((uint64_t)a - (uint64_t)b); }
static inline int64_t nx_wmul(int64_t a, int64_t b) { return (int64_t)((uint64_t)a * (uint64_t)b); }
static inline int64_t nx_to_i64(__int128 v) { if (v < (__int128)INT64_MIN || v > (__int128)INT64_MAX) nx_panic("integer conversion out of range"); return (int64_t)v; }
__attribute__((noreturn)) static void nx_range_panic(__int128 v, int64_t lo, int64_t hi) {
  nx_try_worker_panic();
  if (v < (__int128)INT64_MIN || v > (__int128)INT64_MAX) fprintf(stderr, "panic: integer conversion out of range\n");
  else fprintf(stderr, "panic: integer conversion out of range: %lld not in %lld..%lld\n", (long long)v, (long long)lo, (long long)hi);
  exit(101);
}
static inline int64_t nx_to_range(__int128 v, int64_t lo, int64_t hi) { if (v < (__int128)lo || v > (__int128)hi) nx_range_panic(v, lo, hi); return (int64_t)v; }
static inline int64_t nx_f2i_i64(double v) { if (!(v >= -9223372036854775808.0 && v < 9223372036854775808.0)) nx_panic("float is NaN or out of range for the integer type"); return (int64_t)v; }
static inline double nx_sqrt(double x) { return __builtin_sqrt(x); }
static inline uint64_t nx_bits64(double x) { uint64_t u; memcpy(&u, &x, 8); return u; }
static inline uint32_t nx_bits32(float x) { uint32_t u; memcpy(&u, &x, 4); return u; }
static inline double nx_from_bits64(uint64_t u) { double x; memcpy(&x, &u, 8); return x; }
static inline float nx_from_bits32(uint32_t u) { float x; memcpy(&x, &u, 4); return x; }
