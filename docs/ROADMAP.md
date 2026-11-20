# Yerux master roadmap (M1..M12)

Tracks: **L** language, **B** compiler/backend, **S** stdlib, **T** tooling, **P** platform layer, **R** rendering, **F** framework. Effort in person-months (pm) for one experienced engineer; see FFI.md and RENDERING.md for the basis. Cumulative totals assume strictly serial work; many items can overlap.

## Dependency spine

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
| **M1** | Native backend complete (N2, N3) | Register allocation, SSA-lite opts, structs/enums/match, `view`/`mut` pointers, `mmap` allocator, drop glue; all examples and tests pass on the native backend; C backend retired to a test oracle | now | 4-6 | `yerux build` needs no `cc` for the whole current language; speed within 10% of gcc -O2 on the benchmark |
| **M2** | Systems types | Sized ints, `Float32/64` + SSE2, `Ptr<T>` + `unsafe`, arrays/slices, heap `String`/`Text`, generic `List<T>`, `Option/Result/?`, modules, integer/float formatting | M1 | 5-7 | A program can parse a binary file format and print floats |
| **M3** | Stdlib core + FFI stage 1 (first slice done: `std.io`, see STDLIB_IO.md) | Syscall wrappers (files, mmap, sockets, time, env, args, `epoll`), byte buffers, formatter, test runner; **dynamic ELF writer + `extern "C"` + `dlopen`** (FFI F1-F2); `ffi` capability stub | M2 | 4-6 | `extern` call to libc `write`; open a Unix socket and echo |
| **M4** | Window + pixels (Linux) | Wayland client from scratch, memfd+shm, xdg-shell, events; first CPU fill; headless Weston CI + golden images | M3 (sockets only; no FFI needed) | 3-5 | Solid-colour window, closes cleanly |
| **M5** | Text and first widgets | Closures and function values; interfaces/traits with dynamic dispatch; TTF reader, outline rasterizer, glyph cache; display list; pointer/keyboard input; box-constraint layout core; "counter" app | M4, language work | 6-9 | Line of text and clickable button (Linux Wayland, CPU) |
| **M6** | Framework core + X11 | Element/render tree, rebuild/state model (decision on diff vs signals), gestures, focus, scroll, animation ticker/vsync; X11 backend; hot restart | M5 | 8-12 | Todo-list app with text field (Latin) runs on Wayland and X11 |
| **M7** | Self-hosting and tooling | Compiler rewritten in Yerux; formatter; LSP; package manager; DWARF/debug info; object format for modules; disassembler for encoder tests; fuzzing | M2-M5 (can run in parallel with M5-M6) | 10-14 | Compiler builds itself; incremental builds |
| **M8** | GPU | Vulkan RHI via `dlopen`, shader IR + SPIR-V, SDF primitive renderer, tessellated paths, glyph atlas, layer compositor, partial present; CPU renderer kept as fallback | M6, FFI F2, M7 helpful | 10-14 | Same demo at 60-120 fps on GPU, identical output within tolerance |
| **M9** | Second OS: Windows | PE writer (imports, `.pdata`), Win64 convention + probes, callbacks, Win32 window, IME (TSF), DPI, Vulkan on Windows | M3 (F0-F3), M8 | 8-12 | Demo + text field on Windows |
| **M10** | World text, IME, accessibility | Shaping engine (GSUB/GPOS, Arabic, Hebrew, bidi), CJK + emoji/color fonts, hyphenation, Wayland text-input-v3 IME, AT-SPI and UIA trees, selection/caret everywhere | M6, M9 | 15-25 | RTL and CJK editing; screen reader reads buttons and fields |
| **M11** | Widget library, hot reload, devtools | ~40-100 widgets (lists, nav, dialogs, menus, theming), design system, patchable hot reload (relocatable modules), inspector, perf overlay, image codecs (PNG/JPEG/GIF/WebP) | M6, M7, M10 | 20-30 | Non-trivial app built entirely in Yerux |
| **M12** | Apple, mobile, web | aarch64 backend, Mach-O + signing + ObjC bridge (macOS, iOS), Android `.so` + APK + JNI, wasm backend; Metal RHI; per-OS IME/a11y; release 1.0 | M8-M11 | 40-60 | Same app on macOS, Android, iOS (and web) |

**Cumulative (serial): about 135-205 pm (11-17 years of one person).** Shipping product thresholds: **M6** = usable Linux toy toolkit (about 30-45 pm in); **M9** = two-OS toolkit (about 60-90 pm); **M11** = serious toolkit; M12 = Flutter-class reach.

## Rules of thumb for sequencing

1. **Every milestone ends with a visible demo** and a golden-image or run test. Never do two infrastructure milestones in a row without a demo.
2. **Language features are pulled by the framework**, not designed ahead: add closures, interfaces and generics when M5 code needs them, with the ownership model (no lifetimes, second-class borrows) tested against real UI code early (it is the biggest design risk: shared mutable state for reactivity).
3. **CPU rendering before GPU, Linux before others, x86-64 before aarch64.** Each choice delays risk (drivers, signing, second ISA) until the architecture has stabilised.
4. **Keep the static no-libc target as the reference.** Dynamic linking is an additional output mode (FFI.md), required for GPU on Linux and for every non-Linux OS.
5. **Re-estimate at M5.** Text and platform effort have the widest uncertainty (RENDERING.md section 3); calibrate against actual M4-M5 velocity before committing to M9-M12.

## Open decisions (see final report)

Policy on system libraries (dynamic linking, libc/dlopen); scope of "from scratch" for text (own shaping for all scripts vs OS text services where available); concurrency model (decided: CONCURRENCY.md) and reactivity model (decided: SIGNALS.md), whose spec work is done and whose implementation is needed by M5-M6 (see also RANGES.md and COMPTIME.md for the language features pulled by M2-M6); first extra platform (Windows vs macOS).
