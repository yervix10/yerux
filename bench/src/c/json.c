#include <stdio.h>
#include <stdlib.h>
int main(void) {
  const int n = 100000;
  char *json = malloc((size_t)n * 8 + 2);
  char *p = json;
  *p++ = '[';
  for (int i = 0; i < n; ++i) p += sprintf(p, "%s%d", i ? "," : "", i);
  *p++ = ']'; *p = 0;
  long long sum = 0, number = 0;
  for (p = json; *p; ++p) {
    if (*p >= '0' && *p <= '9') number = number * 10 + *p - '0';
    else { sum += number; number = 0; }
  }
  printf("%lld\n", sum + number);
  free(json);
}
