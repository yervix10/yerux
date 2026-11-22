# Running the test suite

| Command | What | Time (12 CPUs, shared, 4 jobs) |
|---|---|---|
| `tests/run_all.sh --fast` | every example + every 7th test + `tests/fast.list` + the cheap scripts | ~50 s |
| `tests/run_all.sh PATTERN...` | programs/scripts whose name matches a regex (`'iter_\|^e103'`) | seconds |
| `tests/run_all.sh` | everything (pre-push) | ~206 s wall (765 s of work) |
| `tests/ci.sh` | no-warning build, `cargo test`, full `run_all.sh`; exit 0/1 | ~4 min |

Options: `-j N` / `JOBS=N` (default: a third of the CPUs, min 2), `--times` (slowest-first table), `--serial`,
`--bless [PATTERN]`. Concurrent runs queue on `/tmp/yerux-run_all.lock` (`RUN_ALL_NOLOCK=1` skips it).
The last lines always list `FAILED (n): names` and `repeat with: tests/run_all.sh '^(a|b)$'`, then `failures: N`.

## How it works
- `tests/run_one.sh` runs one program (C, native, ASan, or `check` for `e*` tests) in its own temp dir;
  `tests/run_aux.sh` runs one auxiliary script. `run_all.sh` feeds both to `xargs -P`, scripts first (the long
  ones), and prints the results in a fixed order afterwards.
- Timeout = 30 s x max(1, load/CPUs). A run that times out is repeated once with twice the time; wrong output,
  crashes and sanitizer reports are never retried.

## Where the time goes (full run, seconds)
| Item | s | | Item | s |
|---|---|---|---|---|
| selfhost_resolve | 34 | | selfhost_parser | 19 |
| selfhost_types | 32 | | fmt (script) | 15 |
| fuzz_parse (edge cases) | 31 | | sumtool (script) | 14 |
| io_targets | 30 | | http | 12 |
| selfhost_fmt | 24 | | process_leaks (program) | 19 |
| own_deep_box_drop / scc_drop | 17 | | process_basic | 11 |
About 560 programs take 1-6 s each (C build + native build + ASan build); the self-hosted front-end scripts
and `io_targets` are the longest single jobs, so they start first.

## Root cause of the intermittent `failures: 1`
Two different things looked like one flaky failure:
1. **Quadratic compile time.** `fuzz_parse` edge cases (30 000 statements in one function, a 20 000-term `&&`
   chain) took 30 s / 14 s to *check*, so on a loaded machine they exceeded the 60 s fuzz timeout. Fixed in the
   compiler: the module loader scanned all previous tokens for every identifier (`modules.rs`), diagnostics
   found their source line with `lines().nth(n)` (`syntax.rs::render`), and `cond_facts` re-walked deep `&&`
   chains (`check_range.rs`). `many_args_list` 30 s -> 0.6 s, `long_and` 14 s -> 3.5 s. The fuzzer also scales
   its timeout by load and retries a timeout once.
2. **A real, deterministic mismatch** introduced with `derive` in modules: the self-hosted parser rejected
   `derive` items while the Rust loader expanded them (`selfhost_resolve`: 1 mismatch). Both front ends now skip
   `derive ... for X` when only parsing.
