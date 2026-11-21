#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

enum { WORKERS = 4, ITEMS = 5000, ROUNDS = 12000, BATCHES = 4 };
typedef struct { int first, last; int64_t sum; } Job;

static int64_t work(int item) {
  int64_t value = item + 1;
  for (int i = 0; i < ROUNDS; ++i)
    value = (value * INT64_C(1103515245) + 12345) % INT64_C(2147483647);
  return value;
}

static void *run(void *arg) {
  Job *job = arg;
  job->sum = 0;
  for (int i = job->first; i < job->last; ++i) job->sum += work(i);
  return NULL;
}

int main(int argc, char **argv) {
  int parallel = argc > 1;
  int64_t total = 0;
  for (int batch = 0; batch < BATCHES; ++batch) {
    if (!parallel) {
      for (int i = 0; i < ITEMS; ++i) total += work(i);
      continue;
    }
    pthread_t threads[WORKERS];
    Job jobs[WORKERS];
    for (int w = 0; w < WORKERS; ++w) {
      jobs[w] = (Job){ ITEMS * w / WORKERS, ITEMS * (w + 1) / WORKERS, 0 };
      if (pthread_create(&threads[w], NULL, run, &jobs[w])) return 2;
    }
    for (int w = 0; w < WORKERS; ++w) {
      pthread_join(threads[w], NULL);
      total += jobs[w].sum;
    }
  }
  printf("%lld\n", (long long)total);
}
