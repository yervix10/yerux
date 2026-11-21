// Yerux C backend (temporary: it will be replaced by Cranelift/LLVM).
use crate::check::Sigs;
use crate::dropscc::HeapNode;
use crate::numeric::Nk;
use crate::syntax::*;
use std::collections::{HashMap, HashSet};

const C_HEADER: &str = r#"#include <stdint.h>
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
static void nx_try_worker_panic(void) {
  if (!nx_worker_panic) return;
  jmp_buf *target = nx_worker_panic;
  nx_worker_panic = NULL;
  nx_unwinding = true;
  while (nx_cleanup_head) {
    NxCleanup *slot = nx_cleanup_head;
    nx_cleanup_pop();
    if (*slot->live) slot->drop(slot->value);
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
  if (w->n == w->cap) { w->cap = w->cap ? w->cap * 2 : 8; w->d = (void **)realloc(w->d, w->cap * sizeof(void *)); if (!w->d) nx_panic("out of memory"); }
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
  nx_unwinding = false;
  nx_drop_fn(&task->entry);
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
  if (x.n == 0) return;
  if (x.n > INT64_MAX - s->len) nx_panic("string too long");
  int64_t need = s->len + x.n;
  if (need > s->cap) {
    int64_t nc = s->cap ? s->cap : 16;
    while (nc < need) nc = nc > INT64_MAX / 2 ? need : nc * 2;
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
  char buf[32]; int n = snprintf(buf, sizeof buf, "%lld", (long long)v);
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
  if (b >= 0xC0 && b < 0xE0) return 2;
  if (b >= 0xE0 && b < 0xF0) return 3;
  if (b >= 0xF0 && b < 0xF8) return 4;
  nx_panic("invalid UTF-8 leading byte");
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


"#;

struct Gen<'a> {
    out: String,
    ind: usize,
    types: &'a Types,
    sigs: &'a Sigs,
    externs: HashSet<String>,
    vars: HashMap<usize, (String, bool, Ty)>, // id -> (C name, is reference, type)
    capture_flags: HashSet<usize>,
    recover_panics: bool,
    scopes: Vec<Vec<usize>>, // owned variables per scope (to free)
    tmp: usize,
    loops: Vec<std::rc::Rc<CLoop>>,
}

/// An enclosing loop while its body is generated: `break`/`continue` free the scopes inside it, then `goto`.
/// Shared with the generators of expression scopes (`EK::Sequence`), whose statements may leave the loop.
struct CLoop {
    id: usize,
    depth: usize, // number of scopes open outside the loop body
    brk: std::cell::Cell<bool>,
    cont: std::cell::Cell<bool>,
}

pub fn generate(prog: &Program, sigs: Sigs) -> String {
    let mut g = Gen {
        out: String::from(C_HEADER),
        ind: 0,
        types: &prog.types,
        sigs: &sigs,
        externs: prog.externs.iter().map(|f| f.name.clone()).collect(),
        vars: HashMap::new(),
        capture_flags: HashSet::new(),
        recover_panics: prog.has_recoverable_workers(),
        scopes: vec![],
        tmp: 0,
        loops: vec![],
    };
    // 1. forward declarations (types may refer to each other through List<T> and Box<T>)
    let mut all: Vec<Ty> = vec![];
    for i in 0..g.types.structs.len() {
        all.push(Ty::Struct(i));
    }
    for i in 0..g.types.enums.len() {
        all.push(Ty::Enum(i));
    }
    // a slice `[T]` has the C type of `List<T>`: emit that type once (prefer the List entry itself)
    let mut list_c: HashSet<String> = HashSet::new();
    let mut lists: Vec<usize> = (0..g.types.lists.len()).collect();
    lists.sort_by_key(|i| g.types.slices.contains(i));
    for i in lists {
        if !g.types.has_param(Ty::List(i)) && list_c.insert(g.types.cty(Ty::List(i))) {
            all.push(Ty::List(i));
        }
    }
    let all: Vec<Ty> = all.into_iter().filter(|t| !g.types.has_param(*t)).collect();
    for t in &all {
        let c = g.types.cty(*t);
        g.line(&format!("typedef struct {c} {c};"));
    }
    for i in 0..g.types.boxes.len() {
        let t = Ty::Box(i);
        if g.types.has_param(t) {
            continue;
        }
        let inner = g.types.cty(g.types.boxes[i]);
        let c = g.types.cty(t);
        g.line(&format!("typedef {inner} *{c};"));
    }
    // 2. bodies, in dependency order (a value inside a value must be complete first)
    let mut done: HashSet<Ty> = HashSet::new();
    for t in &all {
        g.emit_body(*t, &mut done);
    }
    // 3. drop functions: declared first because owned types refer to each other (recursion)
    let mut owned_tys: Vec<Ty> = all
        .iter()
        .copied()
        .filter(|t| g.types.owned(*t) || matches!(t, Ty::List(_)))
        .collect();
    for i in 0..g.types.boxes.len() {
        if !g.types.has_param(Ty::Box(i)) {
            owned_tys.push(Ty::Box(i));
        }
    }
    for t in &owned_tys {
        let c = g.types.cty(*t);
        match t {
            Ty::List(_) => g.line(&format!("static inline void nx_free_{c}({c} *l);")),
            _ => g.line(&format!("static void nx_drop_{c}({c} *s);")),
        }
    }
    // components of mutually recursive heap nodes are dropped with a worklist (one shared function each)
    let mut sccs: Vec<Vec<HeapNode>> = vec![];
    for t in &owned_tys {
        let node = match t {
            Ty::Box(b) => HeapNode::Box(*b),
            Ty::List(l) => HeapNode::List(*l),
            _ => continue,
        };
        if let Some(m) = g.types.general_scc(node) {
            if !sccs.contains(&m) {
                sccs.push(m);
            }
        }
    }
    if !sccs.is_empty() {
        g.line("typedef struct { int tag; void *a; int64_t b; } NxWI;");
        for m in &sccs {
            let n = g.scc_name(m);
            g.line(&format!("static void {n}(int tag, void *a, int64_t b);"));
        }
    }
    // 4. functions of every type
    for t in &all {
        g.emit_fns(*t);
    }
    for t in &owned_tys {
        if let Ty::Box(_) = t {
            g.emit_fns(*t);
        }
    }
    for m in &sccs {
        g.emit_scc(m);
    }
    if g.recover_panics {
        for t in owned_tys.iter().copied().chain([Ty::String]) {
            g.emit_cleanup(t);
        }
        // All function values and erased objects have one C representation each.
        g.line("static void nx_cleanup_NxFn(void *p) { nx_drop_fn(p); }");
        g.line("static void nx_cleanup_NxDyn(void *p) { nx_drop_dyn(p); }");
    }
    g.line("");
    for f in &prog.externs {
        let ps: Vec<String> = f.params.iter().map(|p| g.types.cty(p.ty)).collect();
        let ps = if f.variadic {
            format!("{}, ...", ps.join(", "))
        } else if ps.is_empty() {
            "void".to_string()
        } else {
            ps.join(", ")
        };
        // Declared under a private name bound to the C symbol with an asm label, so a Yerux prototype
        // (`Ptr<UInt8>` instead of `char *`) never conflicts with a libc header declaration.
        g.line(&format!(
            "extern {} nx_x_{}({}) __asm__(\"{}\");",
            g.types.cty(f.ret),
            f.name,
            ps,
            f.name
        ));
    }
    for f in &prog.funcs {
        let ps: Vec<String> = f
            .params
            .iter()
            .map(|p| {
                if g.is_ref(p.mode, p.ty) {
                    format!("{} *", g.types.cty(p.ty))
                } else {
                    g.types.cty(p.ty)
                }
            })
            .collect();
        let ps = if ps.is_empty() {
            "void".to_string()
        } else {
            ps.join(", ")
        };
        let line = format!("static {} nx_f_{}({});", g.types.cty(f.ret), f.name, ps);
        g.line(&line);
    }
    g.line("");
    g.emit_dyn_tables(&prog.funcs);
    for f in &prog.funcs {
        g.func(f);
    }
    // global cell 0 holds the address of an image of the initial process stack (argc, argv.., 0, envp.., 0), as
    // the native backend's _start stores it
    g.line("int main(int argc, char **argv) { int ne = 0; while (environ && environ[ne]) ne++; int64_t st[argc + ne + 3]; int k = 0; st[k++] = argc; for (int i = 0; i < argc; i++) st[k++] = (int64_t)(intptr_t)argv[i]; st[k++] = 0; for (int i = 0; i < ne; i++) st[k++] = (int64_t)(intptr_t)environ[i]; st[k++] = 0; nx_globals[0] = (int64_t)(intptr_t)st; nx_f_main(); fflush(stdout); return 0; }");
    g.out
}

impl Gen<'_> {
    fn line(&mut self, s: &str) {
        for _ in 0..self.ind {
            self.out.push_str("  ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn emit_dyn_tables(&mut self, funcs: &[Func]) {
        let funcs: HashMap<_, _> = funcs.iter().map(|f| (f.name.as_str(), f)).collect();
        for (id, table) in self.types.dyn_tables.iter().enumerate() {
            let drop_name = format!("nx_dyn_{}_drop", id);
            let drop = if self.types.owned(table.concrete) {
                self.drop_call(
                    table.concrete,
                    &format!("(({} *)data)", self.types.cty(table.concrete)),
                ) + ";"
            } else {
                "(void)data;".into()
            };
            self.line(&format!("static void {drop_name}(void *data) {{ {drop} }}"));
            let mut slots = vec![drop_name];
            for (slot, method) in table.methods.iter().enumerate() {
                let f = funcs[method.as_str()];
                let wrapper = format!("nx_dyn_{}_{slot}", id);
                let mut params = vec!["void *data".to_string()];
                let cty = self.types.cty(table.concrete);
                let deref = if self.is_ref(f.params[0].mode, table.concrete) {
                    ""
                } else {
                    "*"
                };
                let mut args = vec![format!("{deref}(({cty} *)data)")];
                for (i, param) in f.params.iter().enumerate().skip(1) {
                    let ty = self.types.cty(param.ty);
                    let pointer = if self.is_ref(param.mode, param.ty) {
                        "*"
                    } else {
                        ""
                    };
                    params.push(format!("{ty} {pointer}a{i}"));
                    args.push(format!("a{i}"));
                }
                let ret = if f.ret == Ty::Unit { "" } else { "return " };
                self.line(&format!(
                    "static {} {wrapper}({}) {{ {ret}nx_f_{method}({}); }}",
                    self.types.cty(f.ret),
                    params.join(", "),
                    args.join(", ")
                ));
                slots.push(wrapper);
            }
            let slots: Vec<_> = slots.iter().map(|f| format!("(NxDynSlot){f}")).collect();
            self.line(&format!(
                "static const NxDynSlot nx_vtable_{id}[] = {{ {} }};",
                slots.join(", ")
            ));
        }
    }

    fn is_ref(&self, mode: Mode, ty: Ty) -> bool {
        // a slice is passed as a copy of its {ptr, len, cap} header (it can never change the header)
        if self.types.slice_elem(ty).is_some() {
            return false;
        }
        mode == Mode::Mut || (mode == Mode::View && self.types.owned(ty))
    }

    fn plain_struct(&self, sid: usize) -> bool {
        !self.types.is_lock_guard(sid)
            && self.types.join_payload(sid).is_none()
            && self.types.runtime_struct(sid, "Shared").is_none()
            && !self.types.structs[sid].cname.starts_with("Env__")
    }

    /// The most cells of box type `bi` that dropping one value of `ty` reaches directly (see ir.rs `box_fanout`).
    fn box_fanout(&self, ty: Ty, bi: usize) -> usize {
        match ty {
            Ty::Box(b) => (b == bi) as usize,
            Ty::Struct(sid) if self.plain_struct(sid) => self.types.structs[sid]
                .fields
                .iter()
                .map(|(_, f)| self.box_fanout(*f, bi))
                .sum(),
            Ty::Enum(eid) => self.types.enums[eid]
                .variants
                .iter()
                .map(|(_, ps)| ps.iter().map(|p| self.box_fanout(*p, bi)).sum::<usize>())
                .max()
                .unwrap_or(0),
            _ => 0,
        }
    }

    /// Statements dropping the value at pointer expression `addr` while pushing nested cells of box type `bi`
    /// on the worklist `w` (`n`, `cap` are its counters) instead of recursing into them.
    fn defer_c(&self, ty: Ty, addr: &str, bi: usize, bc: &str) -> String {
        if self.box_fanout(ty, bi) == 0 {
            return if self.types.owned(ty) {
                format!("{};", self.drop_call(ty, addr))
            } else {
                String::new()
            };
        }
        match ty {
            Ty::Box(_) => format!("if (n == cap) {{ cap *= 2; w = realloc(w, cap * sizeof({bc})); }} w[n++] = *({addr});"),
            Ty::Struct(sid) => {
                let mut b = String::new();
                for (n, f) in &self.types.structs[sid].fields {
                    if self.types.owned(*f) {
                        b += &self.defer_c(*f, &format!("&({addr})->f_{n}"), bi, bc);
                    }
                }
                b
            }
            Ty::Enum(eid) => {
                let def = &self.types.enums[eid];
                let mut b = format!("switch (({addr})->tag) {{");
                for (vi, (vn, ps)) in def.variants.iter().enumerate() {
                    let drops: String = ps
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| self.types.owned(**p))
                        .map(|(k, p)| self.defer_c(*p, &format!("&({addr})->u.v_{vn}.a{k}"), bi, bc))
                        .collect();
                    if !drops.is_empty() {
                        b += &format!(" case {vi}: {{ {drops} }} break;");
                    }
                }
                b + " default: break; }"
            }
            _ => String::new(),
        }
    }

    /// C name of the shared worklist function of a component of heap nodes (see dropscc.rs).
    fn scc_name(&self, members: &[HeapNode]) -> String {
        match members[0] {
            HeapNode::Box(b) => format!("nx_scc_b{b}"),
            HeapNode::List(l) => format!("nx_scc_l{l}"),
        }
    }

    /// Like `defer_c` for a component: nested member boxes and lists are pushed as `{tag, a, b}` items.
    fn defer_scc_c(&self, ty: Ty, addr: &str, members: &[HeapNode]) -> String {
        if !self.types.reaches_node(ty, members) {
            return if self.types.owned(ty) {
                format!("{};", self.drop_call(ty, addr))
            } else {
                String::new()
            };
        }
        let push = |tag: usize, a: String, b: String| {
            format!("if (n == cap) {{ cap *= 2; w = (NxWI *)realloc(w, cap * sizeof(NxWI)); if (!w) nx_panic(\"out of memory\"); }} w[n].tag = {tag}; w[n].a = (void *)({a}); w[n].b = {b}; n++;")
        };
        match ty {
            Ty::Box(b) => {
                let tag = members.iter().position(|m| *m == HeapNode::Box(b)).unwrap();
                push(tag, format!("*({addr})"), "0".into())
            }
            Ty::List(l) => {
                let tag = members
                    .iter()
                    .position(|m| *m == HeapNode::List(l))
                    .unwrap();
                push(tag, format!("({addr})->d"), format!("({addr})->len"))
            }
            Ty::Struct(sid) => {
                let mut b = String::new();
                for (n, f) in &self.types.structs[sid].fields {
                    if self.types.owned(*f) {
                        b += &self.defer_scc_c(*f, &format!("&({addr})->f_{n}"), members);
                    }
                }
                b
            }
            Ty::Enum(eid) => {
                let def = &self.types.enums[eid];
                let mut b = format!("switch (({addr})->tag) {{");
                for (vi, (vn, ps)) in def.variants.iter().enumerate() {
                    let drops: String = ps
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| self.types.owned(**p))
                        .map(|(k, p)| {
                            self.defer_scc_c(*p, &format!("&({addr})->u.v_{vn}.a{k}"), members)
                        })
                        .collect();
                    if !drops.is_empty() {
                        b += &format!(" case {vi}: {{ {drops} }} break;");
                    }
                }
                b + " default: break; }"
            }
            _ => String::new(),
        }
    }

    /// The worklist function of one component: drops the item `(tag, a, b)` and everything below it.
    fn emit_scc(&mut self, members: &[HeapNode]) {
        let name = self.scc_name(members);
        let mut cases = String::new();
        for (k, m) in members.iter().enumerate() {
            match m {
                HeapNode::Box(b) => {
                    let (bc, inner) = (self.types.cty(Ty::Box(*b)), self.types.boxes[*b]);
                    let body = self.defer_scc_c(inner, "c", members);
                    cases +=
                        &format!(" case {k}: {{ {bc} c = ({bc})it.a; {body} free(c); }} break;");
                }
                HeapNode::List(l) => {
                    let et = self.types.lists[*l];
                    let ec = self.types.cty(et);
                    let body = self.defer_scc_c(et, "&d[i]", members);
                    cases += &format!(" case {k}: {{ {ec} *d = ({ec} *)it.a; for (int64_t i = 0; i < it.b; i++) {{ {body} }} free(d); }} break;");
                }
            }
        }
        self.line(&format!(
            "static void {name}(int tag, void *a, int64_t b) {{ size_t n = 0, cap = 16; NxWI *w = (NxWI *)malloc(cap * sizeof(NxWI)); if (!w) nx_panic(\"out of memory\"); \
w[n].tag = tag; w[n].a = a; w[n].b = b; n++; \
while (n) {{ NxWI it = w[--n]; switch (it.tag) {{{cases} default: break; }} }} free(w); }}"
        ));
    }

    fn drop_call(&self, ty: Ty, addr: &str) -> String {
        match ty {
            Ty::List(_) => format!("nx_free_{}({})", self.types.cty(ty), addr),
            Ty::String => format!("nx_str_free({})", addr),
            Ty::Struct(_) => format!("nx_drop_{}({})", self.types.cty(ty), addr),
            Ty::Fn(_) => format!("nx_drop_fn({})", addr),
            Ty::Dyn(_) => format!("nx_drop_dyn({})", addr),
            Ty::Enum(_) | Ty::Box(_) => format!("nx_drop_{}({})", self.types.cty(ty), addr),
            _ => String::new(),
        }
    }

    /// The `struct` definition of a type (everything it holds by value is defined before it).
    fn emit_body(&mut self, t: Ty, done: &mut HashSet<Ty>) {
        if !matches!(t, Ty::Struct(_) | Ty::Enum(_) | Ty::List(_))
            || self.types.has_param(t)
            || !done.insert(t)
        {
            return;
        }
        let cname = self.types.cty(t);
        match t {
            Ty::List(i) => {
                let et = self.types.cty(self.types.lists[i]);
                self.line(&format!("struct {cname} {{ {et} *d; int64_t len, cap; }};"));
            }
            Ty::Struct(i) if self.types.arrays.contains_key(&i) => {
                let (el, n) = self.types.arrays[&i];
                self.emit_body(el, done);
                let et = self.types.cty(el);
                let c = &cname;
                self.line(&format!("struct {c} {{ {et} e[{n}]; }};"));
                self.line(&format!("static inline {et} *nx_at_{c}({c} *a, int64_t i) {{ if (i < 0 || i >= {n}) nx_panic(\"index out of range\"); return &a->e[i]; }}"));
                self.line(&format!(
                    "static inline int64_t nx_len_{c}({c} *a) {{ (void)a; return {n}; }}"
                ));
                self.line(&format!("static inline void nx_set_{c}({c} *a, int64_t i, {et} v) {{ *nx_at_{c}(a, i) = v; }}"));
                self.line(&format!("static inline void nx_swap_{c}({c} *a, int64_t i, int64_t j) {{ {et} *x = nx_at_{c}(a, i); {et} *y = nx_at_{c}(a, j); {et} t = *x; *x = *y; *y = t; }}"));
            }
            Ty::Struct(i) => {
                let def = self.types.structs[i].clone();
                for (_, f) in &def.fields {
                    self.emit_body(*f, done);
                }
                let mut s = format!("struct {} {{", cname);
                if def.fields.is_empty() {
                    s += " char _pad;";
                }
                for (n, f) in &def.fields {
                    s += &format!(" {} f_{};", self.types.cty(*f), n);
                }
                s += " };";
                self.line(&s);
            }
            Ty::Enum(i) => {
                let def = self.types.enums[i].clone();
                for (_, ps) in &def.variants {
                    for p in ps {
                        self.emit_body(*p, done);
                    }
                }
                let mut s = format!("struct {} {{ int tag; union {{", cname);
                for (vn, ps) in &def.variants {
                    if ps.is_empty() {
                        continue;
                    }
                    s += " struct {";
                    for (k, p) in ps.iter().enumerate() {
                        s += &format!(" {} a{};", self.types.cty(*p), k);
                    }
                    s += &format!(" }} v_{};", vn);
                }
                s += " char _pad; } u; };";
                self.line(&s);
            }
            _ => {}
        }
    }

    /// The helper functions of a type: List operations, drop glue.
    fn emit_fns(&mut self, t: Ty) {
        if self.types.has_param(t) {
            return;
        }
        let cname = self.types.cty(t);
        match t {
            Ty::List(i) => {
                // one monomorphised function set per List<T> instantiation
                let el = self.types.lists[i];
                let et = self.types.cty(el);
                let c = &cname;
                let owned = self.types.owned(el);
                self.line(&format!(
                    "static inline {c} nx_new_{c}(void) {{ {c} l = {{0, 0, 0}}; return l; }}"
                ));
                self.line(&format!(
                    "static inline void nx_push_{c}({c} *l, {et} v) {{"
                ));
                self.line("  if (l->len == l->cap) {");
                self.line(&format!("    if (l->cap > INT64_MAX / 2 / (int64_t)sizeof({et})) nx_panic(\"list too large\");"));
                self.line("    int64_t nc = l->cap ? l->cap * 2 : 4;");
                self.line(&format!(
                    "    {et} *nd = ({et} *)realloc(l->d, (size_t)nc * sizeof({et}));"
                ));
                self.line("    if (!nd) nx_panic(\"out of memory\");");
                self.line("    l->d = nd; l->cap = nc;");
                self.line("  }");
                self.line("  l->d[l->len++] = v;");
                self.line("}");
                self.line(&format!(
                    "static inline int64_t nx_len_{c}({c} *l) {{ return l->len; }}"
                ));
                self.line(&format!(
                    "static inline {et} *nx_at_{c}({c} *l, int64_t i) {{"
                ));
                self.line("  if (i < 0 || i >= l->len) nx_panic(\"index out of range\");");
                self.line("  return &l->d[i];");
                self.line("}");
                self.line(&format!("static inline {et} nx_pop_{c}({c} *l) {{"));
                self.line("  if (l->len == 0) nx_panic(\"pop from an empty list\");");
                self.line("  return l->d[--l->len];");
                self.line("}");
                let drop_p = if owned {
                    format!(" {};", self.drop_call(el, "p"))
                } else {
                    String::new()
                };
                self.line(&format!("static inline void nx_set_{c}({c} *l, int64_t i, {et} v) {{ {et} *p = nx_at_{c}(l, i);{drop_p} *p = v; }}"));
                let drop_i = if owned {
                    format!(" {};", self.drop_call(el, "&l->d[i]"))
                } else {
                    String::new()
                };
                self.line(&format!("static inline void nx_swap_{c}({c} *l, int64_t i, int64_t j) {{ {et} *a = nx_at_{c}(l, i); {et} *b = nx_at_{c}(l, j); {et} t = *a; *a = *b; *b = t; }}"));
                // slices `[T]` share this header: elements [lo, hi) of a buffer, of a List, or of a slice
                self.line(&format!("static inline {c} nx_slice_{c}({et} *d, int64_t n, int64_t lo, int64_t hi) {{ if (hi < 0 || hi > n || lo < 0 || lo > hi) nx_panic(\"index out of range\"); {c} s = {{ d + lo, hi - lo, hi - lo }}; return s; }}"));
                self.line(&format!("static inline {c} nx_slicel_{c}({c} *l, int64_t lo, int64_t hi, bool whole) {{ return nx_slice_{c}(l->d, l->len, lo, whole ? l->len : hi); }}"));
                if let Some(members) = self.types.general_scc(HeapNode::List(i)) {
                    // recursive through lists/boxes: released by the component's worklist (clear gives the buffer up too)
                    let tag = members
                        .iter()
                        .position(|m| *m == HeapNode::List(i))
                        .unwrap();
                    let scc = self.scc_name(&members);
                    self.line(&format!("static inline void nx_free_{c}({c} *l) {{ void *d = l->d; int64_t n = l->len; l->d = 0; l->len = 0; l->cap = 0; {scc}({tag}, d, n); }}"));
                    self.line(&format!(
                        "static inline void nx_clear_{c}({c} *l) {{ nx_free_{c}(l); }}"
                    ));
                } else {
                    self.line(&format!("static inline void nx_clear_{c}({c} *l) {{ for (int64_t i = 0; i < l->len; i++) {{{drop_i} }} l->len = 0; }}"));
                    self.line(&format!("static inline void nx_free_{c}({c} *l) {{ nx_clear_{c}(l); free(l->d); l->d = 0; l->cap = 0; }}"));
                }
            }
            Ty::Struct(i) => {
                let def = self.types.structs[i].clone();
                if self.types.owned(t) {
                    let mut b = format!("static void nx_drop_{}({} *s) {{", cname, cname);
                    if self.types.is_lock_guard(i) {
                        b += " nx_unlock(s->f_address);";
                    }
                    if let Some(payload) = self.types.join_payload(i) {
                        b += " if (!s->f_thread_word) return; void *p = (void *)(intptr_t)nx_thread_wait(s->f_thread_word); if (p) {";
                        if self.types.owned(payload) {
                            b += &format!(" {};", self.drop_call(payload, "p"));
                        }
                        b += " free(p); }";
                    }
                    if let Some(payload) = self.types.runtime_struct(i, "Shared") {
                        b += " if (nx_atomic_fetch_add((int64_t)(intptr_t)s->f_cell, -1) == 1) {";
                        if self.types.owned(payload) {
                            b += &format!(" {};", self.drop_call(payload, "&s->f_cell->f_value"));
                        }
                        b += " free(s->f_cell); }";
                    } else {
                        for (n, f) in &def.fields {
                            if self.types.owned(*f) {
                                if let Some(flag) = self.types.capture_live_field(i, n) {
                                    b += &format!(" if (s->f_{flag})");
                                }
                                b += &format!(" {};", self.drop_call(*f, &format!("&s->f_{n}")));
                            }
                        }
                    }
                    b += " }";
                    self.line(&b);
                }
                if def.cname.starts_with("Env__") {
                    // closure environment: drop what it captured, then free the block
                    // captured closures are not dropped here: their environments go to the caller's worklist
                    let mut d = String::new();
                    for (n, f) in def.fields.iter() {
                        if !self.types.owned(*f) {
                            continue;
                        }
                        let guard = self
                            .types
                            .capture_live_field(i, n)
                            .map(|flag| format!("if (s->f_{flag}) "))
                            .unwrap_or_default();
                        if matches!(f, Ty::Fn(_)) {
                            d += &format!("{guard}nx_envpush(wl, s->f_{n}.env); ");
                        } else {
                            d += &format!("{guard}{}; ", self.drop_call(*f, &format!("&s->f_{n}")));
                        }
                    }
                    self.line(&format!("static void nx_envdrop_{c}(void *p, NxEnvWL *wl) {{ {c} *s = ({c} *)p; {d}free(p); }}", c = cname));
                }
            }
            Ty::Enum(i) => {
                if self.types.owned(t) {
                    let def = self.types.enums[i].clone();
                    let mut b = format!(
                        "static void nx_drop_{}({} *s) {{ switch (s->tag) {{",
                        cname, cname
                    );
                    for (vi, (vn, ps)) in def.variants.iter().enumerate() {
                        let drops: Vec<String> = ps
                            .iter()
                            .enumerate()
                            .filter(|(_, p)| self.types.owned(**p))
                            .map(|(k, p)| {
                                format!(
                                    "{};",
                                    self.drop_call(*p, &format!("&s->u.v_{}.a{}", vn, k))
                                )
                            })
                            .collect();
                        if !drops.is_empty() {
                            b += &format!(" case {}: {} break;", vi, drops.join(" "));
                        }
                    }
                    b += " default: break; } }";
                    self.line(&b);
                }
            }
            Ty::Box(i) => {
                let inner = self.types.boxes[i];
                if let Some(members) = self.types.general_scc(HeapNode::Box(i)) {
                    let tag = members.iter().position(|m| *m == HeapNode::Box(i)).unwrap();
                    let scc = self.scc_name(&members);
                    self.line(&format!("static void nx_drop_{cname}({cname} *b) {{ if (!*b) return; void *a = *b; *b = 0; {scc}({tag}, a, 0); }}"));
                    return;
                }
                if self.box_fanout(inner, i) > 0 {
                    // self-recursive box: visit nested cells with an explicit worklist (no recursion, any depth)
                    let body = self.defer_c(inner, "c", i, cname.as_str());
                    self.line(&format!(
                        "static void nx_drop_{cname}({cname} *b) {{ if (!*b) return; size_t n = 0, cap = 16; {cname} *w = malloc(cap * sizeof({cname})); w[n++] = *b; *b = 0; \
while (n) {{ {cname} c = w[--n]; {body} free(c); }} free(w); }}"
                    ));
                    return;
                }
                let d = if self.types.owned(inner) {
                    format!("{}; ", self.drop_call(inner, "*b"))
                } else {
                    String::new()
                };
                self.line(&format!("static void nx_drop_{cname}({cname} *b) {{ if (*b) {{ {d}free(*b); *b = 0; }} }}"));
            }
            _ => {}
        }
    }

    fn lv(&self, id: usize) -> String {
        let (n, r, _) = &self.vars[&id];
        if *r {
            format!("(*{})", n)
        } else {
            n.clone()
        }
    }

    fn place_c(&self, e: &Expr) -> String {
        self.place_with_indices(e, &mut |index| self.ex(index))
    }

    fn place_with_indices(&self, e: &Expr, index: &mut impl FnMut(&Expr) -> String) -> String {
        match &e.k {
            EK::Var(_, id) => self.lv(*id),
            EK::Field(b, f) => format!("{}.f_{}", self.place_with_indices(b, index), f),
            // `b.get()` / `*b`: the cell of a Box
            EK::Method { recv, m, args }
                if matches!(m.as_str(), "get" | "locked_value") && args.is_empty() =>
            {
                let place = self.place_with_indices(recv, index);
                if m == "locked_value" {
                    format!("(*{place}.f_value)")
                } else if self.types.shared_payload(recv.ty).is_some() {
                    format!("({place}.f_cell->f_value)")
                } else {
                    format!("(*{place})")
                }
            }
            // payload of an enum place (a borrowing `match` binding)
            EK::Payload(b, vid, k) => {
                let Ty::Enum(eid) = b.ty else { unreachable!() };
                format!(
                    "{}.u.v_{}.a{}",
                    self.place_with_indices(b, index),
                    self.types.enums[eid].variants[*vid].0,
                    k
                )
            }
            // `l.at(i)`: a bounds-checked pointer into the list, dereferenced
            EK::Method { recv, args, .. } => {
                let c = self.types.cty(recv.ty);
                let list = self.place_with_indices(recv, index);
                format!("(*nx_at_{}(&{}, {}))", c, list, index(&args[0]))
            }
            EK::Deref(p) => format!("(*({} *)(intptr_t)({}))", self.types.cty(e.ty), self.ex(p)),
            _ => format!("({})", self.ex(e)), // a field of a temporary: `pair().0`
        }
    }

    fn emit_cleanup(&mut self, ty: Ty) {
        let name = self.types.cty(ty);
        let drop = self.drop_call(ty, "p");
        self.line(&format!(
            "static void nx_cleanup_{name}(void *p) {{ {drop}; }}"
        ));
    }

    fn track(&mut self, id: usize) {
        let (name, _, ty) = self.vars[&id].clone();
        self.line(&format!("bool {name}_live = true;"));
        if self.recover_panics {
            let ty = self.types.cty(ty);
            self.line(&format!("NxCleanup {name}_cleanup;"));
            self.line(&format!(
                "nx_cleanup_push(&{name}_cleanup, &{name}_live, &{name}, nx_cleanup_{ty});"
            ));
        }
        self.scopes.last_mut().unwrap().push(id);
    }

    fn drop_local(&self, id: usize) -> String {
        let (name, _, ty) = &self.vars[&id];
        let pop = if self.recover_panics {
            "nx_cleanup_pop(); "
        } else {
            ""
        };
        let drop = self.drop_call(*ty, &format!("&{name}"));
        format!("{pop}if ({name}_live) {drop};")
    }

    /// Bind every leaf of a tuple pattern to a variable holding that part of the temporary `path`.
    fn unpack_c(&mut self, pat: &UnpackPat, path: &str, ty: Ty) {
        match pat {
            UnpackPat::Bind(name, _, id, _) => {
                let n = format!("v{}_{}", id, name);
                self.vars.insert(*id, (n.clone(), false, ty));
                self.line(&format!("{} {} = {};", self.types.cty(ty), n, path));
                if self.types.owned(ty) {
                    self.track(*id);
                }
            }
            UnpackPat::Tuple(items, _) => {
                let Ty::Struct(sid) = ty else {
                    unreachable!("tuple pattern on a non-tuple")
                };
                let fields = self.types.structs[sid].fields.clone();
                for (it, (fname, fty)) in items.iter().zip(fields) {
                    self.unpack_c(it, &format!("{}.f_{}", path, fname), fty);
                }
            }
        }
    }

    fn drops(&self, all: bool) -> Vec<String> {
        let mut v = vec![];
        let list: Vec<&Vec<usize>> = if all {
            self.scopes.iter().rev().collect()
        } else {
            self.scopes.last().into_iter().collect()
        };
        for sc in list {
            for id in sc.iter().rev() {
                v.push(self.drop_local(*id));
            }
        }
        v
    }

    fn func(&mut self, f: &Func) {
        self.vars.clear();
        self.capture_flags.clear();
        self.scopes = vec![vec![]];
        let mut ps = vec![];
        for p in &f.params {
            let n = format!("v{}_{}", p.id, p.name);
            let r = self.is_ref(p.mode, p.ty);
            let c = self.types.cty(p.ty);
            ps.push(if r {
                format!("{} *{}", c, n)
            } else {
                format!("{} {}", c, n)
            });
            self.vars.insert(p.id, (n, r, p.ty));
        }
        let params = if ps.is_empty() {
            "void".to_string()
        } else {
            ps.join(", ")
        };
        let head = format!(
            "static {} nx_f_{}({}) {{",
            self.types.cty(f.ret),
            f.name,
            params
        );
        self.line(&head);
        self.ind += 1;
        for p in &f.params {
            if p.mode == Mode::Own && self.types.owned(p.ty) {
                self.track(p.id);
            }
        }
        self.stmts(&f.body);
        for d in self.drops(false) {
            self.line(&d);
        }
        self.ind -= 1;
        self.line("}");
        self.line("");
    }

    fn block(&mut self, b: &[Stmt]) {
        self.scopes.push(vec![]);
        self.ind += 1;
        self.stmts(b);
        for d in self.drops(false) {
            self.line(&d);
        }
        self.ind -= 1;
        self.scopes.pop();
    }

    fn stmts(&mut self, b: &[Stmt]) {
        for s in b {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.k {
            SK::Let(name, _, init, id) => {
                let e = self.ex(init);
                let n = format!("v{}_{}", id, name);
                self.vars.insert(*id, (n.clone(), false, init.ty));
                self.line(&format!("{} {} = {};", self.types.cty(init.ty), n, e));
                if self.types.owned(init.ty) {
                    self.track(*id);
                }
            }
            SK::Assign(target, val) => {
                let e = self.ex(val);
                let pc = self.place_c(target);
                if self.types.owned(val.ty) {
                    let c = self.types.cty(val.ty);
                    let drop = self.drop_call(val.ty, &format!("&{}", pc));
                    match &target.k {
                        EK::Var(_, id) if self.capture_flags.contains(id) => {
                            let live = format!("(*{}_live)", self.vars[id].0);
                            self.line(&format!(
                                "{{ {c} t_ = {e}; if ({live}) {drop}; {pc} = t_; {live} = true; }}"
                            ));
                        }
                        EK::Var(_, id) if !self.vars[id].1 => {
                            let n = self.vars[id].0.clone();
                            self.line(&format!("{{ {c} t_ = {e}; if ({n}_live) {drop}; {pc} = t_; {n}_live = true; }}"));
                        }
                        _ => {
                            let drop = self.drop_call(val.ty, "p_");
                            self.line(&format!(
                                "{{ {c} t_ = {e}; {c} *p_ = &{pc}; {drop}; *p_ = t_; }}"
                            ))
                        }
                    }
                } else if pc.contains("nx_at_") {
                    // value first, then the target place (SPEC)
                    self.line(&format!("{{ __auto_type t_ = {}; {} = t_; }}", e, pc));
                } else {
                    self.line(&format!("{} = {};", pc, e));
                }
            }
            SK::ExprS(e) => {
                let c = self.ex(e);
                self.line(&format!("(void)({});", c));
            }
            SK::If(c, t, el) => {
                let cc = self.ex(c);
                self.line(&format!("if ({}) {{", cc));
                self.block(t);
                if let Some(e) = el {
                    self.line("} else {");
                    self.block(e);
                }
                self.line("}");
            }
            SK::While(c, b, meta) => {
                let cc = self.ex(c);
                self.line(&format!("while ({}) {{", cc));
                self.ind += 1;
                self.loops.push(std::rc::Rc::new(CLoop {
                    id: meta.id,
                    depth: self.scopes.len(),
                    brk: false.into(),
                    cont: false.into(),
                }));
                self.line("{");
                self.block(b);
                self.line("}");
                let lp = self.loops.pop().unwrap();
                if lp.cont.get() {
                    self.line(&format!("nx_cont_{}: ;", meta.id));
                }
                // the step runs after the body (and after `continue`), outside the body's scope
                self.loops.push(std::rc::Rc::new(CLoop {
                    id: meta.id,
                    depth: self.scopes.len(),
                    brk: false.into(),
                    cont: false.into(),
                }));
                self.stmts(&meta.step);
                let lp2 = self.loops.pop().unwrap();
                self.ind -= 1;
                self.line("}");
                if lp.brk.get() || lp2.brk.get() {
                    self.line(&format!("nx_brk_{}: ;", meta.id));
                }
            }
            SK::Break(_, id) | SK::Continue(_, id) => {
                let is_break = matches!(s.k, SK::Break(..));
                let pos = self
                    .loops
                    .iter()
                    .rposition(|l| l.id == *id)
                    .expect("loop of break/continue");
                if is_break {
                    self.loops[pos].brk.set(true);
                } else {
                    self.loops[pos].cont.set(true);
                }
                let depth = self.loops[pos].depth;
                // free everything declared inside the loop body (innermost first), then jump
                let mut ds = vec![];
                for sc in self.scopes[depth..].iter().rev() {
                    for vid in sc.iter().rev() {
                        ds.push(self.drop_local(*vid));
                    }
                }
                self.line("{");
                self.ind += 1;
                for d in ds {
                    self.line(&d);
                }
                self.line(&format!(
                    "goto nx_{}_{};",
                    if is_break { "brk" } else { "cont" },
                    id
                ));
                self.ind -= 1;
                self.line("}");
            }
            SK::Block(b, _) | SK::Unsafe(b) => {
                self.line("{");
                self.block(b);
                self.line("}");
            }
            SK::For(..) => unreachable!("`for` is rewritten by the checker"),
            SK::Unpack(pat, init) => {
                let e = self.ex(init);
                self.tmp += 1;
                let t = format!("u{}_", self.tmp);
                self.line(&format!("{} {} = {};", self.types.cty(init.ty), t, e));
                self.unpack_c(pat, &t, init.ty);
            }
            SK::Bind(name, mode, place, id) => {
                let n = format!("v{}_{}", id, name);
                if *mode == Mode::Own {
                    let EK::Field(env, capture) = &place.k else {
                        unreachable!("worker capture")
                    };
                    self.line(&format!(
                        "int64_t *{n}_live = &{}.f_live_{capture};",
                        self.place_c(env)
                    ));
                    self.capture_flags.insert(*id);
                }
                let pc = self.place_c(place);
                self.vars.insert(*id, (n.clone(), true, place.ty));
                self.line(&format!("{} *{} = &{};", self.types.cty(place.ty), n, pc));
            }
            SK::Defer(_) => unreachable!("`defer` is expanded by the checker"),
            SK::Match(scrut, arms) => {
                let c = self.ex(scrut);
                self.tmp += 1;
                let t = format!("m{}_", self.tmp);
                self.line("{");
                self.ind += 1;
                self.line(&format!("{} {} = {};", self.types.cty(scrut.ty), t, c));
                for (ai, arm) in arms.iter().enumerate() {
                    let cond = match arm.variant {
                        Some(vi) => format!("{}.tag == {}", t, vi),
                        None => "1".to_string(),
                    };
                    self.line(&format!(
                        "{}if ({}) {{",
                        if ai == 0 { "" } else { "else " },
                        cond
                    ));
                    self.scopes.push(vec![]);
                    self.ind += 1;
                    if let (Some(vi), Ty::Enum(ei)) = (arm.variant, scrut.ty) {
                        let (vname, payload) = self.types.enums[ei].variants[vi].clone();
                        for (k, (bn, bid)) in arm.binds.iter().enumerate() {
                            let pty = payload[k];
                            let n = format!("v{}_{}", bid, bn);
                            self.vars.insert(*bid, (n.clone(), false, pty));
                            self.line(&format!(
                                "{} {} = {}.u.v_{}.a{};",
                                self.types.cty(pty),
                                n,
                                t,
                                vname,
                                k
                            ));
                        }
                    }
                    self.stmts(&arm.body);
                    for d in self.drops(false) {
                        self.line(&d);
                    }
                    self.ind -= 1;
                    self.scopes.pop();
                    self.line("}");
                }
                self.ind -= 1;
                self.line("}");
            }
            SK::Return(opt) => {
                let ex = opt.as_ref().map(|e| (self.ex(e), e.ty));
                let drops = self.drops(true);
                self.line("{");
                self.ind += 1;
                if let Some((c, t)) = &ex {
                    self.line(&format!("{} r_ = {};", self.types.cty(*t), c));
                }
                for d in drops {
                    self.line(&d);
                }
                self.line(if ex.is_some() {
                    "return r_;"
                } else {
                    "return;"
                });
                self.ind -= 1;
                self.line("}");
            }
        }
    }

    /// A by-value argument (see `call_c`).
    fn val_c(&self, e: &Expr) -> Arg {
        Arg {
            code: self.ex(e),
            val: true,
            lit: is_lit(e),
            impure: !is_pure(e),
        }
    }

    /// A Text or borrowed-String argument, as a `Str` view.
    fn text_c(&self, e: &Expr) -> Arg {
        if e.ty == Ty::String {
            Arg {
                code: format!("nx_str_view(&{})", self.place_c(e)),
                val: false,
                lit: false,
                impure: !is_pure(e),
            }
        } else {
            Arg {
                code: format!("nx_str_text({})", self.ex(e)),
                val: true,
                lit: is_lit(e),
                impure: !is_pure(e),
            }
        }
    }

    /// A borrowed place argument (`&place`).
    fn ref_c(&self, e: &Expr) -> Arg {
        Arg {
            code: format!("&{}", self.place_c(e)),
            val: false,
            lit: false,
            impure: !is_pure(e),
        }
    }

    /// Build a C call with a DEFINED evaluation order (SPEC: left to right). C leaves the order of
    /// function arguments (and of binary operands, initializer lists, ...) unspecified, so as soon as
    /// an operand has an effect (a call, arithmetic that can panic, a list access) and there is more
    /// than one non-literal operand, every operand is evaluated into a temporary, in source order.
    /// Exception, also in the SPEC: a borrowed place argument (`l.at(i)`, the receiver) is a pointer
    /// that any call touching `l` could invalidate, so by-value arguments are evaluated first, then
    /// the borrowed places (in order).
    fn call_c(&self, f: &str, args: Vec<Arg>) -> String {
        let impure = args.iter().any(|a| a.impure);
        let hazard = args.iter().any(|a| !a.val && a.code.contains("nx_at_"))
            && args.iter().any(|a| a.val && !a.lit);
        let nonlit = args.iter().filter(|a| !a.lit).count();
        if !(hazard || (impure && nonlit >= 2)) {
            let cs: Vec<String> = args.into_iter().map(|a| a.code).collect();
            return format!("{}({})", f, cs.join(", "));
        }
        let mut pre = String::new();
        let mut names = vec![String::new(); args.len()];
        for pass in [true, false] {
            for (k, a) in args.iter().enumerate() {
                if a.val == pass {
                    pre += &format!("__auto_type a{}_ = {}; ", k, a.code);
                    names[k] = format!("a{}_", k);
                }
            }
        }
        format!("({{ {}{}({}); }})", pre, f, names.join(", "))
    }

    fn cmp_c(&self, op: &str, args: Vec<Arg>) -> String {
        let impure = args.iter().any(|a| a.impure);
        let nonlit = args.iter().filter(|a| !a.lit).count();
        if impure && nonlit >= 2 {
            format!(
                "({{ __auto_type l_ = {}; __auto_type r_ = {}; (l_ {} r_); }})",
                args[0].code, args[1].code, op
            )
        } else {
            format!("({} {} {})", args[0].code, op, args[1].code)
        }
    }

    /// Braced initializer (struct literal / enum payload) with fields evaluated left to right.
    fn init_c(&self, head: &str, items: Vec<(String, &Expr)>, tail: &str) -> String {
        let impure = items.iter().any(|(_, e)| !is_pure(e));
        let nonlit = items.iter().filter(|(_, e)| !is_lit(e)).count();
        if impure && nonlit >= 2 {
            let mut pre = String::new();
            let mut inits = vec![];
            for (k, (n, e)) in items.iter().enumerate() {
                pre += &format!("__auto_type i{}_ = {}; ", k, self.ex(e));
                inits.push(format!("{}i{}_", n, k));
            }
            format!("({{ {}{}{}{}; }})", pre, head, inits.join(", "), tail)
        } else {
            let inits: Vec<String> = items
                .iter()
                .map(|(n, e)| format!("{}{}", n, self.ex(e)))
                .collect();
            format!("{}{}{}", head, inits.join(", "), tail)
        }
    }

    fn ex(&self, e: &Expr) -> String {
        match &e.k {
            EK::Ascribe(_) => unreachable!("type ascriptions are erased by the checker"),
            EK::AsElse(..) => unreachable!("`as .. else` is rewritten by the checker"),
            EK::Sequence(stmts, result) => {
                let mut nested = Gen {
                    out: String::new(),
                    ind: self.ind + 1,
                    types: self.types,
                    sigs: self.sigs,
                    externs: self.externs.clone(),
                    vars: self.vars.clone(),
                    capture_flags: self.capture_flags.clone(),
                    recover_panics: self.recover_panics,
                    scopes: self.scopes.clone(),
                    tmp: 0,
                    loops: self.loops.clone(),
                };
                nested.scopes.push(vec![]);
                nested.stmts(stmts);
                let value = nested.ex(result);
                let tail = if result.ty == Ty::Unit {
                    nested.line(&format!("(void)({value});"));
                    "(void)0"
                } else {
                    nested.line(&format!(
                        "{} nx_value = {};",
                        self.types.cty(result.ty),
                        value
                    ));
                    "nx_value"
                };
                for drop in nested.drops(false) {
                    nested.line(&drop);
                }
                format!("({{\n{}{tail}; }})", nested.out)
            }
            EK::GenStruct { .. } | EK::GenVariant { .. } => {
                unreachable!("generic literal left by the checker")
            }
            EK::Int(_) | EK::Float(..) => self.num_lit(e),
            EK::Char(n) => format!("INT64_C({})", n),
            EK::Conv(x, trunc) => self.conv_c(x, e.ty, *trunc),
            EK::IsVariant(b, vid) => format!("({}.tag == {})", self.ex(b), vid),
            EK::Payload(b, vid, k) => {
                let Ty::Enum(eid) = b.ty else { unreachable!() };
                format!(
                    "{}.u.v_{}.a{}",
                    self.ex(b),
                    self.types.enums[eid].variants[*vid].0,
                    k
                )
            }
            EK::Bool(b) => (if *b { "true" } else { "false" }).to_string(),
            EK::Text(s) => format!(
                "\"{}\"",
                s.replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n")
                    .replace('\t', "\\t")
                    .replace('\r', "\\r")
            ),
            EK::Var(..) | EK::Field(..) => self.place_c(e),
            EK::Method { m, .. } if m == "at" => self.place_c(e),
            EK::Method { m, args, .. }
                if matches!(m.as_str(), "get" | "locked_value") && args.is_empty() =>
            {
                self.place_c(e)
            }
            EK::Method { recv, m, .. } if m == "take" && matches!(recv.ty, Ty::Box(_)) => {
                let Ty::Box(i) = recv.ty else { unreachable!() };
                let t = self.types.cty(self.types.boxes[i]);
                format!(
                    "({{ {} b_ = {}; {} v_ = *b_; free(b_); v_; }})",
                    self.types.cty(recv.ty),
                    self.ex(recv),
                    t
                )
            }
            EK::Method { recv, m, .. } if recv.ty == Ty::Char => match m.as_str() {
                "utf8_len" => format!("nx_char_utf8_len({})", self.ex(recv)),
                _ => self.ex(recv), // to_int: same representation
            },
            EK::Move(_, id) => {
                if self.types.owned(e.ty) {
                    let n = &self.vars[id].0;
                    if self.capture_flags.contains(id) {
                        format!("(*{n}_live = false, *{n})")
                    } else {
                        format!("({n}_live = false, {n})")
                    }
                } else {
                    self.lv(*id)
                }
            }
            EK::Un(op, x) => {
                let c = self.ex(x);
                if *op == "!" {
                    format!("(!{})", c)
                } else if crate::numeric::is_float(e.ty) {
                    format!("(-{})", c)
                } else if matches!(e.ty, Ty::Int | Ty::Range(..)) {
                    format!("nx_neg({})", c)
                } else {
                    format!("nx_neg_{}({})", nsfx(e.ty), c)
                }
            }
            EK::Bin(op, a, b) => {
                let arg = |x: &Expr| Arg {
                    code: self.ex(x),
                    val: true,
                    lit: is_lit(x),
                    impure: !is_pure(x),
                };
                let args = vec![arg(a), arg(b)];
                match *op {
                    "&&" | "||" => format!("({} {} {})", args[0].code, op, args[1].code), // sequenced in C
                    "+" | "-" | "*" | "/" | "%" | "+%" | "-%" | "*%" => {
                        self.num_bin(op, a, b, args)
                    }
                    o => {
                        // comparison: operands are unsequenced in C too
                        self.cmp_c(o, args)
                    }
                }
            }
            EK::Method { recv, m, args } => {
                let mut a: Vec<Arg> = vec![self.ref_c(recv)];
                match recv.ty {
                    Ty::String => {
                        for (k, x) in args.iter().enumerate() {
                            a.push(
                                if matches!(m.as_str(), "push_str" | "eq" | "push_slice") && k == 0
                                {
                                    self.text_c(x)
                                } else {
                                    self.val_c(x)
                                },
                            );
                        }
                        self.call_c(&format!("nx_str_{}", m), a)
                    }
                    _ => {
                        let c = self.types.cty(recv.ty);
                        for x in args {
                            a.push(self.val_c(x));
                        }
                        if m == "get" {
                            format!("(*{})", self.call_c(&format!("nx_at_{}", c), a))
                        } else {
                            self.call_c(&format!("nx_{}_{}", m, c), a)
                        }
                    }
                }
            }
            EK::Static {
                ty: Ty::Box(_),
                f,
                args,
            } if f == "from_runtime_word" => {
                format!(
                    "(({})(intptr_t)({}))",
                    self.types.cty(e.ty),
                    self.ex(&args[0])
                )
            }
            EK::Static {
                ty: Ty::Box(i),
                f,
                args,
            } if f == "new" => {
                let t = self.types.cty(self.types.boxes[*i]);
                format!("({{ {t} v_ = {}; {t} *p_ = ({t} *)malloc(sizeof({t})); if (!p_) nx_panic(\"out of memory\"); *p_ = v_; p_; }})", self.ex(&args[0]))
            }
            EK::Static {
                ty: Ty::Char,
                f,
                args,
            } => {
                format!(
                    "{}({})",
                    if f == "is_valid" {
                        "nx_char_valid"
                    } else {
                        "nx_char_from_int"
                    },
                    self.ex(&args[0])
                )
            }
            EK::Static { ty, f, args } => match ty {
                Ty::String => {
                    let a = args.iter().map(|x| self.text_c(x)).collect();
                    self.call_c(&format!("nx_str_{}", f), a)
                }
                _ => format!("nx_new_{}()", self.types.cty(*ty)),
            },
            EK::ArrayLit(items) => {
                let c = self.types.cty(e.ty);
                let items: Vec<(String, &Expr)> = items
                    .iter()
                    .enumerate()
                    .map(|(k, v)| (format!(".e[{k}] = "), v))
                    .collect();
                self.init_c(&format!("({}){{ ", c), items, " }")
            }
            EK::ArrayFill(v, n) => {
                let c = self.types.cty(e.ty);
                let et = self.types.cty(v.ty);
                format!("({{ {et} v_ = {}; {c} a_; for (int64_t i_ = 0; i_ < {n}; i_++) a_.e[i_] = v_; a_; }})", self.ex(v))
            }
            EK::SliceOf(b, lo, hi) => {
                // a List-shaped header (by value: see `is_ref`)
                let lc = self.types.cty(e.ty);
                let int = |x: &Option<Box<Expr>>, dflt: String| match x {
                    Some(x) => self.val_c(x),
                    None => Arg {
                        code: dflt,
                        val: true,
                        lit: true,
                        impure: false,
                    },
                };
                match self.types.array_of(b.ty) {
                    Some((_, n)) => {
                        let base = Arg {
                            code: format!("({}).e", self.place_c(b)),
                            val: false,
                            lit: false,
                            impure: !is_pure(b),
                        };
                        let args = vec![
                            base,
                            int(&None, n.to_string()),
                            int(lo, "0".into()),
                            int(hi, n.to_string()),
                        ];
                        self.call_c(&format!("nx_slice_{lc}"), args)
                    }
                    None => {
                        let args = vec![
                            self.ref_c(b),
                            int(lo, "0".into()),
                            int(hi, "0".into()),
                            int(&None, (if hi.is_none() { "true" } else { "false" }).into()),
                        ];
                        self.call_c(&format!("nx_slicel_{lc}"), args)
                    }
                }
            }
            EK::StructLit { sid, fields } => {
                let c = self.types.cty(Ty::Struct(*sid));
                if fields.is_empty() {
                    return format!("({}){{0}}", c);
                }
                let items: Vec<(String, &Expr)> = fields
                    .iter()
                    .map(|(n, v)| (format!(".f_{} = ", n), v))
                    .collect();
                self.init_c(&format!("({}){{ ", c), items, " }")
            }
            EK::Variant {
                eid,
                vname,
                vid,
                args,
            } => {
                let c = self.types.cty(Ty::Enum(*eid));
                if args.is_empty() {
                    format!("({}){{ .tag = {} }}", c, vid)
                } else {
                    let items: Vec<(String, &Expr)> =
                        args.iter().map(|x| (String::new(), x)).collect();
                    self.init_c(
                        &format!("({}){{ .tag = {}, .u.v_{} = {{ ", c, vid, vname),
                        items,
                        " } }",
                    )
                }
            }
            EK::DynBox { table, value } => {
                let ty = self.types.cty(value.ty);
                let value = self.ex(value);
                format!("({{ {ty} v_ = {value}; struct {{ NxDyn header; {ty} value; }} *p_ = malloc(sizeof(*p_)); if (!p_) nx_panic(\"out of memory\"); p_->value = v_; p_->header = (NxDyn){{ &p_->value, nx_vtable_{table} }}; &p_->header; }})")
            }
            EK::DynCall { slot, sig, args } => {
                let mut pre = String::new();
                let mut params = vec!["void *".to_string()];
                let mut names = vec!["a0_->data".to_string()];
                for (i, (mode, ty)) in sig.params.iter().enumerate().skip(1) {
                    let cty = self.types.cty(*ty);
                    params.push(if self.is_ref(*mode, *ty) {
                        format!("{cty} *")
                    } else {
                        cty
                    });
                    names.push(format!("a{i}_"));
                }
                // Use the same two evaluation phases as an ordinary call. In particular,
                // delay accessing a receiver stored in a list until value arguments are ready.
                let mut borrows = Vec::new();
                for (i, (arg, (mode, ty))) in args.iter().zip(&sig.params).enumerate() {
                    if self.is_ref(*mode, *ty) {
                        borrows.push((i, arg));
                    } else {
                        pre += &format!("__auto_type a{i}_ = {}; ", self.ex(arg));
                    }
                }
                // Compute every index before taking any pointer into a list.
                let mut refs = String::new();
                let mut next_index = 0;
                for (i, arg) in borrows {
                    let place = self.place_with_indices(arg, &mut |index| {
                        let name = format!("dyn_index{next_index}_");
                        next_index += 1;
                        pre += &format!("int64_t {name} = {}; ", self.ex(index));
                        name
                    });
                    refs += &format!("__auto_type a{i}_ = &{place}; ");
                }
                pre += &refs;
                format!(
                    "({{ {pre}(({} (*)({}))a0_->table[{}])({}); }})",
                    self.types.cty(sig.ret),
                    params.join(", "),
                    slot + 1,
                    names.join(", ")
                )
            }
            EK::Closure { .. } => unreachable!("closure literal left by the checker"),
            EK::AddrOf(p, _) => format!("((int64_t)(intptr_t)&{})", self.place_c(p)),
            EK::Deref(_) => self.place_c(e),
            EK::MkClosure { func, env, caps } => match env {
                None => format!("(NxFn){{ (void *)nx_f_{}, 0 }}", func),
                Some(sid) => {
                    let c = self.types.cty(Ty::Struct(*sid));
                    let mut s = format!("({{ {c} *e_ = ({c} *)malloc(sizeof({c})); if (!e_) nx_panic(\"out of memory\"); e_->f_env_drop = (int64_t)(intptr_t)nx_envdrop_{c}; ");
                    for (k, x) in caps.iter().enumerate() {
                        s += &format!("e_->f_c{k} = {}; ", self.ex(x));
                        if let Some(flag) = self.types.capture_live_field(*sid, &format!("c{k}")) {
                            s += &format!("e_->f_{flag} = 1; ");
                        }
                    }
                    s += &format!("(NxFn){{ (void *)nx_f_{}, e_ }}; }})", func);
                    s
                }
            },
            EK::CallFn(callee, args) => {
                let Ty::Fn(fi) = callee.ty else {
                    unreachable!("call of a non-function")
                };
                let ft = self.types.fns[fi].clone();
                let mut pts = vec!["void *".to_string()];
                let mut a: Vec<Arg> = vec![Arg {
                    code: "f_->env".into(),
                    val: true,
                    lit: true,
                    impure: false,
                }];
                for (x, (mode, pty)) in args.iter().zip(ft.params.iter()) {
                    if self.is_ref(*mode, *pty) {
                        pts.push(format!("{} *", self.types.cty(*pty)));
                        a.push(self.ref_c(x));
                    } else {
                        pts.push(self.types.cty(*pty));
                        a.push(self.val_c(x));
                    }
                }
                let call = self.call_c(
                    &format!(
                        "(({} (*)({}))f_->code)",
                        self.types.cty(ft.ret),
                        pts.join(", ")
                    ),
                    a,
                );
                format!("({{ NxFn *f_ = &{}; {}; }})", self.place_c(callee), call)
            }
            EK::Call(name, args) => match name.as_str() {
                "assert" => format!("nx_assert({})", self.ex(&args[0])),
                "panic" => match args[0].ty {
                    Ty::String => {
                        let p = self.place_c(&args[0]);
                        format!("nx_panic_str(nx_str_view(&({p})))")
                    }
                    _ => format!("nx_panic({})", self.ex(&args[0])),
                },
                "__atomic_load" | "__atomic_store" | "__atomic_cmpxchg" | "__atomic_fetch_add"
                | "__futex_park" | "__futex_wake" => {
                    let operands = args
                        .iter()
                        .map(|x| Arg {
                            code: self.ex(x),
                            val: true,
                            lit: is_lit(x),
                            impure: !is_pure(x),
                        })
                        .collect();
                    self.call_c(&format!("nx_{}", &name[2..]), operands)
                }
                "__check_shared_content" => "((void)0)".into(),
                "__thread_pack" | "__box_address" => {
                    format!("((int64_t)(intptr_t)({}))", self.ex(&args[0]))
                }
                "__locked_unbox" => format!(
                    "({{ __auto_type lock_ = {}; free(lock_.f_mutex); lock_.f_value; }})",
                    self.ex(&args[0])
                ),
                "__parallel_address" => {
                    format!("((int64_t)(intptr_t)&({}))", self.place_c(&args[0]))
                }
                "__parallel_call" => self.call_c(
                    "nx_parallel_call",
                    args.iter().map(|x| self.val_c(x)).collect(),
                ),
                "__thread_start" | "__thread_start_recover" => format!(
                    "nx_thread_start({}, {})",
                    self.ex(&args[0]),
                    name == "__thread_start_recover"
                ),
                "__thread_wait" => format!("nx_thread_wait({})", self.ex(&args[0])),
                "__syscall" => format!(
                    "nx_syscall({})",
                    args.iter()
                        .map(|x| self.ex(x))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                "__global" => format!("((int64_t)(intptr_t)&nx_globals[{}])", self.ex(&args[0])),
                "__alloc" => format!("nx_alloc({})", self.ex(&args[0])),
                "__free" => format!("nx_free({})", self.ex(&args[0])),
                "__ptr_null" => format!("(({})0)", self.types.cty(e.ty)),
                "__ptr_alloc" => format!(
                    "(({})nx_alloc(({}) * ({})))",
                    self.types.cty(e.ty),
                    self.ex(&args[0]),
                    self.ex(&args[1])
                ),
                "__ptr_free" => format!("free((void *)({}))", self.ex(&args[0])),
                "__ptr_is_null" => format!("(({}) == 0)", self.ex(&args[0])),
                "__load8" | "__load64" | "__store8" | "__store64" => {
                    format!(
                        "nx_{}({})",
                        &name[2..],
                        args.iter()
                            .map(|x| self.ex(x))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
                "__sqrt" => format!("nx_sqrt({})", self.ex(&args[0])),
                "__bits" => format!(
                    "nx_bits{}({})",
                    if args[0].ty == Ty::Num(Nk::F32) {
                        32
                    } else {
                        64
                    },
                    self.ex(&args[0])
                ),
                "__from_bits" => format!(
                    "nx_from_bits{}({})",
                    if e.ty == Ty::Num(Nk::F32) { 32 } else { 64 },
                    self.ex(&args[0])
                ),
                "print" => {
                    let c = self.ex(&args[0]);
                    match args[0].ty {
                        Ty::Text => format!("printf(\"%s\\n\", {})", c),
                        Ty::String => format!("nx_print_str(nx_str_view(&{}))", c),
                        Ty::Bool => format!("puts(({}) ? \"true\" : \"false\")", c),
                        Ty::Char => format!("nx_print_char({})", c),
                        _ => format!("printf(\"%lld\\n\", (long long)({}))", c),
                    }
                }
                _ => {
                    let sig = &self.sigs[name];
                    let mut a: Vec<Arg> = args
                        .iter()
                        .zip(sig.params.iter())
                        .map(|(x, (mode, pty))| {
                            let by_value = !self.is_ref(*mode, *pty);
                            if by_value {
                                self.val_c(x)
                            } else {
                                self.ref_c(x)
                            }
                        })
                        .collect();
                    // extra arguments of a C variadic call are scalars passed by value
                    for x in args.iter().skip(sig.params.len()) {
                        a.push(self.val_c(x));
                    }
                    let target = if self.externs.contains(name) {
                        format!("nx_x_{}", name)
                    } else {
                        format!("nx_f_{}", name)
                    };
                    self.call_c(&target, a)
                }
            },
        }
    }
}

struct Arg {
    code: String,
    val: bool,    // by-value (evaluated into a temporary when hoisting) vs borrowed place
    lit: bool,    // literal: needs no ordering
    impure: bool, // may have an effect or panic
}

fn is_lit(e: &Expr) -> bool {
    matches!(
        e.k,
        EK::Int(_) | EK::Float(..) | EK::Char(_) | EK::Bool(_) | EK::Text(_)
    )
}

/// No side effects and cannot panic (so its position in the evaluation order is unobservable).
fn is_pure(e: &Expr) -> bool {
    match &e.k {
        EK::Int(_)
        | EK::Float(..)
        | EK::Char(_)
        | EK::Bool(_)
        | EK::Text(_)
        | EK::Var(..)
        | EK::Move(..) => true,
        EK::Field(b, _) => is_pure(b),
        EK::Un(op, x) => (*op == "!" || crate::numeric::is_float(e.ty)) && is_pure(x),
        EK::Conv(x, _) => {
            (crate::numeric::is_float(e.ty) || (!crate::numeric::is_float(x.ty) && false))
                && is_pure(x)
        }
        EK::Bin(op, a, b) => {
            (!matches!(*op, "+" | "-" | "*" | "/" | "%" | "+%" | "-%" | "*%")
                || crate::numeric::is_float(a.ty))
                && is_pure(a)
                && is_pure(b)
        }
        _ => false,
    }
}

/// An f64 as an exact C99 hexadecimal floating literal.
fn hex_float(v: f64) -> String {
    let b = v.to_bits();
    let neg = b >> 63 != 0;
    let e = ((b >> 52) & 0x7ff) as i64;
    let m = b & ((1u64 << 52) - 1);
    let sign = if neg { "-" } else { "" };
    if e == 0 && m == 0 {
        return format!("{}0.0", sign);
    }
    let (lead, ex) = if e == 0 { (0, -1022) } else { (1, e - 1023) };
    format!("{}0x{}.{:013x}p{}", sign, lead, m, ex)
}

fn nsfx(t: Ty) -> &'static str {
    crate::numeric::sfx(t)
}

impl Gen<'_> {
    /// Literal of a number type.
    fn num_lit(&self, e: &Expr) -> String {
        match (&e.k, e.ty) {
            (EK::Int(n), Ty::Int) => {
                if *n == i64::MIN {
                    "(-INT64_C(9223372036854775807) - 1)".to_string()
                } else {
                    format!("INT64_C({})", n)
                }
            }
            (EK::Int(n), Ty::Range(..)) => {
                if *n == i64::MIN {
                    "(-INT64_C(9223372036854775807) - 1)".to_string()
                } else {
                    format!("INT64_C({})", n)
                }
            }
            (EK::Int(n), Ty::Num(Nk::U64)) => format!("UINT64_C({})", *n as u64),
            (EK::Int(n), Ty::Num(k)) => format!("(({}){})", k.cty(), n),
            (EK::Float(v, _), Ty::Num(Nk::F32)) => format!("((float){})", hex_float(*v)),
            (EK::Float(v, _), _) => hex_float(*v),
            _ => unreachable!("not a number literal"),
        }
    }

    fn conv_c(&self, x: &Expr, to: Ty, trunc: bool) -> String {
        use crate::numeric::*;
        let from = x.ty;
        let c = self.ex(x);
        if from == to {
            return c;
        }
        let ct = self.types.cty(to);
        match (is_float(from), is_float(to)) {
            (false, false) => {
                if trunc {
                    format!("(({}){})", ct, c)
                } else if let Ty::Range(lo, hi) = to {
                    if int_range(from).0 >= lo as i128 && int_range(from).1 <= hi as i128 {
                        c
                    } else {
                        let k = |n: i64| {
                            if n == i64::MIN {
                                "(-INT64_C(9223372036854775807) - 1)".to_string()
                            } else {
                                format!("INT64_C({})", n)
                            }
                        };
                        format!("nx_to_range((__int128)({}), {}, {})", c, k(lo), k(hi))
                    }
                } else {
                    format!("nx_to_{}({})", nsfx(to), c)
                }
            }
            (false, true) => format!("(({}){})", ct, c),
            (true, true) => format!("(({}){})", ct, c),
            (true, false) => format!("nx_f2i_{}((double)({}))", nsfx(to), c),
        }
    }

    fn num_bin(&self, op: &str, a: &Expr, _b: &Expr, args: Vec<Arg>) -> String {
        use crate::numeric::*;
        let t = a.ty;
        if is_float(t) {
            // IEEE: never traps; operands are unsequenced in C so keep the order rule for impure operands
            return self.cmp_c(op, args);
        }
        let s = nsfx(t);
        let f = match op {
            "+" => "add",
            "-" => "sub",
            "*" => "mul",
            "/" => "div",
            "%" => "mod",
            "+%" => "wadd",
            "-%" => "wsub",
            _ => "wmul",
        };
        let name = if matches!(t, Ty::Int | Ty::Range(..)) {
            format!("nx_{}", f)
        } else {
            format!("nx_{}_{}", f, s)
        };
        self.call_c(&name, args)
    }
}
