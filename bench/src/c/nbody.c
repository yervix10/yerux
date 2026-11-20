#include "common.h"
static double x[5], y[5], z[5], vx[5], vy[5], vz[5], m[5];
static double energy(void) {
  double e = 0.0;
  for (int i = 0; i < 5; i++) {
    e = e + 0.5 * m[i] * (vx[i] * vx[i] + vy[i] * vy[i] + vz[i] * vz[i]);
    for (int j = i + 1; j < 5; j++) {
      double dx = x[i] - x[j], dy = y[i] - y[j], dz = z[i] - z[j];
      e = e - m[i] * m[j] / fsqrt(dx * dx + dy * dy + dz * dz);
    }
  }
  return e;
}
int main(void) {
  for (int k = 0; k < 5; k++) {
    double f = (double)k;
    x[k] = f * 1.5 - 3.0; y[k] = f * f * 0.25 - 1.0; z[k] = 2.0 - f * 0.75;
    vx[k] = 0.01 * (f - 2.0); vy[k] = 0.02 * (1.0 - f); vz[k] = 0.005 * f; m[k] = 1.0 + f * 0.5;
  }
  printf("%lld\n", (i64)(energy() * 1000000000.0));
  const double dt = 0.001;
  for (i64 s = 0; s < 2000000; s++) {
    for (int i = 0; i < 5; i++)
      for (int j = i + 1; j < 5; j++) {
        double dx = x[i] - x[j], dy = y[i] - y[j], dz = z[i] - z[j];
        double d2 = dx * dx + dy * dy + dz * dz;
        double mag = dt / (d2 * fsqrt(d2));
        double mi = m[i] * mag, mj = m[j] * mag;
        vx[i] = vx[i] - dx * mj; vy[i] = vy[i] - dy * mj; vz[i] = vz[i] - dz * mj;
        vx[j] = vx[j] + dx * mi; vy[j] = vy[j] + dy * mi; vz[j] = vz[j] + dz * mi;
      }
    for (int i = 0; i < 5; i++) { x[i] = x[i] + dt * vx[i]; y[i] = y[i] + dt * vy[i]; z[i] = z[i] + dt * vz[i]; }
  }
  printf("%lld\n", (i64)(energy() * 1000000000.0));
  return 0;
}
