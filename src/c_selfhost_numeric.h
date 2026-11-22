#include <math.h>

static inline double sc_float_min(double a, double b) {
  if (isnan(a)) return a;
  if (isnan(b)) return b;
  if (a < b) return a;
  if (b < a) return b;
  return signbit(a) ? a : b;
}

static inline double sc_float_max(double a, double b) {
  if (isnan(a)) return a;
  if (isnan(b)) return b;
  if (a > b) return a;
  if (b > a) return b;
  return signbit(a) ? b : a;
}

/* The interval algorithm in prelude_num.yx, using base-2**32 limbs.
   40 limbs leave room for the binary64 extremes and decimal scaling. */
typedef struct { uint32_t word[40]; size_t len; } ScBig;

static inline ScBig sc_big(uint64_t n) {
  return (ScBig){.word = {(uint32_t)n, (uint32_t)(n >> 32)}, .len = n >> 32 ? 2 : 1};
}
static inline void sc_big_mul(ScBig *a, uint32_t n) {
  uint64_t carry = 0;
  for (size_t i = 0; i < a->len; ++i) {
    carry += (uint64_t)a->word[i] * n;
    a->word[i] = (uint32_t)carry;
    carry >>= 32;
  }
  if (carry) a->word[a->len++] = (uint32_t)carry;
}
static inline void sc_big_shift(ScBig *a, int n) {
  while (n >= 31) { sc_big_mul(a, UINT32_C(1) << 31); n -= 31; }
  sc_big_mul(a, UINT32_C(1) << n);
}
static inline void sc_big_pow10(ScBig *a, int n) {
  while (n >= 9) { sc_big_mul(a, 1000000000); n -= 9; }
  while (n-- > 0) sc_big_mul(a, 10);
}
static inline int sc_big_cmp(const ScBig *a, const ScBig *b) {
  if (a->len != b->len) return a->len < b->len ? -1 : 1;
  for (size_t i = a->len; i > 0;) {
    --i;
    if (a->word[i] != b->word[i]) return a->word[i] < b->word[i] ? -1 : 1;
  }
  return 0;
}
static inline void sc_big_add(ScBig *a, const ScBig *b) {
  uint64_t carry = 0;
  for (size_t i = 0; i < b->len || carry; ++i) {
    if (i == a->len) a->word[a->len++] = 0;
    carry += a->word[i];
    if (i < b->len) carry += b->word[i];
    a->word[i] = (uint32_t)carry;
    carry >>= 32;
  }
}
static inline void sc_big_sub(ScBig *a, const ScBig *b) {
  uint64_t borrow = 0;
  for (size_t i = 0; i < b->len || borrow; ++i) {
    uint64_t sub = borrow + (i < b->len ? b->word[i] : 0);
    borrow = a->word[i] < sub;
    a->word[i] = (uint32_t)(a->word[i] - sub);
  }
  while (a->len > 1 && !a->word[a->len - 1]) --a->len;
}
static inline int sc_float_digits(uint64_t mantissa, int exponent, bool asymmetric,
                                  char digits[18], int *length) {
  ScBig r = sc_big(mantissa), s = sc_big(1), upper = sc_big(1), lower = sc_big(1);
  bool even = (mantissa & 1) == 0;
  int scale = asymmetric ? 2 : 1;
  if (exponent >= 0) {
    sc_big_shift(&r, exponent + scale);
    sc_big_shift(&s, scale);
    sc_big_shift(&upper, exponent + asymmetric);
    sc_big_shift(&lower, exponent);
  } else {
    sc_big_shift(&r, scale);
    sc_big_shift(&s, scale - exponent);
    if (asymmetric) upper = sc_big(2);
  }
  int estimate = (exponent + 63 - __builtin_clzll(mantissa)) * 1233;
  int k = estimate / 4096;
  if (estimate < 0 && estimate % 4096) --k;
  if (k >= 0) sc_big_pow10(&s, k);
  else {
    sc_big_pow10(&r, -k);
    sc_big_pow10(&upper, -k);
    sc_big_pow10(&lower, -k);
  }
  for (;;) {
    ScBig sum = r;
    sc_big_add(&sum, &upper);
    int cmp = sc_big_cmp(&sum, &s);
    if (cmp < 0 || (cmp == 0 && !even)) break;
    sc_big_mul(&s, 10);
    ++k;
  }
  int n = 0;
  for (;;) {
    sc_big_mul(&r, 10);
    sc_big_mul(&upper, 10);
    sc_big_mul(&lower, 10);
    int digit = 0;
    while (sc_big_cmp(&r, &s) >= 0) { sc_big_sub(&r, &s); ++digit; }
    int cmp = sc_big_cmp(&r, &lower);
    bool low = cmp < 0 || (cmp == 0 && even);
    ScBig sum = r;
    sc_big_add(&sum, &upper);
    cmp = sc_big_cmp(&sum, &s);
    bool high = cmp > 0 || (cmp == 0 && even);
    if (low || high) {
      sc_big_mul(&r, 2);
      if (high && (!low || sc_big_cmp(&r, &s) >= 0)) ++digit;
      digits[n++] = (char)('0' + digit);
      *length = n;
      return k;
    }
    digits[n++] = (char)('0' + digit);
  }
}

static inline String sc_format_float(double x, bool single) {
  int fraction_bits = single ? 23 : 52;
  int exponent_max = single ? 255 : 2047;
  uint64_t bits = single ? nx_bits32((float)x) : nx_bits64(x);
  uint64_t stored = bits & ((UINT64_C(1) << fraction_bits) - 1);
  int exponent = (int)((bits >> fraction_bits) & exponent_max);
  bool negative = signbit(x);
  if (exponent == exponent_max) {
    if (stored) return nx_str_from(nx_str_text("nan"));
    return nx_str_from(nx_str_text(negative ? "-inf" : "inf"));
  }
  if (!exponent && !stored) return nx_str_from(nx_str_text(negative ? "-0.0" : "0.0"));
  uint64_t mantissa = exponent ? stored + (UINT64_C(1) << fraction_bits) : stored;
  int bias = single ? 127 : 1023;
  int shift = (exponent ? exponent : 1) - bias - fraction_bits;
  char digits[18];
  int length;
  int k = sc_float_digits(mantissa, shift, !stored && exponent > 1, digits, &length);
  char buf[48], *out = buf;
  if (negative) *out++ = '-';
  if (k >= 17 || k < -3) {
    *out++ = digits[0];
    if (length > 1) {
      *out++ = '.';
      memcpy(out, digits + 1, (size_t)length - 1);
      out += length - 1;
    }
    out += sprintf(out, "e%d", k - 1);
  } else if (k <= 0) {
    *out++ = '0';
    *out++ = '.';
    for (int i = 0; i < -k; ++i) *out++ = '0';
    memcpy(out, digits, (size_t)length);
    out += length;
  } else {
    for (int i = 0; i < k; ++i) *out++ = i < length ? digits[i] : '0';
    *out++ = '.';
    if (k >= length) *out++ = '0';
    else {
      memcpy(out, digits + k, (size_t)(length - k));
      out += length - k;
    }
  }
  return nx_str_from((Str){buf, out - buf});
}

static inline void sc_print_float(double x, bool single) {
  String text = sc_format_float(x, single);
  nx_print_str(nx_str_view(&text));
  nx_str_free(&text);
}

/* Match the polynomials and range reduction in prelude_num.yx. */
// 2^n for -1022 <= n <= 1023
static inline double sc_math_pow2(int64_t n) { return nx_from_bits64(((uint64_t)(n + 1023)) * UINT64_C(4503599627370496)); }
static inline double sc_math_scale(double x, int64_t n) {
  double v = x;
  int64_t k = n;
  while (k > 1023) {
    v = v * sc_math_pow2(1023);
    k = k - 1023;
  }
  while (k < -1022) {
    v = v * sc_math_pow2(-1022);
    k = k + 1022;
  }
  return v * sc_math_pow2(k);
}

static inline double sc_math_exp(double x) {
  if (x != x) { return x; }
  if (x > 709.782712893384) { return INFINITY; }
  if (x < -745.2) { return 0.0; }
  double kf = floor(x * 1.4426950408889634 + 0.5);
  int64_t k = ((int64_t)kf);
  double r = (x - kf * 0.6931471803691238) - kf * 1.9082149292705877e-10;
  double p = 1.6059043836821613e-10;
  p = p * r + 2.08767569878681e-09;
  p = p * r + 2.505210838544172e-08;
  p = p * r + 2.755731922398589e-07;
  p = p * r + 2.7557319223985893e-06;
  p = p * r + 2.48015873015873e-05;
  p = p * r + 0.0001984126984126984;
  p = p * r + 0.001388888888888889;
  p = p * r + 0.008333333333333333;
  p = p * r + 0.041666666666666664;
  p = p * r + 0.16666666666666666;
  p = p * r + 0.5;
  p = p * r + 1.0;
  p = p * r + 1.0;
  return sc_math_scale(p, k);
}

static inline double sc_math_ln(double x) {
  if (x != x) { return x; }
  if (x < 0.0) { return NAN; }
  if (x == 0.0) { return -INFINITY; }
  if (x == INFINITY) { return x; }
  double v = x;
  int64_t e = 0;
  if (nx_bits64(v) < UINT64_C(4503599627370496)) {
    v = v * 18014398509481984.0;
    e = -54;
  }
  uint64_t bits = nx_bits64(v);
  e = e + ((int64_t)(bits / UINT64_C(4503599627370496))) - 1023;
  double m = nx_from_bits64((bits % UINT64_C(4503599627370496)) + UINT64_C(4607182418800017408));
  if (m > 1.4142135623730951) {
    m = m * 0.5;
    e = e + 1;
  }
  double s = (m - 1.0) / (m + 1.0);
  double s2 = s * s;
  double p = 2.0 / 27.0;
  p = p * s2 + 2.0 / 25.0;
  p = p * s2 + 2.0 / 23.0;
  p = p * s2 + 2.0 / 21.0;
  p = p * s2 + 2.0 / 19.0;
  p = p * s2 + 2.0 / 17.0;
  p = p * s2 + 2.0 / 15.0;
  p = p * s2 + 2.0 / 13.0;
  p = p * s2 + 2.0 / 11.0;
  p = p * s2 + 2.0 / 9.0;
  p = p * s2 + 2.0 / 7.0;
  p = p * s2 + 2.0 / 5.0;
  p = p * s2 + 2.0 / 3.0;
  p = p * s2 + 2.0;
  double ef = ((double)e);
  return (ef * 0.6931471803691238 + s * p) + ef * 1.9082149292705877e-10;
}

// sin / cos kernels on |r| <= pi/4
static inline double sc_math_sin_k(double r) {
  double r2 = r * r;
  double p = 1.9572941063391263e-20;
  p = p * r2 + -8.22063524662433e-18;
  p = p * r2 + 2.8114572543455206e-15;
  p = p * r2 + -7.647163731819816e-13;
  p = p * r2 + 1.6059043836821613e-10;
  p = p * r2 + -2.505210838544172e-08;
  p = p * r2 + 2.7557319223985893e-06;
  p = p * r2 + -0.0001984126984126984;
  p = p * r2 + 0.008333333333333333;
  p = p * r2 + -0.16666666666666666;
  return r + r * (r2 * p);
}
static inline double sc_math_cos_k(double r) {
  double r2 = r * r;
  double p = 4.110317623312165e-19;
  p = p * r2 + -1.5619206968586225e-16;
  p = p * r2 + 4.779477332387385e-14;
  p = p * r2 + -1.1470745597729725e-11;
  p = p * r2 + 2.08767569878681e-09;
  p = p * r2 + -2.755731922398589e-07;
  p = p * r2 + 2.48015873015873e-05;
  p = p * r2 + -0.001388888888888889;
  p = p * r2 + 0.041666666666666664;
  p = p * r2 + -0.5;
  return 1.0 + r2 * p;
}
// sin(x) for phase 0, cos(x) = sin(x + pi/2) for phase 1; the argument is reduced with a 4-part pi/2
static inline double sc_math_trig(double x, int64_t phase) {
  if (x != x) { return x; }
  double a = fabs(x);
  if (a > 4503599627370496.0) { return NAN; }
  double kf = floor(x * 0.6366197723675814 + 0.5);
  int64_t k = ((int64_t)kf);
  double r = x - kf * 1.570796251296997;
  r = r - kf * 7.549789415861596e-08;
  r = r - kf * 5.390302529957765e-15;
  r = r - kf * 3.2820035428735005e-22;
  int64_t q = (k + phase) % 4;
  if (q < 0) { q = q + 4; }
  if (q == 0) { return sc_math_sin_k(r); }
  if (q == 1) { return sc_math_cos_k(r); }
  if (q == 2) { return -sc_math_sin_k(r); }
  return -sc_math_cos_k(r);
}
static inline double sc_math_sin(double x) { return sc_math_trig(x, 0); }
static inline double sc_math_cos(double x) { return sc_math_trig(x, 1); }
static inline double sc_math_tan(double x) { return sc_math_trig(x, 0) / sc_math_trig(x, 1); }

static inline double sc_math_atan_k(double a) {
  double a2 = a * a;
  double p = 1.0 / 29.0;
  p = p * a2 + -1.0 / 27.0;
  p = p * a2 + 1.0 / 25.0;
  p = p * a2 + -1.0 / 23.0;
  p = p * a2 + 1.0 / 21.0;
  p = p * a2 + -1.0 / 19.0;
  p = p * a2 + 1.0 / 17.0;
  p = p * a2 + -1.0 / 15.0;
  p = p * a2 + 1.0 / 13.0;
  p = p * a2 + -1.0 / 11.0;
  p = p * a2 + 1.0 / 9.0;
  p = p * a2 + -1.0 / 7.0;
  p = p * a2 + 1.0 / 5.0;
  p = p * a2 + -1.0 / 3.0;
  p = p * a2 + 1.0 / 1.0;
  return a * p;
}

static inline double sc_math_atan(double x) {
  if (x != x) { return x; }
  double a = fabs(x);
  bool inv = false;
  if (a > 1.0) {
    a = 1.0 / a;
    inv = true;
  }
  double shift = 0.0;
  if (a > 0.2679491924311227) {
    a = (a * 1.7320508075688772 - 1.0) / (1.7320508075688772 + a);
    shift = 0.5235987755982989;
  }
  double res = shift + sc_math_atan_k(a);
  if (inv) { res = 1.5707963267948966 - res; }
  return copysign(res, x);
}

static inline double sc_math_atan2(double y, double x) {
  if (x != x || y != y) { return NAN; }
  if (y == 0.0) {
    if (x > 0.0 || (x == 0.0 && !signbit(x))) { return y; }
    return copysign(3.141592653589793, y);
  }
  if (x == 0.0) { return copysign(1.5707963267948966, y); }
  if (isinf(x)) {
    if (isinf(y)) {
      if (x > 0.0) { return copysign(0.7853981633974483, y); }
      return copysign(2.356194490192345, y);
    }
    if (x > 0.0) { return copysign(0.0, y); }
    return copysign(3.141592653589793, y);
  }
  if (isinf(y)) { return copysign(1.5707963267948966, y); }
  double r = sc_math_atan(y / x);
  if (x > 0.0) { return r; }
  if (signbit(y)) { return r - 3.141592653589793; }
  return r + 3.141592653589793;
}

static inline double sc_math_asin(double x) {
  if (x != x || fabs(x) > 1.0) { return NAN; }
  return sc_math_atan2(x, sqrt((1.0 - x) * (1.0 + x)));
}
static inline double sc_math_acos(double x) {
  if (x != x || fabs(x) > 1.0) { return NAN; }
  return sc_math_atan2(sqrt((1.0 - x) * (1.0 + x)), x);
}

// x ^ n for an integer n >= 0 by squaring
static inline double sc_math_powi(double x, int64_t n) {
  double result = 1.0;
  double base = x;
  int64_t k = n;
  while (k > 0) {
    if (k % 2 == 1) { result = result * base; }
    base = base * base;
    k = k / 2;
  }
  return result;
}

static inline double sc_math_pow(double x, double y) {
  if (y == 0.0) { return 1.0; }
  if (x == 1.0) { return 1.0; }
  if (x != x || y != y) { return NAN; }
  double ay = fabs(y);
  bool yint = ay >= 9007199254740992.0 || trunc(y) == y;
  bool yodd = false;
  if (yint && ay < 9007199254740992.0) { yodd = ((int64_t)y) % 2 != 0; }
  if (isinf(y)) {
    double ax = fabs(x);
    if (ax == 1.0) { return 1.0; }
    if ((ax > 1.0) == (y > 0.0)) { return INFINITY; }
    return 0.0;
  }
  if (x == 0.0) {
    if (y > 0.0) {
      if (yodd) { return x; }
      return 0.0;
    }
    if (yodd) { return copysign(INFINITY, x); }
    return INFINITY;
  }
  if (isinf(x)) {
    if (x > 0.0) {
      if (y > 0.0) { return x; }
      return 0.0;
    }
    if (y > 0.0) {
      if (yodd) { return x; }
      return INFINITY;
    }
    if (yodd) { return -0.0; }
    return 0.0;
  }
  if (x < 0.0) {
    if (!yint) { return NAN; }
    double r = sc_math_pow(-x, y);
    if (yodd) { return -r; }
    return r;
  }
  if (yint && ay <= 1024.0) {
    double p = sc_math_powi(x, ((int64_t)ay));
    if (y < 0.0) { return 1.0 / p; }
    return p;
  }
  return sc_math_exp(y * sc_math_ln(x));
}
