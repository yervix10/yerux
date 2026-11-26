/* Entry point of the self-hosted compiler stages. Their passes are recursive, so, like the Rust compiler, they
 * run on a thread with a large (lazily committed) stack: the nesting limit of the parser is reached before the
 * stack ends. Stages are compiled with -Dmain=sc_stage_main and linked with this file; user programs are not. */
#include <pthread.h>
#include <stddef.h>
#include <stdint.h>

#undef main
int sc_stage_main(int argc, char **argv);

typedef struct { int argc; char **argv; int status; } ScStage;

static void *sc_stage_thread(void *start) {
  ScStage *stage = start;
  stage->status = sc_stage_main(stage->argc, stage->argv);
  return NULL;
}

int main(int argc, char **argv) {
  ScStage stage = { argc, argv, 0 };
  /* the Rust compiler's 2 GiB, or the largest the system grants */
  for (size_t size = (size_t)(SIZE_MAX > UINT32_MAX ? (uint64_t)1 << 31 : (uint64_t)1 << 28); size >= (size_t)1 << 25;
       size /= 2) {
    pthread_attr_t attributes;
    pthread_t thread;
    if (pthread_attr_init(&attributes)) break;
    int failed = pthread_attr_setstacksize(&attributes, size) ||
                 pthread_create(&thread, &attributes, sc_stage_thread, &stage);
    pthread_attr_destroy(&attributes);
    if (!failed) {
      pthread_join(thread, NULL);
      return stage.status;
    }
  }
  return sc_stage_main(argc, argv);
}
