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
/* Source locations of panics: the generated program defines `nx_dbg` (a table of file:line:col, indexed by the
   `uint32_t l` that every checked operation receives as a constant; 0 = unknown). Programs without it (the
   self-hosted compiler's output) print no location. The location costs nothing on the fast path: the constant
   is only used when the check fails. */
typedef struct { uint32_t file, line, col; } NxLoc;
typedef struct { const NxLoc *locs; uint32_t nlocs; const char *const *files; } NxDebug;
extern const NxDebug nx_dbg __attribute__((weak));
/* Position of the user call into a library function (see cgen `tfuncs`); read only when that function fails. */
static uint32_t nx_cur_loc;
static void nx_backtrace(void);
/* the failure path is kept as small as a plain panic: heavier bodies change GCC's inlining of the callers */
__attribute__((noreturn, noinline, cold)) static void nx_panic_tail(uint32_t l) {
  if (l == 0xFFFFFFFFu) l = nx_cur_loc;
  if (&nx_dbg && l && l < nx_dbg.nlocs) fprintf(stderr, "  at %s:%u:%u\n", nx_dbg.files[nx_dbg.locs[l].file], nx_dbg.locs[l].line, nx_dbg.locs[l].col);
  nx_backtrace();
  exit(101);
}
__attribute__((noreturn)) static void nx_panic0(const char *m) { nx_try_worker_panic(); fprintf(stderr, "panic: %s\n", m); nx_panic_tail(0); }
__attribute__((noreturn)) static void nx_panic_at_(const char *m, uint32_t l) { nx_try_worker_panic(); fprintf(stderr, "panic: %s\n", m); nx_panic_tail(l); }
#define nx_panic_at(m, l) nx_panic_at_((m), (l))
/* programs of the self-hosted compiler define NX_SELFHOST_LOCS: their panics report `nx_cur_loc`, which the generated code sets around the sites that carry a position */
#ifdef NX_SELFHOST_LOCS
#define NX_LOC0 0xFFFFFFFFu
#else
#define NX_LOC0 0
#endif
#define nx_panic(m) nx_panic_at_((m), NX_LOC0)
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
__attribute__((noreturn)) static void nxl_panic_str(Str s, uint32_t l) {
  nx_try_worker_panic();
  fputs("panic: ", stderr); if (s.n > 0) fwrite(s.p, 1, (size_t)s.n, stderr); fputc('\n', stderr);
  nx_panic_tail(l);
}
#define nx_panic_str(s) nxl_panic_str((s), NX_LOC0)
static inline Str nx_str_text(const char *s) { Str r = { s, (int64_t)strlen(s) }; return r; }
static inline Str nx_str_view(const String *s) { Str r = { s->d, s->len }; return r; }
static inline String nx_str_new(void) { String s = {0, 0, 0}; return s; }
static inline void nx_str_push_str(String *s, Str x) {
  if (x.n < 0) nx_panic("negative string length");
  if (x.n == 0) return;
  if (x.n > INT64_MAX - s->len) nx_panic("string too long");
  int64_t need = s->len + x.n;
  if (need > s->cap) {
    int64_t nc = s->cap ? s->cap : 24; /* the usable size of glibc's smallest chunk: no extra memory, and "item=123;value=45;tail" fits */
    while (nc < need) nc = nc > INT64_MAX / 2 ? need : nc * 2;
    if ((uint64_t)nc > SIZE_MAX) nx_panic("string too long");
    char *nd = (char *)realloc(s->d, (size_t)nc);
    if (!nd) nx_panic("out of memory");
    s->d = nd; s->cap = nc;
  }
  memcpy(s->d + s->len, x.p, (size_t)x.n);
  s->len += x.n;
}
static inline void nx_str_reserve(String *s, int64_t extra) {
  if (extra <= 0) return;
  if (extra > INT64_MAX - s->len) nx_panic("string too long");
  int64_t need = s->len + extra;
  if (need > s->cap) {
    int64_t nc = need < 24 ? 24 : need;
    if ((uint64_t)nc > SIZE_MAX) nx_panic("string too long");
    char *nd = (char *)realloc(s->d, (size_t)nc);
    if (!nd) nx_panic("out of memory");
    s->d = nd; s->cap = nc;
  }
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
static inline int64_t nxl_str_byte(String *s, int64_t i, uint32_t l) {
  if (i < 0 || i >= s->len) nx_panic_at("index out of range", l);
  return (unsigned char)s->d[i];
}
static inline int64_t nx_str_byte(String *s, int64_t i) { return nxl_str_byte(s, i, 0); }
static inline void nx_str_clear(String *s) { s->len = 0; }
static inline void nxl_str_push_byte(String *s, int64_t b, uint32_t l) {
  if (b < 0 || b > 255) nx_panic_at("byte value out of range (0..255)", l);
  char c = (char)b; Str x = { &c, 1 }; nx_str_push_str(s, x);
}
static inline void nx_str_push_byte(String *s, int64_t b) { nxl_str_push_byte(s, b, 0); }
static inline void nxl_str_push_slice(String *s, Str x, int64_t a, int64_t b, uint32_t l) {
  if (b < 0 || b > x.n || a < 0 || a > b) nx_panic_at("index out of range", l);
  if (a == b) return;
  Str y = { x.p + a, b - a }; nx_str_push_str(s, y);
}
static inline void nx_str_push_slice(String *s, Str x, int64_t a, int64_t b) { nxl_str_push_slice(s, x, a, b, 0); }
static inline bool nx_char_valid(int64_t i) { return i >= 0 && i <= 0x10FFFF && !(i >= 0xD800 && i <= 0xDFFF); }
static inline int64_t nxl_char_from_int(int64_t i, uint32_t l) { if (!nx_char_valid(i)) nx_panic_at("invalid Unicode scalar value", l); return i; }
static inline int64_t nx_char_from_int(int64_t i) { return nxl_char_from_int(i, 0); }
static inline int64_t nx_char_utf8_len(int64_t c) { return c < 0x80 ? 1 : c < 0x800 ? 2 : c < 0x10000 ? 3 : 4; }
static inline int nx_char_encode(int64_t c, char *b) {
  if (c < 0x80) { b[0] = (char)c; return 1; }
  if (c < 0x800) { b[0] = (char)(0xC0 + c / 64); b[1] = (char)(0x80 + c % 64); return 2; }
  if (c < 0x10000) { b[0] = (char)(0xE0 + c / 4096); b[1] = (char)(0x80 + (c / 64) % 64); b[2] = (char)(0x80 + c % 64); return 3; }
  b[0] = (char)(0xF0 + c / 262144); b[1] = (char)(0x80 + (c / 4096) % 64); b[2] = (char)(0x80 + (c / 64) % 64); b[3] = (char)(0x80 + c % 64); return 4;
}
static inline void nx_str_push_char(String *s, int64_t c) { char b[4]; Str x = { b, nx_char_encode(c, b) }; nx_str_push_str(s, x); }
static inline int64_t nxl_str_char_size(String *s, int64_t i, uint32_t l) {
  int64_t b = nxl_str_byte(s, i, l);
  if (b < 0x80) return 1;
  int64_t n = b >= 0xC2 && b <= 0xDF ? 2 : b >= 0xE0 && b <= 0xEF ? 3 : b >= 0xF0 && b <= 0xF4 ? 4 : 0;
  if (!n || n > s->len - i) nx_panic_at("invalid UTF-8 sequence", l);
  int64_t c1 = (unsigned char)s->d[i + 1];
  if (c1 < 0x80 || c1 > 0xBF) nx_panic_at("invalid UTF-8 sequence", l);
  if (n == 3) {
    int64_t c2 = (unsigned char)s->d[i + 2];
    if (c2 < 0x80 || c2 > 0xBF || (b == 0xE0 && c1 < 0xA0) || (b == 0xED && c1 > 0x9F))
      nx_panic_at("invalid UTF-8 sequence", l);
  }
  if (n == 4) {
    int64_t c2 = (unsigned char)s->d[i + 2];
    int64_t c3 = (unsigned char)s->d[i + 3];
    if (c2 < 0x80 || c2 > 0xBF || c3 < 0x80 || c3 > 0xBF ||
        (b == 0xF0 && c1 < 0x90) || (b == 0xF4 && c1 > 0x8F))
      nx_panic_at("invalid UTF-8 sequence", l);
  }
  return n;
}
static inline int64_t nx_str_char_size(String *s, int64_t i) { return nxl_str_char_size(s, i, 0); }
static inline int64_t nxl_str_char_at(String *s, int64_t i, uint32_t l) {
  int64_t n = nxl_str_char_size(s, i, l);
  int64_t b = (unsigned char)s->d[i];
  if (n == 1) return b;
  int64_t c = n == 2 ? b - 0xC0 : n == 3 ? b - 0xE0 : b - 0xF0;
  for (int64_t k = 1; k < n; k++) c = c * 64 + ((unsigned char)s->d[i + k] - 0x80);
  return c;
}
static inline int64_t nx_str_char_at(String *s, int64_t i) { return nxl_str_char_at(s, i, 0); }
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
static inline void nxl_assert(bool c, uint32_t l) { if (!c) nx_panic_at("assertion failed", l); }
static inline void nx_assert(bool c) { nxl_assert(c, NX_LOC0); }
static inline int64_t nxl_add(int64_t a, int64_t b, uint32_t l) { int64_t r; if (__builtin_add_overflow(a, b, &r)) nx_panic_at("integer overflow in addition", l); return r; }
static inline int64_t nx_add(int64_t a, int64_t b) { return nxl_add(a, b, 0); }
static inline int64_t nxl_sub(int64_t a, int64_t b, uint32_t l) { int64_t r; if (__builtin_sub_overflow(a, b, &r)) nx_panic_at("integer overflow in subtraction", l); return r; }
static inline int64_t nx_sub(int64_t a, int64_t b) { return nxl_sub(a, b, 0); }
static inline int64_t nxl_mul(int64_t a, int64_t b, uint32_t l) { int64_t r; if (__builtin_mul_overflow(a, b, &r)) nx_panic_at("integer overflow in multiplication", l); return r; }
static inline int64_t nx_mul(int64_t a, int64_t b) { return nxl_mul(a, b, 0); }
static inline int64_t nxl_div(int64_t a, int64_t b, uint32_t l) { if (b == 0) nx_panic_at("division by zero", l); if (a == INT64_MIN && b == -1) nx_panic_at("integer overflow in division", l); return a / b; }
static inline int64_t nx_div(int64_t a, int64_t b) { return nxl_div(a, b, 0); }
static inline int64_t nxl_mod(int64_t a, int64_t b, uint32_t l) { if (b == 0) nx_panic_at("modulo by zero", l); if (a == INT64_MIN && b == -1) return 0; return a % b; }
static inline int64_t nx_mod(int64_t a, int64_t b) { return nxl_mod(a, b, 0); }
static inline int64_t nxl_neg(int64_t a, uint32_t l) { if (a == INT64_MIN) nx_panic_at("integer overflow in negation", l); return -a; }
static inline int64_t nx_neg(int64_t a) { return nxl_neg(a, 0); }

/* ---- numbers: sized integers (checked like Int), floats (IEEE-754, no traps) ---- */
#define NX_INT(S, T, MINV, LO, HI, LOF, HIF) \
static inline T nxl_add_##S(T a, T b, uint32_t l) { T r; if (__builtin_add_overflow(a, b, &r)) nx_panic_at("integer overflow in addition", l); return r; } \
static inline T nxl_sub_##S(T a, T b, uint32_t l) { T r; if (__builtin_sub_overflow(a, b, &r)) nx_panic_at("integer overflow in subtraction", l); return r; } \
static inline T nxl_mul_##S(T a, T b, uint32_t l) { T r; if (__builtin_mul_overflow(a, b, &r)) nx_panic_at("integer overflow in multiplication", l); return r; } \
static inline T nxl_div_##S(T a, T b, uint32_t l) { if (b == 0) nx_panic_at("division by zero", l); if ((MINV) != 0 && a == (T)(MINV) && b == (T)-1) nx_panic_at("integer overflow in division", l); return (T)(a / b); } \
static inline T nxl_mod_##S(T a, T b, uint32_t l) { if (b == 0) nx_panic_at("modulo by zero", l); if ((MINV) != 0 && a == (T)(MINV) && b == (T)-1) return 0; return (T)(a % b); } \
static inline T nxl_neg_##S(T a, uint32_t l) { if ((MINV) != 0 && a == (T)(MINV)) nx_panic_at("integer overflow in negation", l); return (T)(-a); } \
static inline T nxl_to_##S(__int128 v, uint32_t l) { if (v < (__int128)(LO) || v > (__int128)(HI)) nx_panic_at("integer conversion out of range", l); return (T)v; } \
static inline T nxl_f2i_##S(double v, uint32_t l) { if (!(LOF && v < (HIF))) nx_panic_at("float is NaN or out of range for the integer type", l); return (T)v; } \
static inline T nx_add_##S(T a, T b) { return nxl_add_##S(a, b, 0); } \
static inline T nx_sub_##S(T a, T b) { return nxl_sub_##S(a, b, 0); } \
static inline T nx_mul_##S(T a, T b) { return nxl_mul_##S(a, b, 0); } \
static inline T nx_div_##S(T a, T b) { return nxl_div_##S(a, b, 0); } \
static inline T nx_mod_##S(T a, T b) { return nxl_mod_##S(a, b, 0); } \
static inline T nx_neg_##S(T a) { return nxl_neg_##S(a, 0); } \
static inline T nx_to_##S(__int128 v) { return nxl_to_##S(v, 0); } \
static inline T nx_f2i_##S(double v) { return nxl_f2i_##S(v, 0); } \
static inline T nx_wadd_##S(T a, T b) { return (T)((uint64_t)a + (uint64_t)b); } \
static inline T nx_wsub_##S(T a, T b) { return (T)((uint64_t)a - (uint64_t)b); } \
static inline T nx_wmul_##S(T a, T b) { return (T)((uint64_t)a * (uint64_t)b); }
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
static inline int64_t nxl_to_i64(__int128 v, uint32_t l) { if (v < (__int128)INT64_MIN || v > (__int128)INT64_MAX) nx_panic_at("integer conversion out of range", l); return (int64_t)v; }
static inline int64_t nx_to_i64(__int128 v) { return nxl_to_i64(v, 0); }
__attribute__((noreturn, noinline)) static void nx_range_panic(__int128 v, int64_t lo, int64_t hi, uint32_t l) {
  nx_try_worker_panic();
  if (v < (__int128)INT64_MIN || v > (__int128)INT64_MAX) fprintf(stderr, "panic: integer conversion out of range\n");
  else fprintf(stderr, "panic: integer conversion out of range: %lld not in %lld..%lld\n", (long long)v, (long long)lo, (long long)hi);
  nx_panic_tail(l);
}
static inline int64_t nxl_to_range(__int128 v, int64_t lo, int64_t hi, uint32_t l) { if (v < (__int128)lo || v > (__int128)hi) nx_range_panic(v, lo, hi, l); return (int64_t)v; }
static inline int64_t nx_to_range(__int128 v, int64_t lo, int64_t hi) { return nxl_to_range(v, lo, hi, 0); }
static inline int64_t nxl_f2i_i64(double v, uint32_t l) { if (!(v >= -9223372036854775808.0 && v < 9223372036854775808.0)) nx_panic_at("float is NaN or out of range for the integer type", l); return (int64_t)v; }
static inline int64_t nx_f2i_i64(double v) { return nxl_f2i_i64(v, 0); }
static inline double nx_sqrt(double x) { return __builtin_sqrt(x); }
static inline uint64_t nx_bits64(double x) { uint64_t u; memcpy(&u, &x, 8); return u; }
static inline uint32_t nx_bits32(float x) { uint32_t u; memcpy(&u, &x, 4); return u; }
static inline double nx_from_bits64(uint64_t u) { double x; memcpy(&x, &u, 8); return x; }
static inline float nx_from_bits32(uint32_t u) { float x; memcpy(&x, &u, 4); return x; }

/* YERUX_BACKTRACE=1: the Yerux functions on the call stack, innermost first. The names come from the ELF symbol
   table of the running executable (`nx_f_<name>` symbols, `.cold` / `.constprop` parts folded into their
   function), so the fast path pays nothing. Functions the C compiler inlined have no frame of their own. */
/* The stack backtrace (YERUX_BACKTRACE) is compiled in only for --debug or a build made with YERUX_BACKTRACE set
   (-DNX_BACKTRACE): the code is big and, merely by existing, moved the hot code of a release build (textops +14 %). */
#ifdef NX_BACKTRACE
#include <execinfo.h>
#include <elf.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
static void nx_backtrace(void) {
  const char *on = getenv("YERUX_BACKTRACE");
  if (!on || !*on || !strcmp(on, "0")) return;
  void *pcs[256];
  int n = backtrace(pcs, 256);
  int fd = open("/proc/self/exe", O_RDONLY);
  struct stat st;
  void *map = MAP_FAILED;
  if (fd >= 0 && fstat(fd, &st) == 0) map = mmap(0, (size_t)st.st_size, PROT_READ, MAP_PRIVATE, fd, 0);
  if (fd >= 0) close(fd);
  fputs("stack backtrace:\n", stderr);
  if (map == MAP_FAILED) { fputs("   (unavailable: cannot read the executable)\n", stderr); return; }
  const unsigned char *base = (const unsigned char *)map;
  const Elf64_Ehdr *eh = (const Elf64_Ehdr *)base;
  const Elf64_Shdr *sh = (const Elf64_Shdr *)(base + eh->e_shoff);
  const Elf64_Sym *syms = 0; size_t nsyms = 0; const char *strs = 0;
  for (int i = 0; i < eh->e_shnum; i++)
    if (sh[i].sh_type == SHT_SYMTAB) {
      syms = (const Elf64_Sym *)(base + sh[i].sh_offset); nsyms = sh[i].sh_size / sizeof(Elf64_Sym);
      strs = (const char *)(base + sh[sh[i].sh_link].sh_offset);
    }
  if (!syms) { fputs("   (unavailable: the executable has no symbol table)\n", stderr); munmap(map, (size_t)st.st_size); return; }
  uintptr_t bias = 0; /* load address of a position-independent executable: where this function is minus where the file says */
  for (size_t j = 0; j < nsyms; j++)
    if (ELF64_ST_TYPE(syms[j].st_info) == STT_FUNC && !strcmp(strs + syms[j].st_name, "nx_backtrace")) { bias = (uintptr_t)nx_backtrace - syms[j].st_value; break; }
  int shown = 0;
  for (int k = 0; k < n; k++) {
    uintptr_t pc = (uintptr_t)pcs[k] - bias - 1; /* the call instruction, not the one after it */
    for (size_t j = 0; j < nsyms; j++) {
      if (ELF64_ST_TYPE(syms[j].st_info) != STT_FUNC || pc < syms[j].st_value || pc >= syms[j].st_value + syms[j].st_size) continue;
      const char *nm = strs + syms[j].st_name;
      if (strncmp(nm, "nx_f_", 5)) break;
      nm += 5;
      size_t len = strcspn(nm, ".");
      fprintf(stderr, "  %2d: %.*s\n", shown++, (int)len, nm);
      break;
    }
  }
  if (!shown) fputs("   (no Yerux frames: the C compiler inlined them; build with --debug, or with YERUX_BACKTRACE set)\n", stderr);
  munmap(map, (size_t)st.st_size);
}
#else
static void nx_backtrace(void) {
  const char *on = getenv("YERUX_BACKTRACE");
  if (on && *on && strcmp(on, "0")) fputs("note: no stack backtrace in this build; rebuild with --debug or with YERUX_BACKTRACE=1 set\n", stderr);
}
#endif
