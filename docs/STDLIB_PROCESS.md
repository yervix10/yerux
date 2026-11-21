# `std.process`: running other programs

Linux x86-64 native backend and the C backend (oracle). Raw syscalls only (`pipe2`, `fork`, `execve`,
`dup2`, `wait4`, `kill`, `poll`, `rt_sigaction`, `fcntl`), no libc on the native path. Every failure is an
`io.IoError` value. `--target windows` / `aarch64` refuse the module at compile time like `std.io`
(`tests/io_targets.sh`). It relies on user `Drop` (docs/DROP.md).

```
import std.process

var args = List<String>.new()
args.push(String.from("hello"))
match process.run("/bin/echo", args) {
  Ok(o) => { print(o.stdout) print(o.status.show()) }       // "hello\n"   "exit code 0"
  Err(e) => { print("{e}") }
}
```

Note: **`spawn` is a language keyword** (threads), so the function that starts a child is `process.start`.

## API

| Item | Notes |
|---|---|
| `command(program) -> Command` | `program` with a `/` is used as is; a bare name is searched in `PATH` of the child's environment (`/usr/local/bin:/usr/bin:/bin` when it has none). |
| `Command` methods (`var c`) | `arg(a)`, `add_args(list)`, `set_env(name, value)`, `unset_env(name)`, `clear_env()`, `set_dir(path)`, `set_stdin(s)`, `set_stdout(s)`, `set_stderr(s)` with `s: process.Stdio` = `Inherit` (default) / `Null` (`/dev/null`) / `Piped`. |
| `start(c) -> Result<Child, IoError>` | fork + exec. `NotFound`, `PermissionDenied` (also a directory or a non-executable file), `InvalidInput` (NUL in program/argument/dir, empty program, bad `set_env` name), `NotFound` for a missing `set_dir` directory (message `change directory '..'`), or the errno of `pipe2`/`fork`. The exec result comes back through a close-on-exec pipe, so a failed exec is reported by `start` itself, not as an exit code 127. |
| `run(program, args) -> Result<Output, IoError>` | stdin empty, stdout and stderr captured, waits. |
| `run_command(c) -> Result<Output, IoError>` | same for a prepared Command: unset (Inherit) stdin becomes `Null`, unset stdout/stderr are captured; `Null`/`Piped` you set win. |
| `Output { status: ExitStatus, stdout: String, stderr: String }` | invalid UTF-8 is converted lossily (non-ASCII byte -> `?`); use `read_stdout_bytes` for exact bytes. |
| `ExitStatus { code, signal }` | `code` 0..255 or -1 when killed; `signal` 0 or the signal number; `success()`; `Show`: `exit code 3` / `signal 9`. |
| `id() -> Int` | this process's pid. |

`Child` (owned; use `var`): `id()`, `write_stdin(s)`, `write_stdin_bytes(b: view [UInt8])`, `close_stdin()`,
`read_stdout()`, `read_stderr()` (to end of stream, `InvalidData` when not UTF-8), `read_stdout_bytes()`,
`wait()` (closes stdin first; repeating it returns the same status), `try_wait() -> Result<Option<ExitStatus>>`,
`signal(n)` (1..64), `kill()` (SIGKILL), `communicate(input) -> Result<Output>`.

`communicate` is the safe way to talk to a child that reads and writes a lot: it feeds `input` to stdin and
drains stdout and stderr in one `poll(2)` loop with non-blocking descriptors, so none of the three pipes can
fill up and stall the others (tested with 3 MB through `cat`, and 1 MB + 1.5 MB on stdout/stderr while sending
a big stdin). `write_stdin` followed by `read_stdout` by hand can deadlock when the child's output exceeds the
pipe buffer (64 KiB) while you are still writing.

## Environment

The child's environment is this process's current one, i.e. including every `env.set` / `env.remove`
(`io.env_vars()`), then `clear_env`, `set_env` and `unset_env` of the command in that order of
precedence (`clear_env` first). Entries are rebuilt as a fresh `envp` block for `execve`.

## Ownership, Drop, zombies

- `Child` has a destructor: when it is dropped it closes whatever pipe ends are still open (stdin first, so a
  child blocked reading stdin sees EOF) and, if it was never waited for, **waits for it** (blocking). No
  zombies and no leaked descriptors, also on early returns and error paths (a failed `start` reaps the
  half-started child). A child that keeps running and ignores its closed stdin makes the drop block: `kill()` it
  first. A `Child` is move-only; using it after `move` is an error.
- Internal `process.Fd` is the same idea for one descriptor (public only because `Child` has it as a field).
- Test: `tests/process_leaks.yx` (651 spawns of every kind; `/proc/self/fd` count and `wait4(-1, WNOHANG)`
  == ECHILD are checked after each group).

## Signals and pipes

- When a child gets a piped stdin the **parent ignores SIGPIPE** (process-wide, from then on) so a write to a
  child that stopped reading is `BrokenPipe`, not a fatal signal. The child gets the default SIGPIPE
  disposition back before `exec`.
- Pipes are `O_CLOEXEC`; descriptors are moved above 2 before `dup2`, so closed standard streams in the parent
  do not break the redirection.
- Exit/termination decoding follows `wait4`: low 7 bits = signal, else `(status >> 8) & 255`.

## Implementation notes

`start` builds everything the child needs before `fork` (argv, envp and PATH candidates as raw NUL-terminated
blocks, the three source descriptors, a scratch block), so the child only runs raw syscalls: `dup2` x3,
`chdir`, `rt_sigaction`, then `execve` for each candidate path (first non-ENOENT/ENOTDIR error wins), and on
failure writes the errno down the error pipe and `exit_group(127)`. The C backend's `syscall()` wrapper
flushes C's `stdout` before every call, so the child never inherits buffered output.

## Tests

`tests/process_basic.yx` (echo, args, PATH, exit codes, signals, kill/try_wait, cat via `communicate` and via
explicit pipes, stdio modes, cwd, bad-input errors), `env_process.yx`, `process_pipes.yx` (big data, broken
pipes), `process_leaks.yx`; checker errors `e1300`..`e1304`. All run on C, native and ASan; the first three
and `process_leaks` also under `tests/leakcheck.sh` (`env_process` uses `env.set`, which keeps its table until
exit by design, so it is not in the leak list).

## Limitations / not done

- `fork` is only safe while the program is single-threaded (use it before starting workers).
- No `Stdio` from a file or from another child's pipe (no redirect chaining), no process groups / sessions /
  `setuid`, no `wait` with a timeout, no `posix_spawn`/`vfork` (the whole address space is copied lazily by
  `fork`), no shell-quoting helpers.
- stdout/stderr text is lossy in `Output`; stdin is fed as `String` or `[UInt8]` (bytes are copied).
- Windows and aarch64: refused at compile time (they would need `CreateProcessW` / the aarch64 syscall numbers).
