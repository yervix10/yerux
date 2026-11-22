# Command-line tools

`yerux` is also the project tool. Commands that take no file work from a project root (the directory with
`yerux.toml`); see [PACKAGES.md](PACKAGES.md). Each section below is covered by a script in `tests/tool_*.sh`.

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
