# Panic diagnostics

## Output

    panic: index out of bounds
      at examples/foo.yx:12:5

`panic:` line is unchanged; the `at path:line:col` line is the .yx position of the
failing operation (overflow, index, slice, unwrap/expect, division, explicit `panic`,
`assert`...). For failures inside library code (prelude) the location is the
**caller's** call site in user code.

`YERUX_BACKTRACE=1` (any value but `0`/empty) at run time adds

    stack backtrace:
       0: main.parse
       1: main.main

(Yerux function names, innermost first, no addresses). Without the variable nothing changes.

## C backend
* Each checked operation passes a `uint32_t` location id (`nxl_*` helpers), used only on the
  failure path; `nx_dbg` holds the loc table + file names.
* **Cost in `--release`.** A distinct constant per site stops GCC from merging the failure paths and it
  duplicates hot blocks (measured with every position on: structs +14 %, textops +19 %, json +15 %). So a
  plain C release carries the position only of checks that measured ~0 %: index/slice, `panic`, `assert`,
  `unwrap`/`expect` (their caller stores its position in `nx_cur_loc` right before the call). Positions of
  arithmetic (overflow, division), string byte ops, integer conversions and other library calls are left out
  (`panic:` line only). `--debug`, or a build made with `YERUX_LOCS=1` (or `YERUX_BACKTRACE`) keeps all of them.
  The native backend always has every position. `tests/run_all.sh` exports `YERUX_LOCS=1` so the goldens are
  complete and C == native.
* The ELF-symtab backtrace is compiled in (`-DNX_BACKTRACE`) only for `--debug` or a build with
  `YERUX_BACKTRACE` set (which also adds `-fno-inline -fno-optimize-sibling-calls` so every function keeps a
  frame). Merely having that code in the binary moved the hot code (textops +14 %). A plain build that is run
  with `YERUX_BACKTRACE=1` prints a one-line note saying to rebuild.
* Release timings after the change (bench/src, C backend, best of 11, before -> after): structs 208 -> 208 ms,
  json 6.4 -> 6.4, hashmap 19.6 -> 20.0, sort 229 -> 228, textops 63.9 -> 67.2 (hot code is
  instruction-identical; the rest is code layout).

## Native backend (x86-64 Linux)
* IR has `Inst::Loc(u32)` markers (cleaned/patched by opt, inliner, layout), stripped to per-inst
  locations before regalloc. Each panic site is a cold per-function stub that records its
  return address in a site table; the runtime reports by walking the rbp chain.
  Functions that call are no longer frameless. Active only when the program has user markers:
  hello world 3301 -> 3327 bytes.
* Limits: aarch64 ignores markers (no location line); Windows prints no locations;
  programs built by the selfhost compiler print the location of the same sites as the plain C release build (see below).

## DWARF / gdb for the native backend (x86-64 Linux)
`yerux build prog.yx --native --dwarf` (and `--debug`, which also turns the optimiser off) appends a section header
table, `.symtab`/`.strtab`, `.debug_abbrev`, `.debug_info`, `.debug_line` and `.debug_frame` after the loaded image
(they are not part of any PT_LOAD: the program itself is the same bytes, a few KB larger on disk). gdb then shows Yerux
function names and `.yx` lines:

    $ gdb -batch -ex 'break fact.yx:3' -ex run -ex bt ./prog
    Breakpoint 1 at 0x400f5a: file fact.yx, line 3.
    #0  fact () at fact.yx:3
    #1  0x0000000000400f7a in fact () at fact.yx:5
    ...
    #5  0x0000000000400fde in main () at fact.yx:14

Works: `break fun`, `break file.yx:N`, `bt` (stops at `main`), `up`/`down`, `finish`, `next`/`step` by line, `list`, `info line`,
`info symbol`. Not there: variables and types (`print x`, `info locals`): every row is a statement boundary, the
language tag is assembler, and parameters are not described.

How it is built (`src/dwarf.rs`, hooks in `x64::gen_func`):
* `ir::ALL_LOCS` puts a location marker before every instruction (normally only before checks and library calls); the
  stripped per-instruction locations become line rows `(code offset, file:line:col)`, one sequence per function.
* Frame state per code offset: `CFA = rsp+8` at entry and in shrink-wrapped early blocks and after `leave`;
  `rsp+16` after `push rbp`; `rbp+16` in the body. The prologue may be out of line (trampolines at the end of the
  function), so the table is recorded where the bytes are emitted, not by function.
* Not covered: runtime helpers (allocator, printing) have no symbols, programs with `extern` imports (the dynamic ELF
  writer) get no debug sections, and aarch64/Windows have none.

Panic locations (above) need none of this; they work without gdb.
