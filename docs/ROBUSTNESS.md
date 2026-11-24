# Robustness of the compiler and runtime

Results of the robustness round: fuzzers over the compiler, sanitizer runs over the runtime, and an audit of the
native allocator. Every number below was measured on this tree (fixed seeds, so the runs repeat).

## 1. Fuzzers

Run in two batches (each fuzzer bounded to about 7 minutes). "Findings" are compiler crashes, hangs, or
disagreements between the C backend, native, `--no-opt` and the model.

| Fuzzer | Cases | Compiler crashes / hangs | Mismatches |
|---|---|---|---|
| `fuzz_parse.py` (token and byte mutations, pathological sources) | 4063 | 0 | n/a |
| `fuzz/fuzz_compile.py` (valid and invalid programs, diagnostics) | 3000 | 0 | 0 |
| `fuzz/mutate_compile.py` (mutated real sources; 64% rejected by the parser, 555 accepted by the whole checker) | 8000 | 0 (0 timeouts) | n/a |
| `fuzz.py` (scalar programs, C vs native vs `--no-opt`) | 1200 | 0 | 0 |
| `fuzz.py --agg` / `--structs` / `--num` | 300 / 300 / 300 | 0 | 0 |
| `fuzz_coll.py` (collections against a Python model, three backends) | 250 | 0 | 0 |
| `fuzz_new.py` (closures, owned enums, Box, `dyn`, defer) | 120 | 0 | 0 |
| `fuzz_opt.py` (every optimizer pass switched off in turn, plus ASAN/UBSAN C build) | 100 | 0 | 0 |
| `fuzz_loops.py` / `fuzz_loops2.py` (loop optimizations) | 400 / 250 | 0 | 0 |
| `fuzz_order.py` (evaluation order) | 400 | 0 | 0 |
| `fuzz_unsound.py` (ownership checker accepts a program that uses freed memory?) | 300 + 300 `--soup` | 0 | 0 holes (57 accepted programs, all sound) |
| `fuzz_io.py` (stdin handling) | 150 | 0 | 0 |
| `fuzz_range.py` (range arithmetic, 4 build modes) | 20 programs x 8 calls per operator | 0 | 0 |

Total: about 19,000 generated or mutated programs, no compiler panic, signal or hang, and no wrong-code
disagreement between backends.

Findings in the harnesses themselves (not in the compiler), fixed in this round:

1. The native backend prints the panic location (`  at file:line:col`) in release builds and the C backend only with
   `YERUX_LOCS` / `--debug` (docs/PANIC_DIAGNOSTICS.md). `fuzz.py`, `fuzz_new.py`, `fuzz_opt.py`, `fuzz_loops.py`,
   `fuzz_loops2.py` and `fuzz_order.py` compared stderr raw, so every panicking program was reported as a mismatch
   (13 false reproducers in the first run, deleted). Their `sh()` helper now drops those lines.
2. The Python model of `fuzz_io.py` stripped a final `\r` of an unterminated last line; the documented and
   implemented behaviour keeps it (docs/STDLIB_IO.md). 6 false mismatches in 120 cases; the model is fixed
   (150 new cases, 0 bad).

## 2. Sanitizers

* **ASAN + UBSAN (C backend, leak detection on)** over all `tests/*.yx` and `examples/*.yx` through the ASAN column of
  `tests/run_all.sh`: 0 failures in the full suites of this round (the column is part of every run). Also run by hand
  on the real programs (`examples/real/webkv` self test with its server and client threads; `examples/real/minidb`
  `load 3000`, `compact`, and a log cut at 20,000 bytes): clean, no leaks.
* **TSAN** (`tests/tsan_concurrency.sh`: every `conc_*` / `concurrency*` fixture and the thread pool) passes. A wider
  run over every program that starts threads or uses `Shared`/`Atomic`/`Mutex`/`Sender` (23 programs that finish
  normally, the webkv server included, plus `examples/concurrency_channels.yx`): 23 clean, 0 reports. Programs that
  panic on purpose (exit 101) are skipped, as in the TSAN script.
* No leak or race was found, so no runtime fix came out of this step.

## 3. Audit of unsafe code and the native allocator

The Rust sources of the compiler contain no `unsafe` block (`grep` finds only the word inside diagnostics and the
language's own `unsafe { }` keyword handling). The unsafe surface is the Yerux runtime: the C runtime
(`src/c_runtime.h`, `src/c_selfhost_*.h`) and the native runtime generated in `src/x64.rs` (`rt_alloc`, `rt_free`,
`rt_mmap`: size classes of 2^k bytes served from per-class free lists, a bump pointer in 1 MiB arenas, one `mmap` per
block above 64 KiB, an 8-byte size header, a double-free tag in `--native-debug-alloc` builds).

Boundary cases tried on both backends (`Ptr<UInt8>.alloc(n)` and the collections):

| Case | Before | Now |
|---|---|---|
| `alloc(0)` | works (1-byte block / minimum class) | same, tested |
| `alloc(-1)` ... `alloc(-8)` | C: ok; native: ok (the +8 header wrapped to a tiny class) | both: `panic: out of memory`, exit 101 (`tests/alloc_negative_size.yx`) |
| `alloc(i64::MIN)` and other negative sizes | C: ok; native: out of memory | both: out of memory |
| `alloc(i64::MAX)`, `2^62`, `2^60`, `2^48` | out of memory on both | unchanged |
| lengths 2^k - 9 ... 2^k + 1 for k = 4..20, 65528..65536, 1 MiB - 8, 1 MiB + 1, 3 MB, many blocks alive at once, freed in a scrambled order and re-allocated from the free lists | not tested | no overlap or corruption on both backends; `--native-debug-alloc`: 1724 allocations, 1724 frees, `live: 0` (`tests/alloc_boundaries.yx`) |

Known difference, not changed: `String.reserve(9223372036854775807)` reports `out of memory` on the C backend and
`integer overflow in multiplication` natively (both exit 101).

## 4. Status

* Compiler: 0 crashes or hangs in about 19,000 fuzz cases; 0 ownership-checker soundness holes.
* Backends: 0 disagreements between C, native and native `--no-opt` once the harnesses ignore the native-only
  location lines.
* Runtime: 1 inconsistency fixed (negative allocation sizes); ASAN/UBSAN and TSAN clean on the real programs.
* Open: the `String.reserve` message difference above; fuzzers are bounded in time, so a longer run with new seeds is
  the next step (`/tmp`-style batch: `python3 tests/fuzz.py -n 5000 --seed N`).
