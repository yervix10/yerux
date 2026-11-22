/* The bootstrap emitter instantiates these helpers for concrete element types. */
#define SC_LIST(C, E, DROP) \
typedef struct { E *d; int64_t len, cap; } C; \
static inline E *sc_at_##C(C *l, int64_t i) { \
  if (i < 0 || i >= l->len) nx_panic("index out of range"); \
  return &l->d[i]; \
} \
static inline void sc_push_##C(C *l, E x) { \
  if (l->len == l->cap) { \
    if (l->cap > INT64_MAX / 2 / (int64_t)sizeof(E)) nx_panic("list too large"); \
    int64_t cap = l->cap ? l->cap * 2 : 4; \
    E *d = realloc(l->d, (size_t)cap * sizeof(E)); \
    if (!d) nx_panic("out of memory"); \
    l->d = d; l->cap = cap; \
  } \
  l->d[l->len++] = x; \
} \
static inline E sc_pop_##C(C *l) { \
  if (l->len == 0) nx_panic("pop from an empty list"); \
  return l->d[--l->len]; \
} \
static inline void sc_insert_##C(C *l, int64_t i, E x) { \
  if (i < 0 || i > l->len) nx_panic("List.insert index out of range"); \
  sc_push_##C(l, x); \
  memmove(l->d + i + 1, l->d + i, (size_t)(l->len - i - 1) * sizeof(E)); \
  l->d[i] = x; \
} \
static inline E sc_remove_##C(C *l, int64_t i, bool swap) { \
  if (i < 0 || i >= l->len) \
    nx_panic(swap ? "List.swap_remove index out of range" : "List.remove index out of range"); \
  E result = l->d[i]; \
  --l->len; \
  if (swap) l->d[i] = l->d[l->len]; \
  else memmove(l->d + i, l->d + i + 1, (size_t)(l->len - i) * sizeof(E)); \
  l->d[l->len] = (E){0}; \
  return result; \
} \
static inline void sc_reverse_##C(C *l) { \
  for (int64_t lo = 0, hi = l->len - 1; lo < hi; ++lo, --hi) { \
    E x = l->d[lo]; l->d[lo] = l->d[hi]; l->d[hi] = x; \
  } \
} \
static inline C sc_copy_range_##C(const C *l, int64_t lo, int64_t hi) { \
  if (lo < 0 || lo > hi || hi > l->len) nx_panic("List.slice range out of bounds"); \
  C result = {0}; \
  for (int64_t i = lo; i < hi; ++i) sc_push_##C(&result, l->d[i]); \
  return result; \
} \
static inline void sc_extend_##C(C *l, C other) { \
  for (int64_t i = 0; i < other.len; ++i) sc_push_##C(l, other.d[i]); \
  free(other.d); \
} \
static inline void sc_truncate_##C(C *l, int64_t n) { \
  if (n < 0) nx_panic("List.truncate length is negative"); \
  while (l->len > n) { --l->len; DROP(&l->d[l->len]); } \
} \
static inline void sc_clear_##C(C *l) { \
  for (int64_t i = 0; i < l->len; ++i) DROP(&l->d[i]); \
  l->len = 0; \
} \
static inline void sc_drop_##C(C *l) { sc_clear_##C(l); free(l->d); *l = (C){0}; } \
static inline C sc_take_##C(C *l) { C x = *l; *l = (C){0}; return x; }

#define SC_ARRAY(C, E, N) \
typedef struct { E d[(N) > 0 ? (N) : 1]; } C; \
static inline E *sc_at_##C(C *a, int64_t i) { \
  if (i < 0 || i >= (N)) nx_panic("index out of range"); \
  return &a->d[i]; \
}

#define SC_ARRAY_OWNED(C, E, N, DROP) \
SC_ARRAY(C, E, N) \
static inline void sc_drop_##C(C *a) { for (int64_t i = 0; i < (N); ++i) DROP(&a->d[i]); *a = (C){0}; } \
static inline C sc_take_##C(C *a) { C x = *a; *a = (C){0}; return x; }

#define SC_SLICE(C, E) \
typedef struct { E *d; int64_t len; } C; \
static inline E *sc_at_##C(C *s, int64_t i) { \
  if (i < 0 || i >= s->len) nx_panic("index out of range"); \
  return &s->d[i]; \
} \
static inline C sc_slice_##C(E *data, int64_t len, int64_t lo, int64_t hi) { \
  if (lo < 0 || lo > hi || hi > len) nx_panic("index out of range"); \
  return (C){lo == 0 ? data : data + lo, hi - lo}; \
}

#define SC_COPY_ELEMENT(p) ((void)(p))

#define SC_STRING_LIST(C) \
static inline C sc_split_##C(String *s, Str sep) { \
  if (!sep.n) nx_panic("String.split separator is empty"); \
  C out = {0}; \
  int64_t start = 0; \
  for (int64_t i; (i = sc_find(nx_str_view(s), sep, start)) >= 0; start = i + sep.n) \
    sc_push_##C(&out, sc_str_slice(s, start, i)); \
  sc_push_##C(&out, sc_str_slice(s, start, s->len)); \
  return out; \
} \
static inline C sc_lines_##C(String *s) { \
  C out = {0}; \
  int64_t start = 0; \
  for (int64_t i = 0; i < s->len; ++i) { \
    if (s->d[i] != '\n') continue; \
    int64_t end = i - (i > start && s->d[i - 1] == '\r'); \
    sc_push_##C(&out, sc_str_slice(s, start, end)); \
    start = i + 1; \
  } \
  if (start < s->len) sc_push_##C(&out, sc_str_slice(s, start, s->len)); \
  return out; \
} \
static inline String sc_join_##C(C *values, Str sep) { \
  String out = nx_str_new(); \
  for (int64_t i = 0; i < values->len; ++i) { \
    if (i) nx_str_push_str(&out, sep); \
    nx_str_push_str(&out, nx_str_view(&values->d[i])); \
  } \
  return out; \
}
