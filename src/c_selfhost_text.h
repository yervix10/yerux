/* Primitive string and process operations used while bootstrapping the prelude. */
#include <sys/stat.h>

static inline bool sc_boundary(String *s, int64_t i) {
  return i == 0 || i == s->len || ((unsigned char)s->d[i] & 0xc0) != 0x80;
}

static inline String sc_str_slice(String *s, int64_t start, int64_t end) {
  if (start >= 0 && start <= s->len && !sc_boundary(s, start))
    nx_panic("String.slice start is not a character boundary");
  if (end >= 0 && end <= s->len && !sc_boundary(s, end))
    nx_panic("String.slice end is not a character boundary");
  if (start < 0 || end < start || end > s->len) nx_panic("index out of range");
  if (start == end) return nx_str_new();
  return nx_str_from((Str){s->d + start, end - start});
}

static inline void sc_str_push_slice(String *dst, Str src, int64_t start, int64_t end) {
  String view = {(char *)src.p, src.n, src.n};
  String part = sc_str_slice(&view, start, end);
  nx_str_push_str(dst, nx_str_view(&part));
  nx_str_free(&part);
}

static inline bool sc_space(unsigned char b) { return b == 32 || (b >= 9 && b <= 13); }

static inline String sc_str_trim(String *s) {
  int64_t lo = 0, hi = s->len;
  while (lo < hi && sc_space(s->d[lo])) ++lo;
  while (lo < hi && sc_space(s->d[hi - 1])) --hi;
  return sc_str_slice(s, lo, hi);
}

static inline bool sc_str_is_boundary(String *s, int64_t i) {
  return i >= 0 && i <= s->len && sc_boundary(s, i);
}

static inline int64_t sc_str_char_count(String *s) {
  int64_t count = 0;
  for (int64_t i = 0; i < s->len; ++i) count += ((uint8_t)s->d[i] & 0xc0) != 0x80;
  return count;
}

static inline bool sc_str_is_empty(String *s) { return s->len == 0; }

static inline String sc_str_trim_start(String *s) {
  int64_t lo = 0;
  while (lo < s->len && sc_space(s->d[lo])) ++lo;
  return sc_str_slice(s, lo, s->len);
}

static inline String sc_str_trim_end(String *s) {
  int64_t hi = s->len;
  while (hi > 0 && sc_space(s->d[hi - 1])) --hi;
  return sc_str_slice(s, 0, hi);
}

static inline String sc_str_repeat(String *s, int64_t n) {
  if (n < 0) nx_panic("String.repeat count is negative");
  String out = nx_str_new();
  while (n-- > 0) nx_str_push_str(&out, nx_str_view(s));
  return out;
}

/* Use the same simple case mappings as prelude_string.yx. */
static inline int sc_case_cp(int cp, bool up) {
  if (up) {
      if (cp >= 97 && cp <= 122) {
          return cp - 32;
      }
      if (cp >= 224 && cp <= 254 && cp != 247) {
          return cp - 32;
      }
      if (cp == 255) {
          return 376;
      }
      if (cp == 305) {
          return 73; // dotless i -> I
      }
      if (cp == 383) {
          return 83; // long s -> S
      }
      if ((cp >= 257 && cp <= 303 && cp % 2 == 1) || (cp >= 307 && cp <= 311 && cp % 2 == 1) || (cp >= 331 && cp <= 375 && cp % 2 == 1)) {
          return cp - 1;
      }
      if ((cp >= 314 && cp <= 328 && cp % 2 == 0) || (cp >= 378 && cp <= 382 && cp % 2 == 0)) {
          return cp - 1;
      }
      if (cp >= 945 && cp <= 969 && cp != 962) {
          return cp - 32;
      }
      if (cp == 962) {
          return 931;
      }
      if (cp == 181) {
          return 924; // micro sign -> Greek capital mu
      }
      if (cp == 970 || cp == 971) {
          return cp - 32; // dialytika: ϊ ϋ -> Ϊ Ϋ
      }
      if (cp == 940) {
          return 902; // ά -> Ά
      }
      if (cp >= 941 && cp <= 943) {
          return cp - 37; // έ ή ί -> Έ Ή Ί
      }
      if (cp == 972) {
          return 908; // ό -> Ό
      }
      if (cp == 973 || cp == 974) {
          return cp - 63; // ύ ώ -> Ύ Ώ
      }
      if (cp >= 1072 && cp <= 1103) {
          return cp - 32;
      }
      if (cp >= 1104 && cp <= 1119) {
          return cp - 80;
      }
      return cp;
  }
  if (cp >= 65 && cp <= 90) {
      return cp + 32;
  }
  if (cp >= 192 && cp <= 222 && cp != 215) {
      return cp + 32;
  }
  if (cp == 304) {
      return 105; // I with dot above -> i
  }
  if ((cp >= 256 && cp <= 302 && cp % 2 == 0) || (cp >= 306 && cp <= 310 && cp % 2 == 0) || (cp >= 330 && cp <= 374 && cp % 2 == 0)) {
      return cp + 1;
  }
  if ((cp >= 313 && cp <= 327 && cp % 2 == 1) || (cp >= 377 && cp <= 381 && cp % 2 == 1)) {
      return cp + 1;
  }
  if (cp == 376) {
      return 255;
  }
  if (cp >= 913 && cp <= 939 && cp != 930) {
      return cp + 32;
  }
  if (cp == 902) {
      return 940; // Ά -> ά
  }
  if (cp >= 904 && cp <= 906) {
      return cp + 37; // Έ Ή Ί -> έ ή ί
  }
  if (cp == 908) {
      return 972; // Ό -> ό
  }
  if (cp == 910 || cp == 911) {
      return cp + 63; // Ύ Ώ -> ύ ώ
  }
  if (cp >= 1040 && cp <= 1071) {
      return cp + 32;
  }
  if (cp >= 1024 && cp <= 1039) {
      return cp + 80;
  }
  return cp;
}

static inline String sc_str_change_case(String *s, bool upper) {
  String out = nx_str_new();
  for (int64_t i = 0; i < s->len; ++i) {
    unsigned char lead = s->d[i];
    if (lead < 128) nx_str_push_byte(&out, sc_case_cp(lead, upper));
    else if (lead >= 194 && lead < 224 && i + 1 < s->len) {
      int cp = (lead - 192) * 64 + ((unsigned char)s->d[++i] - 128);
      nx_str_push_char(&out, sc_case_cp(cp, upper));
    } else nx_str_push_byte(&out, lead);
  }
  return out;
}

static inline String sc_str_to_upper(String *s) { return sc_str_change_case(s, true); }
static inline String sc_str_to_lower(String *s) { return sc_str_change_case(s, false); }

static inline int64_t sc_find(Str src, Str part, int64_t start) {
  if (part.n == 0) return start <= src.n ? start : -1;
  for (int64_t i = start; i <= src.n - part.n; ++i)
    if (!memcmp(src.p + i, part.p, (size_t)part.n)) return i;
  return -1;
}

static inline bool sc_str_starts_with(String *s, Str part) {
  return part.n <= s->len && (!part.n || !memcmp(s->d, part.p, (size_t)part.n));
}

static inline bool sc_str_ends_with(String *s, Str part) {
  return part.n <= s->len && (!part.n || !memcmp(s->d + s->len - part.n, part.p, (size_t)part.n));
}

static inline int64_t sc_str_find_from(String *s, Str part, int64_t start) {
  if (start < 0 && part.n) nx_panic("index out of range");
  return sc_find(nx_str_view(s), part, start);
}

static inline int64_t sc_str_find(String *s, Str part) { return sc_find(nx_str_view(s), part, 0); }
static inline bool sc_str_contains(String *s, Str part) { return sc_str_find(s, part) >= 0; }

static inline String sc_str_replace(String *s, Str from, Str to) {
  if (!from.n || !s->len) return nx_str_from(nx_str_view(s));
  String result = nx_str_new();
  int64_t start = 0;
  for (int64_t i; (i = sc_find(nx_str_view(s), from, start)) >= 0; start = i + from.n) {
    nx_str_push_str(&result, (Str){s->d + start, i - start});
    nx_str_push_str(&result, to);
  }
  nx_str_push_str(&result, (Str){s->d + start, s->len - start});
  return result;
}

static inline int sc_compare(Str a, Str b) {
  int64_t n = a.n < b.n ? a.n : b.n;
  int cmp = n ? memcmp(a.p, b.p, (size_t)n) : 0;
  return cmp ? cmp : (a.n > b.n) - (a.n < b.n);
}

static inline bool sc_str_parse_int(String *s, int64_t *out) {
  int64_t i = 0, value = 0;
  bool negative = false;
  if (s->len && (s->d[0] == '+' || s->d[0] == '-')) {
    negative = s->d[0] == '-';
    ++i;
  }
  if (i == s->len) return false;
  for (; i < s->len; ++i) {
    int digit = (unsigned char)s->d[i] - '0';
    if (digit < 0 || digit > 9 || value < (INT64_MIN + digit) / 10) return false;
    value = value * 10 - digit;
  }
  if (!negative && value == INT64_MIN) return false;
  *out = negative ? value : -value;
  return true;
}

static inline String sc_fmt_text(String dst, Str part) { nx_str_push_str(&dst, part); return dst; }
static inline String sc_fmt_int(String dst, int64_t v) { nx_str_push_int(&dst, v); return dst; }
static inline String sc_fmt_char(String dst, int64_t v) { nx_str_push_char(&dst, v); return dst; }
static inline String sc_fmt_bool(String dst, bool v) { return sc_fmt_text(dst, nx_str_text(v ? "true" : "false")); }
static inline String sc_fmt_uint(String dst, uint64_t v) {
  char buf[32];
  int n = snprintf(buf, sizeof buf, "%llu", (unsigned long long)v);
  return sc_fmt_text(dst, (Str){buf, n});
}

static int sc_argc;
static char **sc_argv;

static inline bool sc_utf8(Str text) {
  for (int64_t i = 0; i < text.n;) {
    unsigned char b = text.p[i++];
    if (b < 0x80) continue;
    int n = b < 0xc2 ? 0 : b < 0xe0 ? 1 : b < 0xf0 ? 2 : b <= 0xf4 ? 3 : 0;
    if (!n || n > text.n - i) return false;
    unsigned char next = text.p[i];
    if ((b == 0xe0 && next < 0xa0) || (b == 0xed && next >= 0xa0) ||
        (b == 0xf0 && next < 0x90) || (b == 0xf4 && next >= 0x90)) return false;
    while (n--) if (((unsigned char)text.p[i++] & 0xc0) != 0x80) return false;
  }
  return true;
}

static inline String sc_cstr(const char *arg) {
  String s = nx_str_from(nx_str_text(arg));
  if (!sc_utf8(nx_str_view(&s))) {
    for (int64_t i = 0; i < s.len; ++i) if ((unsigned char)s.d[i] >= 128) s.d[i] = '?';
  }
  return s;
}

static inline String sc_read_error(const char *op, String *path, int err) {
  String out = nx_str_from(nx_str_text(op));
  if (path->len) {
    nx_str_push_str(&out, nx_str_text(" '"));
    nx_str_push_str(&out, nx_str_view(path));
    nx_str_push_str(&out, nx_str_text("'"));
  }
  nx_str_push_str(&out, nx_str_text(": "));
  nx_str_push_str(&out, nx_str_text(strerror(err)));
  nx_str_push_str(&out, nx_str_text(" (os error "));
  nx_str_push_int(&out, err);
  nx_str_push_byte(&out, ')');
  return out;
}

static inline bool sc_read_file(String *path, String *data, String *error) {
  if (path->len && memchr(path->d, 0, (size_t)path->len)) {
    *error = nx_str_from(nx_str_text("path contains a NUL byte"));
    return false;
  }
  String name = nx_str_from(nx_str_view(path));
  nx_str_push_byte(&name, 0);
  FILE *file = fopen(name.d, "rb");
  int err = errno;
  nx_str_free(&name);
  if (!file) { *error = sc_read_error("open", path, err); return false; }
  char buf[8192];
  size_t n;
  while ((n = fread(buf, 1, sizeof buf, file)) != 0) nx_str_push_str(data, (Str){buf, n});
  err = ferror(file) ? errno : 0;
  fclose(file);
  if (err || !sc_utf8(nx_str_view(data))) {
    nx_str_free(data);
    *error = err ? sc_read_error("read", path, err) : nx_str_from(nx_str_text("stream did not contain valid UTF-8"));
    return false;
  }
  return true;
}

static inline bool sc_is_file(String *path) {
  if (path->len && memchr(path->d, 0, (size_t)path->len)) return false;
  String name = nx_str_from(nx_str_view(path));
  nx_str_push_byte(&name, 0);
  struct stat st;
  bool found = stat(name.d, &st) == 0 && S_ISREG(st.st_mode);
  nx_str_free(&name);
  return found;
}

static inline void sc_eprint(Str s) {
  fflush(stdout);
  if (s.n) fwrite(s.p, 1, (size_t)s.n, stderr);
}

static inline void sc_eprintln(Str s) {
  sc_eprint(s);
  putc('\n', stderr);
}

static inline int64_t sc_write_fd(int fd, Str text, String *error) {
  fflush(stdout);
  int64_t done = 0;
  while (done < text.n) {
    ssize_t count = write(fd, text.p + done, (size_t)(text.n - done));
    if (count < 0 && errno == EINTR) continue;
    if (count <= 0) {
      const char *op = fd == 1 ? "write stdout" : "write stderr";
      String empty = nx_str_new();
      *error = count < 0 ? sc_read_error(op, &empty, errno) :
        nx_str_from(nx_str_text("write made no progress"));
      return -1;
    }
    done += count;
  }
  return done;
}

static inline bool sc_write_file(Str path, Str data, String *error) {
  String name = nx_str_from(path);
  if (path.n && memchr(path.p, 0, (size_t)path.n)) {
    nx_str_free(&name);
    *error = nx_str_from(nx_str_text("path contains a NUL byte"));
    return false;
  }
  nx_str_push_byte(&name, 0);
  FILE *file = fopen(name.d, "wb");
  int failure = file ? 0 : errno;
  --name.len;
  if (file) {
    if (data.n && fwrite(data.p, 1, (size_t)data.n, file) != (size_t)data.n)
      failure = errno ? errno : EIO;
    if (fclose(file) != 0 && !failure) failure = errno;
  }
  if (failure) *error = sc_read_error("write", &name, failure);
  nx_str_free(&name);
  return failure == 0;
}

static inline bool sc_current_exe(String *output, String *error) {
  String path = nx_str_from(nx_str_text("/proc/self/exe"));
  nx_str_push_byte(&path, 0);
  --path.len;
  size_t capacity = 256;
  for (;;) {
    char *buffer = malloc(capacity);
    if (!buffer) nx_panic("out of memory");
    ssize_t length = readlink(path.d, buffer, capacity);
    int failure = errno;
    if (length >= 0 && (size_t)length < capacity)
      *output = nx_str_from((Str){buffer, length});
    free(buffer);
    if (length < 0) {
      *error = sc_read_error("readlink", &path, failure);
      nx_str_free(&path);
      return false;
    }
    if ((size_t)length < capacity) {
      nx_str_free(&path);
      return true;
    }
    capacity *= 2;
  }
}

/* Return EOF, a value, or an error using the Option/Result tag order. */
static inline int sc_read_stdin(bool line, String *data, String *error) {
  fflush(stdout);
  int character;
  while ((character = fgetc(stdin)) != EOF) {
    if (line && character == '\n') {
      if (data->len && data->d[data->len - 1] == '\r') --data->len;
      break;
    }
    nx_str_push_byte(data, character);
  }
  if (ferror(stdin) || !sc_utf8(nx_str_view(data))) {
    if (ferror(stdin)) {
      String empty = nx_str_new();
      *error = sc_read_error("read stdin", &empty, errno);
    } else {
      *error = nx_str_from(nx_str_text("stream did not contain valid UTF-8"));
    }
    nx_str_free(data);
    return -1;
  }
  return line && character == EOF && data->len == 0 ? 0 : 1;
}
