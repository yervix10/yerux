#include <inttypes.h>
#include <math.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>

enum { N = 1500 };

static void multiply(const double src[N], double dst[N], bool transpose) {
    for (int i = 0; i < N; ++i) {
        double sum = 0.0;
        for (int j = 0; j < N; ++j) {
            int k = i + j;
            int denominator = k * (k + 1) / 2 + (transpose ? j : i) + 1;
            sum += src[j] / (double)denominator;
        }
        dst[i] = sum;
    }
}

int main(void) {
    double u[N], v[N], tmp[N], uv = 0.0, vv = 0.0;
    for (int i = 0; i < N; ++i) u[i] = 1.0;
    for (int round = 0; round < 10; ++round) {
        multiply(u, tmp, false);
        multiply(tmp, v, true);
        multiply(v, tmp, false);
        multiply(tmp, u, true);
    }
    for (int i = 0; i < N; ++i) {
        uv += u[i] * v[i];
        vv += v[i] * v[i];
    }
    printf("%" PRId64 "\n", (int64_t)(sqrt(uv / vv) * 1000000000.0));
}
