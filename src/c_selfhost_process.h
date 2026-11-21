/* Process capture for the self-hosted compiler's std.process.run binding. */
#include <spawn.h>
#include <fcntl.h>
#include <sys/wait.h>

extern char **environ;

static inline char *sc_process_arg(Str arg) {
  if (arg.n && memchr(arg.p, 0, (size_t)arg.n)) return NULL;
  char *result = malloc((size_t)arg.n + 1);
  if (!result) nx_panic("out of memory");
  if (arg.n) memcpy(result, arg.p, (size_t)arg.n);
  result[arg.n] = 0;
  return result;
}

static inline int sc_capture(FILE *file, String *output) {
  if (fseek(file, 0, SEEK_SET) != 0) return errno;
  char buf[8192];
  size_t size;
  while ((size = fread(buf, 1, sizeof buf, file)) > 0)
    nx_str_push_str(output, (Str){buf, size});
  if (ferror(file)) return errno ? errno : EIO;
  if (!sc_utf8(nx_str_view(output))) {
    for (int64_t i = 0; i < output->len; ++i)
      if ((unsigned char)output->d[i] >= 128) output->d[i] = '?';
  }
  return 0;
}

static inline bool sc_run_process(Str program, String *args, int64_t count,
                                  int64_t *code, int64_t *signal_number,
                                  String *out, String *err, String *error) {
  char **argv = calloc((size_t)count + 2, sizeof *argv);
  if (!argv) nx_panic("out of memory");
  argv[0] = sc_process_arg(program);
  int failure = argv[0] && program.n ? 0 : EINVAL;
  for (int64_t i = 0; i < count && !failure; ++i) {
    argv[i + 1] = sc_process_arg(nx_str_view(&args[i]));
    if (!argv[i + 1]) failure = EINVAL;
  }

  FILE *stdout_file = NULL, *stderr_file = NULL;
  if (!failure && !(stdout_file = tmpfile())) failure = errno;
  if (!failure && !(stderr_file = tmpfile())) failure = errno;
  posix_spawn_file_actions_t actions;
  bool initialized = false;
  if (!failure) {
    failure = posix_spawn_file_actions_init(&actions);
    initialized = failure == 0;
  }
  if (!failure) failure = posix_spawn_file_actions_addopen(&actions, 0, "/dev/null", O_RDONLY, 0);
  if (!failure) failure = posix_spawn_file_actions_adddup2(&actions, fileno(stdout_file), 1);
  if (!failure) failure = posix_spawn_file_actions_adddup2(&actions, fileno(stderr_file), 2);
  pid_t child;
  if (!failure) failure = posix_spawnp(&child, argv[0], &actions, NULL, argv, environ);
  if (initialized) posix_spawn_file_actions_destroy(&actions);
  if (!failure) {
    int status;
    pid_t waited;
    do { waited = waitpid(child, &status, 0); } while (waited < 0 && errno == EINTR);
    if (waited < 0) failure = errno;
    else {
      *code = WIFEXITED(status) ? WEXITSTATUS(status) : -1;
      *signal_number = WIFSIGNALED(status) ? WTERMSIG(status) : 0;
      failure = sc_capture(stdout_file, out);
      if (!failure) failure = sc_capture(stderr_file, err);
    }
  }
  if (stdout_file) fclose(stdout_file);
  if (stderr_file) fclose(stderr_file);
  for (int64_t i = 0; i <= count; ++i) free(argv[i]);
  free(argv);
  if (!failure) return true;

  nx_str_free(out);
  nx_str_free(err);
  String name = nx_str_from(program);
  *error = sc_read_error("run", &name, failure);
  nx_str_free(&name);
  return false;
}
