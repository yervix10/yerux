# Yerux rendering engine and UI framework: architecture and effort

Status: research proposal. Goal: a Flutter-class framework with its own renderer, written in Yerux. Estimates are **person-months (pm) of an experienced engineer** to reach "good enough to ship an app" quality, not parity with 15-year-old products. They are order-of-magnitude judgements from comparable open-source projects, not measurements.

## 0. Conclusions up front

1. **Total for a Flutter-lite (excluding compiler/stdlib): roughly 130-200 pm, i.e. 11-16 person-years.** One person full-time: a decade-scale project. The plan only works if it is staged so that each stage is a usable product.
2. **Verified suspicion: text and platform integration are the two true time sinks**, each larger than GPU rasterization. Rough split of the effort: text 20-25%, platform/windowing/input/IME/accessibility 20-25%, widget library 15%, GPU abstraction + 2D raster 15-20%, everything else 20%.
3. Evidence for the text claim: even the "from-scratch" Rust ecosystem composes a text stack from several separately maintained projects. Parley (rich-text layout) is built on HarfRust (HarfBuzz port), Fontique (font enumeration/fallback), Skrifa (font reading) and ICU4X (bidi, segmentation, normalisation). Flutter itself delegates text to HarfBuzz, ICU and FreeType.
4. **A software rasterizer first is the right call**, not a compromise. Every target OS has a way to present a CPU pixel buffer (Wayland `wl_shm`, X11 PutImage/SHM, Win32 `StretchDIBits`, Cocoa `CALayer` contents, Android `ANativeWindow_lock`, iOS `CALayer`). That gives a window + pixels on **all** platforms with only platform FFI, and no GPU API, shader compiler or driver issues. The GPU backend is then an optimisation behind the same display-list interface.
5. **The language is the early critical path**: a UI framework needs closures, generics, trait-like interfaces/dynamic dispatch, strings, maps, and a concurrency story. Yerux today has none of them. Compiler milestones gate the framework, not the other way round.

## 1. Layer map

```
 App code (Yerux)
 +--------------------------------------------------------------+
 | Widgets / state / reactivity / themes        (framework)     |
 | Gestures, focus, accessibility semantics tree                 |
 | Layout (constraints) -> render objects -> layers              |
 | Display list (recorded draw ops) + compositor / layer tree    |
 | 2D renderer: CPU rasterizer | GPU renderer (same input)       |
 | Text: fonts, shaping, bidi, line breaking, glyph raster/cache |
 | Images/codecs, colour management                              |
 | GPU abstraction (Vulkan/Metal/D3D/GLES) + shader compiler     |
 | Platform: window, input, IME, clipboard, DnD, a11y, vsync     |
 | Yerux runtime: allocator, threads, syscalls / FFI             |
 +--------------------------------------------------------------+
```

The key interface is the **display list** (an immutable, serialisable list of draw ops with transform/clip/paint state). Widgets never talk to a rasterizer directly; this is what lets us swap CPU/GPU renderers, test with golden images, and later hot-reload.

## 2. Components

### 2.1 Platform layer: windowing, input, IME, accessibility

**What it does.** Creates windows/surfaces, delivers pointer/keyboard/touch events, text input composition (IME), clipboard, drag and drop, cursors, DPI/scale changes, vsync-paced frame callbacks, dialogs, and exposes the accessibility tree to screen readers.

**From scratch means.** Speaking each OS's native protocol or API ourselves, with no GLFW/SDL/winit.

| Platform | How | pm (window+input) | pm (IME) | pm (a11y) |
|---|---|---|---|---|
| Linux Wayland | Wire protocol over `AF_UNIX` (generated from the protocol XML): `wl_compositor`, `wl_shm`, `xdg_shell`, `wl_seat`, `xdg-decoration`, `cursor-shape-v1`, `fractional-scale`, `viewporter`, `text-input-v3`, `data-device`. Own xkb keymap parser (keyboard layouts, compose, dead keys). GNOME has no server-side decorations, so we draw our own title bar. | 3-4 | 1.5 | - |
| Linux X11 | X11 wire protocol (handshake, `~/.Xauthority` cookie, `CreateWindow`, `PutImage`/MIT-SHM, XInput2, XKB, selections, XIM/IBus/Fcitx) | 2-3 | 1.5 | - |
| Linux a11y | AT-SPI over D-Bus: needs an own D-Bus client (auth, marshalling) and the AT-SPI object model | - | - | 2-3 |
| Windows | Win32 (`CreateWindowEx`, message loop, `WM_*`), DPI awareness, IME via TSF (COM vtables by hand), UI Automation provider (COM), DWM, clipboard | 3-4 | 2 | 2 |
| macOS | Cocoa through the Objective-C runtime (`NSApplication`, `NSWindow`, `NSView` subclass, `NSTextInputClient`), `CAMetalLayer`, `NSAccessibility` | 3-4 | 1.5 | 2 |
| Android | Native activity + JNI (`InputConnection`, `AccessibilityNodeProvider`), `ANativeWindow`, lifecycle, soft keyboard insets, permissions | 4-5 | 2 | 1.5 |
| iOS | UIKit via ObjC, `UITextInput`, `UIAccessibility`, lifecycle, signing | 3-4 | 1.5 | 1.5 |
| Web (wasm + canvas/WebGPU) | wasm backend (BACKEND N4) + JS glue | 3 | 1 | 1.5 |

Total for five native platforms: **about 40-55 pm** (one pm figure per cell, added up, includes testing on real devices, which is the expensive part). Desktop Linux + one more desktop OS: about 15-20 pm.

**Biggest risks.** IME and accessibility are poorly documented and vary per compositor/toolkit; keyboard layouts and dead-key composition; Wayland protocol fragmentation (each compositor supports different extensions); Apple signing/notarisation friction; Android/iOS lifecycle quirks; testing matrix size.

**Is it a time sink? Yes.** Individually modest, collectively the largest, and it never finishes (new OS releases).

### 2.2 GPU abstraction

**What it does.** A small "render hardware interface" (RHI): device/queue, buffers, textures, samplers, render pipelines, render passes, compute dispatch, fences, swapchain, memory allocation, and a way to author shaders once.

**From scratch means.** Bind Vulkan (and Metal, D3D12, GLES) ourselves through FFI (needs FFI.md stages F1-F2), write our own memory sub-allocator (Vulkan limits the number of allocations), our own synchronisation helpers and swapchain recreation, and a **shader pipeline**: author in one language, emit SPIR-V (Vulkan), MSL (Metal), HLSL/DXIL (D3D). Our UI needs perhaps 20-50 shaders, so the plan is a tiny shader IR with three emitters rather than a general compiler (naga/Tint-class projects are 100k+ lines).

| Part | pm |
|---|---|
| Vulkan RHI (instance, device, swapchain, sync, allocator, descriptors/push constants, pipeline cache) | 4-6 |
| Shader IR + SPIR-V emitter | 2 |
| Metal RHI + MSL emitter | 2-3 |
| D3D12 RHI + HLSL/DXIL (DXIL must be signed by Microsoft's `dxil.dll`; use D3D11 + runtime HLSL compilation instead, or skip D3D and use Vulkan on Windows) | 3-4 (or 0) |
| GLES3 fallback (old Android, VMs) | 1.5 |

**Recommendation.** Vulkan first (covers Linux, Windows, Android with every modern driver), Metal for Apple, no native D3D until proven necessary. About **8-12 pm** for Vulkan + Metal + shader IR.

**Risks.** Driver bugs (especially mobile GPUs and Intel on Windows), validation-layer-only bugs, GPU-less CI (use lavapipe/SwiftShader for testing), no GPU reachable from a libc-free process (dependent on FFI).

### 2.3 2D rasterization: Skia vs Impeller vs Vello vs Graphite vs software

| Engine | Approach | Notes for us |
|---|---|---|
| **Skia (Ganesh)** | Mature immediate-mode API; GPU backend tessellates/atlases paths, shader-per-state (runtime shader compilation, "jank" on first use); huge CPU fallback | Millions of lines, 15+ years, large team. Not reproducible from scratch; useful as a *feature checklist* only. |
| **Skia Graphite** | Skia's newer GPU backend: records in multiple threads, pipelines compiled ahead, Dawn/Metal/Vulkan; replacing Ganesh | Cited from knowledge; not re-verified in this session (docs site blocked fetch). Same size problem as Skia. |
| **Impeller (Flutter)** | AOT-compiled shaders and prebuilt pipelines for predictable performance; backends Metal and Vulkan (OpenGL ES fallback on old Android); one fixed set of shaders; paths tessellated on CPU | The design goal we copy: **no runtime shader compilation, predictable frame time**. Flutter docs state it is the only engine on iOS and the default on Android 29+, macOS, Linux and Windows. |
| **Vello** | Compute-shader pipeline: path flattening, binning, coarse/fine rasterization with prefix sums, almost no CPU work. Original "vello" crate is *experimental, compute-capable GPUs only*; newer **Vello GPU / Vello CPU** use a "sparse strips" architecture; per its README Vello CPU is currently the most mature variant | Technically the most elegant design and the likeliest long-term winner, but it needs compute shaders and has been through repeated rewrites; a moving target. |
| **Software (tiny-skia / font-rs style)** | Scanline/tile coverage accumulation, analytic anti-aliasing, SIMD blending | 4-6 pm to a decent AA path rasterizer (fill rules, strokes with joins/caps/dashes, clip masks, gradients, blend modes). |

**Recommended plan (3 steps, same display-list input):**

1. **CPU rasterizer** (tile-based, analytic coverage, SIMD later, multi-threaded by tiles). 5-7 pm total including strokes, gradients, images, clips, blend modes, blur/shadow (box-blur approximations). Doubles as the golden-image reference and as the fallback on machines without usable GPUs.
2. **GPU renderer, Impeller-style**: SDF fragment shaders for rects, rounded rects, circles, shadows (the 90% of UI); CPU tessellation + stencil-then-cover for general paths; glyph atlas for text; gradients and blends as shader variants; offscreen layers for opacity/blur/backdrop filters. 8-12 pm (needs the RHI of 2.2).
3. **Optional Vello-style compute backend** only if tessellation shows quality/perf problems (huge, complex vector scenes). Not needed for typical UI. 6-10 pm, unplanned.

**Risks.** Anti-aliasing quality (conflation artefacts for adjacent shapes), thin-stroke correctness, precision for huge coordinate ranges, GPU/CPU result drift (solve by golden tests with tolerance).

### 2.4 Text stack (the biggest single time sink)

| Sub-component | What it does | From scratch in practice | pm |
|---|---|---|---|
| Font parsing | `cmap`, `head/hhea/hmtx/maxp`, `glyf` (simple + composite), `loca`, `CFF`/`CFF2` (Type 2 charstrings), `kern`, `GDEF/GSUB/GPOS` tables, `fvar/gvar/avar/HVAR` (variable fonts), `name`, `OS/2`; WOFF2 needs a **Brotli decoder** | Table readers are mechanical; CFF2 and variable fonts are fiddly | 3-5 |
| Unicode data | Normalisation, general category, scripts, grapheme/word/sentence (UAX 29), line break classes (UAX 14), East Asian width, emoji properties, case mapping | Generated tables from UCD; must be updated every Unicode release | 1.5-2 |
| Bidi (UAX 9) | Resolve embedding levels, reorder runs, mirrored glyphs | Algorithm is specified and testable (BidiTest.txt) | 1-1.5 |
| Itemisation | Split text into runs by script, language, font, direction | | 0.5 |
| Shaping | GSUB/GPOS lookups (ligatures, kerning, marks, contextual); script-specific shapers (Arabic joining, Indic reordering, Thai/Lao/Khmer/Myanmar, Hangul, the Universal Shaping Engine) | HarfBuzz is ~150k lines with a large conformance suite and decades of font-specific workarounds. Realistic scope: Latin/Cyrillic/Greek/CJK/Hebrew/Arabic first (4-6 pm), Indic and USE scripts later (+5-8 pm). Full parity: 20+ pm and never really "done". | 4-6 initial, 10-15 broad |
| Line breaking and layout | Break opportunities, hyphenation (Liang patterns), justification, ellipsis, rich spans, inline widgets, tab stops, vertical metrics, caret/selection geometry, hit testing, bidi caret movement | Paragraph layout is where most "text bugs" live | 3-4 |
| Font discovery and fallback | Locate system fonts (fontconfig dirs, Windows/macOS font folders), match by family/weight/style/language, per-character fallback chain, color emoji font selection | Own matcher; avoid parsing fontconfig XML fully | 1.5-2 |
| Glyph rasterization | Outline to coverage via our path rasterizer; subpixel positioning; optional LCD AA; **hinting is skipped** (auto-light at most; TrueType bytecode interpreter is 2-3 pm and rarely worth it on HiDPI) | Reuses 2.3 | 1 |
| Glyph cache and atlas | LRU glyph cache, GPU atlas (bitmap or SDF/MSDF for scalable text), fallback to path rendering for huge sizes | | 1.5-2 |
| Color fonts and emoji | COLRv0/v1 (gradients, compositing graph, variable), CBDT/CBLC and sbix (embedded PNG), SVG-in-OpenType, emoji ZWJ sequences, skin tones | COLRv1 is a mini graphics language | 2-3 |
| Editing | Text input: caret, selection, composing region, undo/redo, clipboard, password fields, platform IME protocol (2.1) | Text field is the hardest single widget | 3-4 |
| Accessibility text | Text range/selection exposure | in 2.1 | - |

**Total text: about 25-35 pm for good Latin/CJK/Arabic/Hebrew support; 40-55 pm for broad world-script coverage.** Lower bound if one scopes to Latin + CJK + system fallback: about 12 pm.

**Pragmatic scope ladder (decision for the user):**
- Level 1: Latin/Cyrillic/Greek with kerning, one bundled OFL font. About 3 pm; enough for the first milestones.
- Level 2: + CJK (no shaping needed beyond fallback and vertical metrics), emoji. About +4 pm.
- Level 3: + bidi, Arabic, Hebrew. About +5 pm.
- Level 4: Indic, Thai, Khmer, Myanmar and other complex scripts via a full shaping engine. +10 pm or delegate to OS text services (DirectWrite/CoreText/Android/pango) on non-Linux platforms.

**Biggest risks.** Correctness for scripts the developer cannot read (need native-speaker test corpora), font bugs ("works in Noto, breaks in vendor font"), emoji, selection/caret in bidi, performance of shaping/layout for long documents (must cache), IME interplay. **A bundled font** also raises licensing and size questions (use SIL OFL fonts such as Inter, Noto).

### 2.5 Images and codecs

| Format | From scratch? | pm |
|---|---|---|
| PNG (inflate, filters, interlace, gamma/ICC chunks), APNG | Yes | 0.5-0.8 |
| JPEG (baseline + progressive, chroma upsampling, EXIF orientation, CMYK/ICC) | Yes | 1-1.5 |
| GIF (animated) | Yes | 0.3 |
| WebP lossless / lossy (VP8 key frames) | Yes | 1 / 2 |
| SVG (renderer for a static subset) | Possible, large | 3-4 |
| AVIF (AV1), HEIC (HEVC), JPEG XL | **No**: decoder effort 10+ pm each and patent/licence tangles. Use OS decoders (ImageIO, WIC, Android `ImageDecoder`) through FFI | 0 (+bindings) |
| Video | **No** (OS media frameworks) | - |
| Colour management | sRGB, Display P3, linear blending, ICC v2/v4 subset | 1 |

Total: **about 6-9 pm** (PNG + JPEG + GIF + WebP + colour), plus optional SVG.

### 2.6 Layout

Adopt **Flutter's box-constraint protocol** ("constraints go down, sizes go up, parent sets position"): single pass, O(n), simple, and enough for 95% of UIs. Implement a core set (flex row/column, stack, padding/align/sized-box, wrap, scroll "slivers", intrinsic sizing sparingly) rather than CSS or a general constraint solver. CSS-style block/inline/grid/float layout is a multi-year engine of its own; avoid it.

Effort: **3-4 pm** for the core plus **2-3 pm** for scrolling/slivers/virtualised lists. Risks: text-dependent sizing and intrinsic measurement cost; RTL mirroring; keeping the layout API small enough to be teachable.

### 2.7 Compositing and layers

Layer tree (transform, clip, opacity, backdrop filter, picture layers), repaint boundaries, retained display lists, raster cache, damage tracking and partial present (`wl_surface.damage_buffer`, `VK_KHR_incremental_present`/`EGL_KHR_partial_update`), offscreen pass management, and a compositor thread. **3-4 pm.** Risk: invalidation correctness and memory use of offscreen surfaces.

### 2.8 Animation and frame scheduling

Vsync source per OS (Wayland `frame` callback / `wp_presentation`, `CVDisplayLink` or `CAMetalDisplayLink`, DXGI waitable swapchain/DWM, Android Choreographer), frame pipeline (input, animation tick, build, layout, paint, raster, present), tickers, curves, spring physics, implicit animations, input event resampling, gesture arena (tap/drag/scale/long-press disambiguation), scroll physics per platform (iOS bounce vs Android clamp). **3-4 pm.** Needs threads (UI thread + raster thread), so the concurrency decision in SPEC.md must be taken first.

### 2.9 Widgets, state and reactivity

Two design choices matter for Yerux:
- **Retained element tree with rebuild + diff (Flutter/React)** vs **fine-grained signals (Solid/Leptos)**. With no GC and single-owner semantics, signals need shared mutable cells (an `Rc`-like escape hatch the current ownership model forbids), while rebuild+diff works with plain values but allocates a lot. This is a **language design decision** that must be taken with the ownership model, and it requires closures.
- The declarative API's ergonomics depend on named/default arguments, trailing closures or builder syntax, enums with payloads, generics. None exist yet.

| Part | pm |
|---|---|
| Core: element/render-object trees, build/rebuild scheduling, keys, state lifecycle, inherited data, focus, semantics tree | 6-10 |
| Widget library (about 100 widgets: buttons, text field, checkbox/radio/switch, slider, lists, grids, tabs, navigation, dialogs, menus, tooltips, scrollbars, date pickers, theming and dark mode, localisation, forms, drag and drop) | 15-25 |
| Material/Cupertino-like design systems (optional: ship one neutral design system first) | 3-6 each |
| Devtools (inspector, perf overlay, widget tree, layout explorer) | 3-5 |

### 2.10 Hot reload and what it asks of the compiler

Dart gets hot reload from a JIT VM. For an AOT compiler there are three viable designs:

1. **Hot restart via ultra-fast rebuild.** Our backend builds a hello world in about 3 ms (BACKEND.md); a whole framework app might build in under 1 s. Re-exec the process and restore state through serialisation. Cheapest, no JIT, works first. 1 pm once the app state is serialisable.
2. **Patchable code in dev builds.** Compile each module to a relocatable blob (needs the object format of BACKEND N5), `mmap` it executable, route all calls through an indirection table, keep the heap in the host process, and migrate state when struct layouts change. Needs: modules/incremental compilation, stable function identity, a layout-diff for structs, per-function relocation info. 3-5 pm. Because we write both the compiler and the loader we do not need `dlopen`.
3. **Bytecode/interpreter tier for UI code** in dev mode only. Largest cost (6+ pm) and duplicates semantics. Not recommended.

Platform constraint: iOS devices forbid creating executable pages, so on-device hot reload there would need design 1 over the network (rebuild and reinstall) or design 3. Desktop and Android are fine.

## 3. Where the time goes (summary)

| Area | pm (range) | Share | Time sink? |
|---|---|---|---|
| Platform: window, input, IME, a11y (5 native platforms) | 40-55 | 25-30% | **Yes** |
| Text stack (to broad coverage) | 25-45 | 20-25% | **Yes** |
| Widget core + library + devtools | 25-40 | 20% | Yes (long tail) |
| GPU abstraction + shader pipeline | 8-12 | 6% | Moderate |
| 2D rasterization (CPU + GPU) | 13-19 | 9% | Moderate |
| Images, codecs, colour | 6-9 | 4% | No |
| Layout, compositing, animation, gestures | 9-12 | 6% | No |
| Hot reload, tooling hooks | 4-6 | 3% | No |
| **Total (excl. compiler/stdlib)** | **~130-200** | | |

Linux + Windows only, Level 1-3 text, CPU raster plus Vulkan, 40 core widgets: about **45-60 pm**. That is the realistic "first product" target.

## 4. Realistic vs infeasible

**Realistic for a solo developer working for years (with AI assistance, staged):**
- Own compiler/backends, Wayland + X11 clients, software and Vulkan renderers, a TrueType/OpenType reader, shaping for Latin/CJK/Hebrew/Arabic, PNG/JPEG/GIF/WebP lossless, a Flutter-style layout and widget set, Windows via Win32.

**Realistic only with a team or by delegating to the OS:**
- Complete accessibility on every platform; Indic/complex-script shaping parity with HarfBuzz; IME for all languages on all platforms; macOS + iOS + Android in addition to desktop Linux/Windows; a native D3D12 path.

**Infeasible or not worth it from scratch:**
- GPU drivers; AV1/HEVC/JPEG XL decoders; video playback; TrueType hinting parity; a full Skia/Vello-class feature set; a browser-grade CSS engine; a web target with the same fidelity as native.

## 5. Recommended first milestone, with concrete steps

Milestone R: "window + coloured rectangle, then a line of text, then a button" on **Linux Wayland first, CPU pixels, no FFI**.

**R0 Window + rectangle (about 3-5 weeks)**
1. Syscalls needed (we emit raw syscalls today; add): `socket(AF_UNIX)`, `connect`, `sendmsg`/`recvmsg` with `SCM_RIGHTS`, `memfd_create`, `ftruncate`, `mmap`, `ppoll`/`epoll`, `close`. Compiler prerequisites: byte arrays/slices, `UInt8/UInt32`, bit operations, `mmap` allocator (N3).
2. Resolve the socket path from `XDG_RUNTIME_DIR` + `WAYLAND_DISPLAY` (default `wayland-0`); read the environment from the initial stack (`envp`).
3. Speak the wire protocol: 32-bit words in host order; header = object id, then `(size << 16) | opcode`. Hand-write the six or seven interfaces needed first (`wl_display`, `wl_registry`, `wl_compositor`, `wl_shm`/`wl_shm_pool`/`wl_buffer`, `wl_surface`, `xdg_wm_base`/`xdg_surface`/`xdg_toplevel`); later generate bindings from the protocol XML.
4. Create a `wl_shm_pool` from a `memfd` (fd passed with `SCM_RIGHTS`), make a `wl_buffer` (`XRGB8888`), fill pixels with a solid colour in our own memory, `attach` + `damage_buffer` + `commit`. Answer `xdg_wm_base.ping` with `pong`; handle `xdg_surface.configure` with `ack_configure`; handle `xdg_toplevel.close`.
5. Test headless in CI with Weston's headless backend (`weston --backend=headless`) and golden-image dumps of the buffer.
6. Window decorations: ask for server-side via `xdg-decoration`; if refused (GNOME), draw a minimal title bar ourselves (later).

**R1 A line of text (about 3-4 weeks)**
1. Bundle one OFL font (e.g. a Latin sans) in the binary; parse `head`, `maxp`, `hhea`, `hmtx`, `cmap` (format 4/12), `loca`, `glyf` (simple and composite).
2. Flatten quadratic Beziers and rasterize with an accumulation-buffer coverage algorithm (the `font-rs`/`stb_truetype` approach: signed-area accumulation, then prefix sum per scanline).
3. Glyph cache by (glyph, size, subpixel offset); advance widths from `hmtx`; kerning from `kern`/GPOS pair adjustment later.
4. Alpha-blend glyph coverage onto the buffer; draw one line of ASCII/Latin-1 text.

**R2 A button (about 3-4 weeks)**
1. Input: bind `wl_seat`; `wl_pointer` motion/button/axis; `wl_keyboard` with the keymap delivered as an fd (parse a minimal xkb subset, or use US-layout evdev tables first), `repeat_info`.
2. Frame loop on `wl_surface.frame` callbacks; redraw only on change.
3. Rounded-rectangle anti-aliased fill and border, a cheap drop shadow, centred label, hit-testing, hover and pressed states, `on_click` callback (needs closures or function values in the language).
4. Output: a "counter" app (a label and a button that increments). That is the first framework demo.

**The same milestone on other OSes (done after Linux, with FFI/PE/Mach-O work from FFI.md):**

| OS | Window | Pixels | Needs |
|---|---|---|---|
| Linux X11 | connect `/tmp/.X11-unix/X0`, auth from `~/.Xauthority`, `CreateWindow`, `PutImage` or MIT-SHM, XKB for keys | CPU buffer | Syscalls only. 2-3 weeks. |
| Windows | `kernel32`/`user32`/`gdi32` imports, `RegisterClassExW`, `CreateWindowExW`, `GetMessageW` loop, a `WndProc` callback thunk | `StretchDIBits` or `UpdateLayeredWindow` | PE writer, Win64 convention, callbacks. About 4-6 weeks plus F5. |
| macOS | ObjC runtime: `NSApplication`, `NSWindow`, a custom `NSView` subclass | set `CALayer.contents` from an `IOSurface`/`CGImage` | Mach-O writer, ad-hoc signing, ObjC bridge, arm64 backend. 6-8 weeks plus F6/F7. |
| Android | `ANativeActivity_onCreate` entry in an `.so`, `ANativeWindow_lock` | CPU buffer | `.so` writer, APK packager. 4-6 weeks plus F8. |
| iOS | UIKit via ObjC | `CALayer.contents` | Mach-O, signing, provisioning. 4-6 weeks. |

Only after the CPU path works on at least two OSes should the GPU renderer start.

## 6. Recommended order of work, interleaved with the compiler

The detailed schedule with milestone numbers is in `ROADMAP.md`. The dependency logic is:

1. **Backend N2 (register allocation) and N3 (aggregates, `view`/`mut`, allocator)** first: without them neither strings nor pixel buffers are practical.
2. **Language additions that the framework forces**, in this order: sized integers and byte slices, `String`, generics (`List<T>`, `Map<K,V>`), `Option/Result/?`, modules, closures/function values, trait-like interfaces with dynamic dispatch, threads/channels. These land during R0-R2, driven by the demo code.
3. **R0-R2 (Linux Wayland, CPU, text, button)**, with the stdlib (syscall wrappers, byte buffers, formatting).
4. **FFI F1-F2 and Vulkan** once the CPU path is solid; GPU renderer behind the existing display list.
5. **Second platform: Windows** (same ISA, PE writer), then **macOS** (needs aarch64 backend, Mach-O, signing), then Android/iOS.
6. **Text levels 2-3 (CJK, bidi, Arabic), IME, a11y** in parallel with the widget library.
7. **Hot restart (design 1)** early because it is cheap and pays for itself; **patchable hot reload (design 2)** after modules/object format exist.

## Sources

- Flutter docs, "Impeller rendering engine" (backends, platform status, ahead-of-time shaders): https://docs.flutter.dev/perf/impeller
- Linebender, "Vello" README (Vello CPU / GPU / compute renderer status): https://github.com/linebender/vello
- Linebender, "Parley" README (HarfRust, Fontique, Skrifa, ICU4X composition): https://github.com/linebender/parley
- Wayland Book, "Wire protocol basics": https://wayland-book.com/protocol-design/wire-protocol.html
- Go issue 36435 (macOS/OpenBSD use libc rather than raw syscalls): https://github.com/golang/go/issues/36435
- Microsoft, "x64 calling convention": https://learn.microsoft.com/en-us/cpp/build/x64-calling-convention?view=msvc-170
- Skia Graphite design page (fetch failed; Graphite description is from knowledge): https://skia.org/docs/dev/design/graphite/
- Effort figures are the author's judgement based on the scope of HarfBuzz, FreeType, Skia, Flutter and winit/GLFW-class projects; they are estimates, not measurements.
