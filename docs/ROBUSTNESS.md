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

## 4. Stdlib coverage findings

### Closed

| Finding | Reproduction before the fix | Current behavior and coverage |
|---|---|---|
| Percent decoding could return a String containing invalid UTF-8 | `server.url_decode("%FF", false)` returned a String containing byte 255. Iterating its characters panicked with exit 101. The HTTP parser exposed the decoded bytes in `Request.path` and through `query_param`. | The decoder returns `None` for invalid UTF-8, malformed escapes or NUL. The parser returns HTTP 400 with a message naming the path or query. The cases in `tests/std_http_util.yx` include truncated sequences, overlong encodings, a surrogate, and valid multi-byte text. `tests/http_server_loopback.yx` covers the wire responses. |
| Timed-out `net.accept` returned raw errno 11 (`EAGAIN`) | Set `net.set_accept_timeout(l, 30)` on a listener with no pending connections and inspect the error from `net.accept(l)`. | `accept` uses the same error conversion as socket reads and writes. `tests/std_net_accept.yx` requires code 110 (`ETIMEDOUT`). |
| Hex and base64 text decoders could return invalid UTF-8 | `hex.decode("ff")` and `base64.decode("/w==")` each returned a String containing byte 255. | Both text decoders return an error. Their `decode_bytes` variants preserve arbitrary bytes. `tests/std_encoding.yx` covers both paths and `tests/sumtool.sh` checks binary base64 decoding end to end. |
| JSON could pass through malformed UTF-8 from raw input | Append byte 255 between two quotes with `String.push_byte`, then call `json.parse`. It returned a JSON string containing that byte. | JSON rejects malformed UTF-8 in string values and object keys at the opening quote. Escaped surrogate pairs still decode as before. `tests/std_json.yx` covers both errors. |

### Decoder audit

There is no separate `std.url` module. CSV preserves valid UTF-8 because its delimiters are ASCII.
The second round fixed TOML's handling of malformed headers and non-ASCII literals.

Some interfaces carry arbitrary bytes in a String. For example, `compress.decompress` accepts an RLE pair assembled
with `push_byte(1)` and `push_byte(255)` and returns byte 255, as permitted by its byte-oriented format.
The TCP read API and HTTP body fields can likewise carry binary data, and HTTP header values may contain opaque
bytes. Moving these byte-oriented interfaces to byte buffers requires a separate API migration.

## 5. Status

* Compiler: 0 crashes or hangs in about 19,000 fuzz cases; 0 ownership-checker soundness holes.
* Backends: 0 disagreements between C, native and native `--no-opt` once the harnesses ignore the native-only
  location lines.
* Runtime: 1 inconsistency fixed (negative allocation sizes); ASAN/UBSAN and TSAN clean on the real programs.
* Stdlib: the four decoder and timeout findings of the first round are closed. The byte-oriented interfaces
  listed above still use String.
* Open: the `String.reserve` message difference above. Fuzzers are bounded in time, so a longer run with new
  seeds is the next step (`python3 tests/fuzz.py -n 5000 --seed N`).

## 6. Second robustness round

| Run | Seed | Cases and result |
|---|---|---|
| `python3 tests/fuzz_text.py --cases 10000 --seed 703513 --asan --timeout 90` | 703513 | 10,000 inputs each for JSON, TOML, HTTP, hex and base64. 50,000 cases each in C, x86-64 native and ASan/UBSan C. Zero model failures or output differences. |
| `tests/http_server_fuzz.yx` built and run with arguments `20000 411091` on both backends | 411091 | Each backend checked 20,000 requests and 117,200 prefixes and answered 884 loopback requests. No invariant violations, unstable parser verdicts, HTTP/parser status differences or backend differences. |
| `python3 tests/fuzz_parse.py -n 10000 --seed 381924 -j 3 --fmt-properties` | 381924 | 10,063 sources, zero failures. Formatter properties on checked inputs include well-formed UTF-8, idempotence and preservation of C output. |
| `python3 tests/fuzz/mutate_compile.py --cases 12000 --seed 909211 -j 2 --timeout 15` | 909211 | 12,000 corpus mutations, 805 accepted by the checker, zero timeouts or diagnostic/crash failures. |
| `python3 tests/fuzz.py -n 5000 --seed 761829 -j 3` | 761829 | 5,000 scalar programs across C, native and native without optimization. The 2,692 intentional panics agreed across the three modes. Zero mismatches or skips. |

Valgrind Memcheck with origin tracking replayed the first 5,000 stdlib cases on C and native with identical output
and zero errors. The dynamically linked C run freed all 273,954 allocations. Native uses its own allocator, so
that C heap count does not measure native allocations.

### Fixed and covered by regressions

- **TOML header panic.** `toml.get_string("[é\nname = \"wrong\"\n", "name")` cut a byte inside `é`.
  A bare `[` also reached the invalid slice path. Malformed headers now return the getter's default.
  `tests/std_toml_hostile.yx` exercises both paths.
- **TOML Unicode decoding.** `toml.get_string("name = \"é\"", "name")` returned `Ã©`, and quoted array items
  behaved the same way. The getters validate UTF-8 and preserve the bytes of literal characters. Regressions
  cover strings, arrays and invalid UTF-8 in `tests/std_toml.yx` and `tests/std_toml_hostile.yx`.
- **TOML multiline array cost.** Give `toml.get_array` a value made from `"items = [\n"`, 40,000 lines
  of `"0,\n"` and a final `"1]\n"`. Rescanning the accumulated text exceeded a 5-second native run
  budget. Each line is now scanned once. The same input returned all 40,001 items in about 0.02 seconds
  on the test host and is covered by `tests/std_toml_hostile.yx`.
- **JSON silent overflow.** `json.parse("1e999")` produced a non-finite Float, which the writer emitted as
  `null`. The reader now reports an out-of-range error. The writer still emits `null` for non-finite values
  supplied directly by callers.
- **JSON error-location panic.** Build the five-byte input `22 ff 5c 71 22` with `String.push_byte`,
  then pass the parse error's offset to `json.line_col`. Counting characters used to panic at byte `ff`.
  The helper now counts bytes for malformed input. Both JSON fixes are covered by `tests/std_json.yx`.

### Closed — JSON conversion near the limits of Float

The previous reader rounded `1e300` differently on successive parse and serialize passes,
rounded `9.9e-309` to zero and rejected `1.7976931348623157e308`. The shared
`text.parse_float` now rounds to the nearest binary64 value, with ties to even. JSON preserves
subnormals and signed zero and reports an out-of-range error when rounding overflows to infinity.
`tests/std_json_float_limits.yx` checks exact bits, halfway rounding, overflow, long decimals
and round trips in both backends. A separate comparison against Python's binary64 conversion
matched all 6,000 seeded decimal inputs in each backend (seed 27015).

### Open — selfhost lexer and large hexadecimal UInt64 literals

`fn main() { print(0x8000000000000000u64) }` is valid in the Rust compiler.
The selfhost lexer accumulates hexadecimal digits in a signed `Int` and panics
when the literal exceeds its range. The JSON rounding regression uses the
equivalent decimal spelling `9223372036854775808u64`, which the lexer supports.
