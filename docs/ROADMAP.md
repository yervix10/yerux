# Yerux master roadmap (M1..M12)

> **Status note:** the milestone table is a direction and effort estimate; the section "Measured status" below
> counts what exists in this repository (version 0.5.0), deliverable by deliverable, with the evidence for each
> count. The compiler has a C backend (default, the test oracle, needs `cc`) and a native backend (x86-64 Linux; AArch64
> Linux and Windows PE with documented limits). Framework milestones M4-M6 and M8-M11 have not been started.

Tracks: **L** language, **B** compiler/backend, **S** stdlib, **T** tooling, **P** platform layer, **R** rendering, **F** framework. Effort in person-months (pm) for one experienced engineer; see FFI.md and RENDERING.md for the basis. Cumulative totals assume strictly serial work; many items can overlap.

## Long-term dependency sketch

This sketch describes ordering for the proposed framework work. Language and
backend items already implemented are summarized in the milestone table and
their linked reference docs; this is not a current implementation dependency
graph.

```
 B: N2 regalloc --> N3 aggregates/ptrs/allocator --> F0 sized ints, floats, Ptr --> F1 dynamic ELF --> PE --> aarch64 --> Mach-O
 L: generics, String, Option/Result, modules --> closures + interfaces --> threads/effects --> self-host
 S: syscalls, bytes, fmt --> collections --> sockets/time/threads --> unicode tables, inflate, codecs
 P: Wayland (no FFI) --> X11 --> Windows --> macOS --> Android/iOS ; IME and a11y follow each
 R: CPU raster + TTF text --> display list/layers --> Vulkan RHI + GPU renderer --> complex text --> color fonts
 F: layout + button demo --> widget core --> widget library --> hot restart/reload --> devtools
 Gates: F needs closures+interfaces; GPU needs F1 (dlopen); macOS/iOS need aarch64 + Mach-O + signing
```

## Milestones

| M | Name | Contents | Depends on | Effort | Done when |
|---|---|---|---|---|---|
| **M1** | Native backend foundations | x86-64 Linux native backend, register allocation, structs/enums/match, borrows, allocator and drop glue are implemented. AArch64 Linux covers core IR and basic OS services. Windows PE support is present with documented limitations. C remains the default backend and test oracle. | substantially complete | — | See `BACKEND.md` for supported features and target-specific gaps. |
| **M2** | Systems types and language core | Sized integers, `Float`/`Float32`, `Ptr<T>` construction, `String`, generic `List<T>`, `Option`/`Result`/`?`, modules, formatting, ranges, compile-time evaluation, fixed-size arrays `[T; N]` and second-class slices `view [T]`/`mut [T]` are implemented (docs/ARRAYS.md: owned elements, `==`/`print`, `const` items, `const N: Int` parameters, chunks/windows/split_at, arrays in comptime). | substantially complete | — | See `SPEC.md` and `docs/STDLIB.md`. |
| **M3** | Stdlib core + FFI | `std.io` has an implemented first slice, extended by `std.fs` (directories, metadata), `std.env` and the clocks/calendar of `std.time` (Linux x86-64 and AArch64 native + C backend), then by file handles (explicit close), binary/buffered I/O, links, `env.set` and `time.Duration` (docs/STDLIB_SYSTEM.md), and `std.process` on x86-64 native and C (docs/STDLIB_PROCESS.md). C FFI works through the C backend and Linux x86-64 native path; dynamic ELF loading is implemented for that native path. `std.net` provides TCP sockets with configurable timeouts and endpoint queries (docs/NET.md) and `std.http` a small HTTP/1.1 client; also `std.log`, `std.kv`, `std.compress` (run-length), `std.unicode`, `std.sync`, and the text/encoding/regex/csv/toml/hash/sort/iter/collections modules (docs/STDLIB.md). The broader syscall, polling and capability design remains future work. | partial | — | See `docs/STDLIB_IO.md` and `docs/FFI.md`. |
| **M4** | Window + pixels (Linux) | Wayland client from scratch, memfd+shm, xdg-shell, events; first CPU fill; headless Weston CI + golden images | M3 (sockets only; no FFI needed) | 3-5 | Solid-colour window, closes cleanly |
| **M5** | Text and first widgets | Closures and function values (done); interfaces/traits with dynamic dispatch (done: `Box<dyn Trait>`); TTF reader, outline rasterizer, glyph cache; display list; pointer/keyboard input; box-constraint layout core; "counter" app | M4, language work | 6-9 | Line of text and clickable button (Linux Wayland, CPU) |
| **M6** | Framework core + X11 | Element/render tree, rebuild/state model (decision on diff vs signals), gestures, focus, scroll, animation ticker/vsync; X11 backend; hot restart | M5 | 8-12 | Todo-list app with text field (Latin) runs on Wayland and X11 |
| **M7** | Self-hosting and tooling | Compiler rewritten in Yerux (partial: `selfhost/`, a bootstrap of the front end and C emitter for the subset the stages use); formatter (done, `yerux fmt` + a self-hosted copy identical on the whole corpus); LSP (done, `yerux lsp` + VS Code extension); package manager (SemVer, transitive path/Git resolution and pinned locks, `docs/PACKAGES.md`); DWARF/debug info (done: x86-64 native `--dwarf`, line tables, frame unwinding, parameters and locals for gdb); module object format (done: `yerux obj`, `--obj`, `docs/MODULE_OBJECT.md`; stage 1 = interface + sources + hashes, stage 2: `--separate` caches C objects per module); disassembler for encoder tests (done: `tests/disasm_check.sh`); fuzzing (done: `tests/fuzz*.py`) | M2-M5 (can run in parallel with M5-M6) | 10-14 | Compiler builds itself; incremental builds |
| **M8** | GPU | Vulkan RHI via `dlopen`, shader IR + SPIR-V, SDF primitive renderer, tessellated paths, glyph atlas, layer compositor, partial present; CPU renderer kept as fallback | M6, FFI F2, M7 helpful | 10-14 | Same demo at 60-120 fps on GPU, identical output within tolerance |
| **M9** | Second OS: Windows | PE writer (imports, `.pdata`), Win64 convention + probes, callbacks, Win32 window, IME (TSF), DPI, Vulkan on Windows | M3 (F0-F3), M8 | 8-12 | Demo + text field on Windows |
| **M10** | World text, IME, accessibility | Shaping engine (GSUB/GPOS, Arabic, Hebrew, bidi), CJK + emoji/color fonts, hyphenation, Wayland text-input-v3 IME, AT-SPI and UIA trees, selection/caret everywhere | M6, M9 | 15-25 | RTL and CJK editing; screen reader reads buttons and fields |
| **M11** | Widget library, hot reload, devtools | ~40-100 widgets (lists, nav, dialogs, menus, theming), design system, patchable hot reload (relocatable modules), inspector, perf overlay, image codecs (PNG/JPEG/GIF/WebP) | M6, M7, M10 | 20-30 | Non-trivial app built entirely in Yerux |
| **M12** | Apple, mobile, web | aarch64 backend, Mach-O + signing + ObjC bridge (macOS, iOS), Android `.so` + APK + JNI, wasm backend; Metal RHI; per-OS IME/a11y; release 1.0 | M8-M11 | 40-60 | Same app on macOS, Android, iOS (and web) |

## Measured status (version 0.5.0)

Method: each milestone's deliverables are the items listed in its "Contents" cell above (100 in total). An item
counts as 1 when it is implemented and covered by a test in the suite, 0.5 when it exists but only partially (the
gap is named), 0 otherwise. The percentages are item counts, **not** effort-weighted: M4-M6 and M8-M11 are the
expensive milestones and are at zero.

| M | Done / items | % | Evidence and gaps |
|---|---|---|---|
| M1 | 9 / 9 | 100 | x86-64 backend, register allocation, structs/enums/match, borrows, allocator, drop glue, AArch64 core IR and Linux system services (`tests/a64_all.sh`), Windows PE (`tests/wine_all.sh`; not run on real Windows), C backend as oracle |
| M2 | 12 / 12 | 100 | sized ints, floats, `Ptr<T>`, `String`, `List<T>`, `Option`/`Result`/`?`, modules, formatting, ranges, `comptime`, arrays, slices |
| M3 | 12 / 13 | 92 | `std.io/fs/env/time/process/net`, handles, binary/buffered I/O, links, `Duration`, C FFI, dynamic ELF; missing: the broader capability design |
| M4 | 0 / 6 | 0 | no Wayland, shm, xdg-shell, windowing code in the repository (RENDERING.md is design only) |
| M5 | 2 / 9 | 22 | closures and `Box<dyn Trait>` exist; no TTF reader, rasterizer, glyph cache, display list, input or layout |
| M6 | 0 / 8 | 0 | SIGNALS.md is a design document |
| M7 | 6.5 / 8 | 81 | formatter, LSP, package manager (path/Git resolution, no registry), disassembler check, fuzzing done; self-hosting partial (0.5, 431/447 candidates, see below); DWARF debug info done (`--native --dwarf`: line tables, stack unwinding, locals/params in gdb); module object format stage 1 done (docs/MODULE_OBJECT.md); stage 2 done for the C backend (`--separate`, per-module C objects cached); missing: native backend and objects embedded in .yxo |
| M8 | 0 / 7 | 0 | |
| M9 | 1.5 / 7 | 21 | PE writer (1.0: `.pdata`/`.xdata` unwind info, ASLR with an empty `.reloc`, checked on a Windows Server 2022 VM; console only) and Win64 calling convention at the OS boundary; no callbacks, Win32 window, IME, DPI, Vulkan |
| M10 | 0 / 6 | 0 | `std.unicode` is a small helper set, not a shaping engine |
| M11 | 0 / 6 | 0 | `std.compress` is run-length encoding, not an image codec |
| M12 | 0.5 / 9 | 6 | AArch64 Linux backend (0.5: core IR, console, filesystem, environment, clocks and blocking TCP); Apple, mobile and web pending |
| **All** | **43.5 / 100** | **43.5** | |

The AArch64 system fixture group passed 31/31 under QEMU, up from 4/31 at
`a1e3b3a` on the same corpus. See the [AArch64 backend limits](BACKEND.md).

### Numbers behind the table

Taken from the repository at the time of writing (re-measure with the commands shown):

| Measure | Value | How |
|---|---|---|
| Rust compiler and tools | 43,866 lines in `src/` (`*.rs`, `syntax/`, `lsp/`) | `cat src/*.rs src/syntax/*.rs src/lsp/*.rs \| wc -l` |
| Library written in Yerux | prelude 3,605 lines; 27 `std.*` modules, 7,781 lines | `src/prelude*.yx`, `src/std/*.yx` |
| Self-hosted compiler | 10,616 lines in 12 files (`selfhost/`) | `cat selfhost/*.yx \| wc -l` |
| Test programs | 598 `tests/*.yx`, 63 `examples/*.yx`, 1,188 expected outputs, 45 scripts, 12 fuzzers/checkers in Python | `ls` |
| Doc-tests | every `yerux` block of the book, GUIDE.md and STDLIB.md | `bash tests/doctests.sh` |

Self-hosting parity measured by `tests/selfhost_*.sh`:

| Stage (`selfhost/`) | Result |
|---|---|
| lexer | 859 files compared, 0 mismatches |
| parser (source AST) | 916 ASTs compared plus 8 lexical and 11 syntax rejections, 0 mismatches |
| module resolution | 896 resolved, 20 module rejections, 19 source rejections, 0 mismatches |
| lexical bindings, types, ownership | 916 lexical snapshots, 5 typed and 4 ownership fixtures, 21 type and 13 ownership rejections, 0 mismatches |
| formatter | identical to `yerux fmt` on the whole dirty corpus, idempotent |
| bootstrap (`tests/selfhost_codegen.sh`) | 7 of 7 stage executables rebuilt by the compiler they were built with, byte-identical C |

The self-hosted compiler matched the Rust C backend's stdout, stderr and
exit status for **458 of 459 candidates**, including all 14 required
concurrency cases and 101 runtime panics. The run had one expected type
rejection, zero candidates outside the subset and **zero execution
mismatches**. All seven stages rebuilt with byte-identical C between
self-hosted generations. The generated C from Rust and Yerux differed in
all 14 concurrency cases. The remaining work to switch the default compiler
is tracked in [SELFHOST.md](SELFHOST.md).
Every program of the corpus, concurrency
included, now compiles; what is left before it can be called self-hosted is
language surface the corpus does not exercise (the closing list of
[SELFHOST.md](SELFHOST.md): slices as bindings, `extern`, generic function items
as values, precise loans for nested calls), the native backends and tools that
exist only in the Rust binary, and diagnostics with spans.
The milestone remains at 0.5 while the self-hosted compiler covers a subset
of the language and standard library.

**Cumulative effort estimate (serial): about 135-205 pm (11-17 years of one person).** Shipping product thresholds: **M6** = usable Linux toy toolkit (about 30-45 pm in); **M9** = two-OS toolkit (about 60-90 pm); **M11** = serious toolkit; M12 = Flutter-class reach.

## Rules of thumb for sequencing

1. **Every milestone ends with a visible demo** and a golden-image or run test. Never do two infrastructure milestones in a row without a demo.
2. **Language features are pulled by the framework**, not designed ahead: add closures, interfaces and generics when M5 code needs them, with the ownership model (no lifetimes, second-class borrows) tested against real UI code early (it is the biggest design risk: shared mutable state for reactivity).
3. **CPU rendering before GPU, Linux before others, x86-64 before aarch64.** Each choice delays risk (drivers, signing, second ISA) until the architecture has stabilised.
4. **Keep the static no-libc target as the reference.** Dynamic linking is an additional output mode (FFI.md), required for GPU on Linux and for every non-Linux OS.
5. **Re-estimate at M5.** Text and platform effort have the widest uncertainty (RENDERING.md section 3); calibrate against actual M4-M5 velocity before committing to M9-M12.

## Open decisions (see final report)

Compatibility and stability levels for 1.0, and the list of names to settle before the freeze, are in [COMPATIBILITY.md](COMPATIBILITY.md).

Policy on system libraries (dynamic linking, libc/dlopen); scope of "from scratch" for text (own shaping for all scripts vs OS text services where available); concurrency model (decided: CONCURRENCY.md) and reactivity model (decided: SIGNALS.md), whose spec work is done and whose implementation is needed by M5-M6 (see also RANGES.md and COMPTIME.md for the language features pulled by M2-M6); first extra platform (Windows vs macOS).
