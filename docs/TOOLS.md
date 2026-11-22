# Command-line tools

`yerux` is also the project tool. Commands that take no file work from a project root (the directory with
`yerux.toml`); see [PACKAGES.md](PACKAGES.md). Each section below is covered by a script in `tests/tool_*.sh`.

## `yerux fmt`

```
yerux fmt [--check|--stdout] [file.yx|dir ...]
```

Formats files in place with the canonical style (2-space indent). With no argument it formats the current directory
recursively, skipping `target/`, `node_modules/` and hidden directories. `--check` changes nothing: it prints the path
of every file that would change and exits 1 (use it in CI); `--stdout` prints the formatted source. A file with a
syntax error is reported as `path: line N: message` and makes the exit code 1; a missing path or unknown option exits 2.
Formatting is idempotent and never changes the generated program (`tests/fmt.sh` checks this on every example).

## `yerux bench`

```
yerux bench [file.yx|dir ...] [--native] [--no-opt] [--filter text] [--runs N]
```

A benchmark is a top-level block, like a `test`:

```
bench "fib 20" {
  assert(fib(20) == 6765)
}
```

`bench` blocks are dropped from normal builds and are not run by `yerux test`. `yerux bench` compiles each one as its
own program (the block body is `main`), runs it once as a warm-up (a non-zero exit, e.g. a failing `assert`, is a failure
and the command exits 1), then `--runs N` timed runs (default 5), and prints min, median and mean of the **wall time of
the whole process** (so it includes process start-up, typically under a millisecond). `--filter` selects by name
substring; `--native` uses the native backend. Without arguments it scans `src/` and `tests/` in a project, or the
current directory otherwise.

```
tests/tools/bench_ok.yx
  bench fib 20  min 756.08 us  median 807.13 us  mean 904.02 us  (5 runs)
```
