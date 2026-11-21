# Yerux master roadmap (M1..M12)

> **Status note:** this is a direction and effort estimate, not a checklist of
> the compiler's current feature status. The original M1–M3 descriptions below
> have been updated to reflect the implementation recorded in `SPEC.md`,
> `BACKEND.md`, and the focused docs. The later framework milestones remain
> proposed work. In particular, the compiler still has a C backend and uses
> `cc` for its default output; native output does not yet cover every feature
> or target.

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
| **M1** | Native backend foundations | x86-64 Linux native backend, register allocation, structs/enums/match, borrows, allocator and drop glue are implemented. AArch64 Linux core IR and Windows PE support are present with documented limitations. C remains the default backend and test oracle. | substantially complete | — | See `BACKEND.md` for supported features and target-specific gaps. |
| **M2** | Systems types and language core | Sized integers, `Float`/`Float32`, `Ptr<T>` construction, `String`, generic `List<T>`, `Option`/`Result`/`?`, modules, formatting, ranges, compile-time evaluation, fixed-size arrays `[T; N]` and second-class slices `view [T]`/`mut [T]` are implemented (docs/ARRAYS.md; still missing: owned array elements, `==`/`print` on arrays, const-generic lengths). | substantially complete | — | See `SPEC.md` and `docs/STDLIB.md`. |
| **M3** | Stdlib core + FFI | `std.io` has an implemented first slice, extended by `std.fs` (directories, metadata), `std.env` and the clocks/calendar of `std.time` (Linux x86-64 native + C backend). C FFI works through the C backend and Linux x86-64 native path; dynamic ELF loading is implemented for that native path. The broader syscall, networking, and capability design remains future work. | partial | — | See `docs/STDLIB_IO.md` and `docs/FFI.md`. |
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
