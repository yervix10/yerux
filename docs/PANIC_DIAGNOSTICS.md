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
  failure path; `nx_dbg` holds the loc table + file names. Library functions that are not used
  as values get a hidden trailing `nx_l` parameter (caller's location).
* `nx_backtrace()` reads `nx_f_*` names from `.symtab` of `/proc/self/exe`. For reliable
  frames build with `YERUX_BACKTRACE` set (adds `-fno-inline -fno-optimize-sibling-calls`);
  otherwise inlined/tail-called frames can be missing (a note is printed if none are found).
* Cost in `--release`: location constants stop GCC from merging panic blocks:
  measured structs +14 %, textops +19 %, json ~+15 %; other benchmarks ~0 %.

## Native backend (x86-64 Linux)
* IR has `Inst::Loc(u32)` markers (cleaned/patched by opt, inliner, layout), stripped to per-inst
  locations before regalloc. Each panic site is a cold per-function stub that records its
  return address in a site table; the runtime reports by walking the rbp chain.
  Functions that call are no longer frameless. Active only when the program has user markers:
  hello world 3301 -> 3327 bytes.
* Limits: aarch64 ignores markers (no location line); Windows prints no locations;
  programs built by the selfhost compiler print no location (loc 0).

## Design: DWARF / gdb for native (not implemented)
Minimal viable plan: emit section headers into the ELF with `.symtab`/`.strtab` (one FUNC symbol
per Yerux function; the `funcs` table already exists in the `dbg` blob) and a `.debug_line`
(DWARF 4, one sequence per function, rows from the same per-inst loc table; the `sites`/`locs`
tables give offset->(file,line,col)). That gives `bt`, `break main.foo` and `list`-less stepping
by line in gdb. `.debug_info` (CU + subprograms with `low_pc/high_pc`) is a small addition;
variables/types (`DW_TAG_variable`, location lists from regalloc) are the expensive part and
are out of scope. Work is mostly a writer for those sections plus offsets recorded per inst in x64.rs.
