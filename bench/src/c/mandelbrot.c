#include "common.h"
int main(void) {
  i64 w = 800, h = 600, maxit = 200, inside = 0, total = 0;
  for (i64 py = 0; py < h; py++) {
    double ci = (double)py * (2.4 / 600.0) - 1.2;
    for (i64 px = 0; px < w; px++) {
      double cr = (double)px * (3.2 / 800.0) - 2.2, zr = 0.0, zi = 0.0;
      i64 it = 0;
      while (it < maxit && zr * zr + zi * zi <= 4.0) {
        double t = zr * zr - zi * zi + cr;
        zi = 2.0 * zr * zi + ci;
        zr = t;
        it++;
      }
      if (it == maxit) inside++;
      total += it;
    }
  }
  printf("%lld\n%lld\n", inside, total);
  return 0;
}
