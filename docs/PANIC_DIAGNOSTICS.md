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

## Design: DWARF / gdb for native (not implemented)
Minimal viable plan: emit section headers into the ELF with `.symtab`/`.strtab` (one FUNC symbol
per Yerux function; the `funcs` table already exists in the `dbg` blob) and a `.debug_line`
(DWARF 4, one sequence per function, rows from the same per-inst loc table; the `sites`/`locs`
tables give offset->(file,line,col)). That gives `bt`, `break main.foo` and `list`-less stepping
by line in gdb. `.debug_info` (CU + subprograms with `low_pc/high_pc`) is a small addition;
variables/types (`DW_TAG_variable`, location lists from regalloc) are the expensive part and
are out of scope. Work is mostly a writer for those sections plus offsets recorded per inst in x64.rs.

## Self-hosted compiler
`yerux-self` programs print `at file:line:col` for the sites the plain C release build reports: element access and
mutation on List/Array/Slice (`at`, `get`, `set`, `swap`, `pop`, `xs[i]`, `xs[a..b]`), `chunks`/`windows` with a zero
size, `unwrap`/`expect` of an Option, `assert*` and `panic` (unless its message is a built temporary, e.g. an
interpolated string). The lexer's `--columns` adds the column to each token row (`line:col:Kind..`); in `--build`
mode the parser appends `Pos(line,col)` as the last child of `Method`, `Slice` and the `panic`/`assert*` `Call`
(position of the start of the expression, as in the Rust compiler); the AST of the parity scripts is unchanged.
`codegen FILE.ast NAME USER-LINES` stamps `nx_cur_loc` around those sites and writes the `nx_dbg` table; the runtime
reads it because the program defines `NX_SELFHOST_LOCS`. The driver puts the prelude *after* the user's source so
lines stay true, and sites past `USER-LINES` (prelude code) carry no position. Limits: every site is attributed
to the entry file (modules print their line under the entry name); positions inside `{..}` interpolations use the
string's column.
