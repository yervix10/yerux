# Yerux FFI and dynamic linking

Status: experimental. `Ptr<T>` can be read, written and offset inside `unsafe` (`p.read()`, `p.write(v)`, `p.add(n)`, `p.sub(n)`, `p.offset_from(q)`, `p.addr()`, `Ptr<T>.from_addr(a)`; element types `Int`, `UInt64`, `UInt8`, `Ptr<..>`; null read/write panics; no bounds or alignment checks; no `*p` sugar or struct field access through a pointer yet), so C buffers can be filled and inspected without helper C code (`tests/ptr_rw.yx`). `extern "C" { fn ... }` declarations, `link "..."`, C scalar prototypes and direct calls work through the C backend. Linux native builds use eager ELF imports and pass scalar integers, pointers and floats through SysV on x86-64 and AAPCS64 on AArch64. An extern parameter declared as `Text` receives a pointer to NUL-terminated UTF-8 bytes for either a literal or a typed `Text` value. `Ptr<T>` is accepted as a raw C ABI parameter or result, and extern calls require a lexical `unsafe { ... }` block. The dynamic ELF writer maps code and read-only literals RX and loader metadata/GOT/runtime state RW in separate page-aligned `PT_LOAD`s, with a non-executable RW stack. Variadic declarations/calls (section 6.1) work on both native Linux targets and the C backend. AArch64 imports require the corresponding glibc loader and shared libraries. AArch64 executables with both C imports and Yerux workers use pthreads so workers have libc TLS. Windows PE supports a small [Universal CRT subset](WINDOWS.md#current-cross-build-audit-and-wine-verification). A first-class `CString` helper remains unimplemented.

## 0. Conclusions up front

1. **Linux can go a long way with no FFI at all.** Wayland and X11 are byte protocols over a Unix socket; shared pixel buffers come from `memfd_create` + `mmap` + `SCM_RIGHTS`. A window with software-rendered pixels needs only raw syscalls, which we already emit. FFI is mandatory on Linux only for the **GPU** (Mesa/NVIDIA drivers are user-space shared objects loaded through `libvulkan.so`/`libEGL.so`).
2. **macOS, iOS and Windows cannot be libc-free.** Their stable ABI is the system-library ABI, not the syscall ABI. Every windowed program there links dynamically. So a dynamic-linking-capable object writer (PE, Mach-O) is a hard requirement for any non-Linux platform; this is not optional polish.
3. The minimal Linux dynamic ELF is **small** (about 4.2 KB file, 4 program headers, 9 dynamic entries, no PLT, no hash table, no section headers). Section 9 gives the exact bytes and `docs/examples/mk_dyn_hello.py` generates it; it was run on glibc here and prints `hello`.
4. Recommended shape: **one DT_NEEDED (`libc.so.6`)** for `dlopen`/`dlsym`, everything else (Vulkan, Wayland client libs if ever wanted, libxkbcommon...) loaded lazily at run time so a missing GPU library degrades to the software path instead of failing at process start.
5. Safety: two independent gates. `unsafe` (memory soundness, local, lexical) and a `ffi` capability (authority, per package, fits the planned v0.5 effects system).

## 1. ELF dynamic linking basics

A dynamically linked executable is a normal ELF plus: a request for an interpreter (the dynamic loader, `ld.so`), and a `.dynamic` table that tells the loader what to load and patch. The kernel only maps the `PT_LOAD` segments; if `PT_INTERP` exists it also maps the interpreter and jumps to *its* entry point. `ld.so` then maps the needed libraries, resolves symbols, applies relocations, runs library initialisers (this is when libc initialises itself), and finally jumps to our `e_entry`.

| Piece | What it is |
|---|---|
| `PT_INTERP` | NUL-terminated path, `/lib64/ld-linux-x86-64.so.2` on glibc x86-64 (musl: `/lib/ld-musl-x86_64.so.1`; Android: `/system/bin/linker64`). Hard-coded path: a portability cost on NixOS/Guix. |
| `PT_DYNAMIC` | Points at the `.dynamic` array of `(tag, value)` pairs. |
| `DT_NEEDED` | Offset into `DT_STRTAB` of a library SONAME to load (`libc.so.6`). One entry per library. |
| `DT_SYMTAB`, `DT_STRTAB`, `DT_STRSZ`, `DT_SYMENT` | Dynamic symbol table (24-byte `Elf64_Sym`) and string table. Imported symbols are entries with `st_shndx = UND (0)`, `STB_GLOBAL`, `STT_FUNC` (`st_info = 0x12`). |
| `DT_RELA`, `DT_RELASZ`, `DT_RELAENT` | Eager relocations, 24-byte `Elf64_Rela {r_offset, r_info = sym<<32 | type, r_addend}`. |
| `DT_JMPREL`, `DT_PLTRELSZ`, `DT_PLTREL`, `DT_PLTGOT` | Lazy-binding relocations (`.rela.plt`) and the GOT base used by PLT stubs. |
| `DT_HASH` or `DT_GNU_HASH` | Symbol lookup acceleration for symbols *this* object **exports**. A pure executable that exports nothing does not need one (verified in section 9). Shared objects we produce (Android) do need one. |
| `DT_FLAGS` / `DT_FLAGS_1` (`BIND_NOW`, `PIE`) | Force eager binding; mark PIE. |

**Relocation types needed on x86-64**

| Type | Value | Use |
|---|---|---|
| `R_X86_64_GLOB_DAT` | 6 | Fill a GOT slot with the address of a symbol. This is all the minimal example needs. |
| `R_X86_64_JUMP_SLOT` | 7 | Same, but for `.rela.plt` and lazy binding. |
| `R_X86_64_64` | 1 | Absolute 64-bit address (e.g. function-pointer tables in data). |
| `R_X86_64_RELATIVE` | 8 | `base + addend`; needed once we emit PIE/ET_DYN (ASLR) or shared objects. |
| `R_X86_64_TPOFF64`, `DTPMOD64`, `DTPOFF64` | 18, 16, 17 | Thread-local storage. Avoid until threads need TLS; keep thread state in a register/stack instead. |

**Symbol lookup.** `ld.so` searches the global scope in load order (executable, then its DT_NEEDED libraries breadth-first); first definition wins. Versioned symbols (`DT_VERSYM`/`DT_VERNEED`, e.g. `memcpy@GLIBC_2.14`) are optional for imports: an unversioned reference binds to the default version, which is what we want.

**Lazy vs eager.** Lazy binding uses a PLT stub that jumps through a GOT slot initially pointing back into the PLT, which calls `_dl_runtime_resolve`. It saves startup time for programs with thousands of imports and costs: a PLT generator, `DT_PLTGOT` with three reserved GOT entries, `DT_JMPREL`. Eager binding (`GLOB_DAT` + indirect `call [rip+got]`, what `-fno-plt -z now` does) is simpler, is the security-hardened default on many distributions, and makes every import a single indirect call. **Recommendation: eager only.** We import tens of symbols, not thousands; lazy PLT is a size/startup optimisation to revisit never or last.

**Minimal emission checklist for "call libc/libvulkan on Linux x86-64"**

1. ELF header, program headers: `PT_INTERP`, `PT_LOAD` (R+X, covers headers+code+rodata), `PT_LOAD` (RW: `.dynamic` + GOT), `PT_DYNAMIC`. Optionally `PT_GNU_STACK` (flags RW) so the stack stays non-executable (absent => some kernels assume exec stack).
2. `.interp` string; `.dynstr` (library names + symbol names); `.dynsym` (null symbol + one UND symbol per import).
3. `.rela.dyn` with one `GLOB_DAT` per import, pointing at a GOT slot.
4. `.dynamic`: `NEEDED`..., `STRTAB`, `SYMTAB`, `STRSZ`, `SYMENT`, `RELA`, `RELASZ`, `RELAENT`, `NULL`.
5. Code calls `call qword [rip+disp32]` (`FF 15 disp32`) targeting the GOT slot.
6. **No section headers required.** (`e_shnum = 0` is fine for execution; they are only needed for debuggers/`strip` tooling, add later with DWARF.)
7. Entry point is still our own `_start` (no crt1.o). ld.so runs libc's initialisers before jumping to it. At entry `rsp` is 16-byte aligned and points at `argc`; `argv`, `envp`, auxv follow on the stack, same as the static case.
8. For `libvulkan`: either list `libvulkan.so.1` as a second `DT_NEEDED` (hard dependency) or call `dlopen("libvulkan.so.1", RTLD_NOW)` + `dlsym("vkGetInstanceProcAddr")` through libc (soft dependency; **preferred**). A static, `ld.so`-less binary **cannot** `dlopen`: there is no loader in the process. Writing our own ELF loader that can host glibc/Mesa is not realistic (glibc relies on `ld.so` internals, TLS setup and IRELATIVE ifuncs; Mesa pulls in LLVM and libdrm).

Two build modes therefore coexist: `--static` (today's output: syscalls only, the CPU-rendering and server/CLI default) and `--dynamic` (adds `PT_INTERP` and imports). Same IR, same codegen; only the object writer and the runtime shim differ.

**Is libc itself needed?** For Vulkan on Linux we need `ld.so` and `dlopen`; in practice that means glibc (or musl, which has a working `dlopen` only when dynamically linked). Everything else we keep calling by raw syscall. This is the same compromise Go made on macOS/OpenBSD/Solaris. Source: golang/go#36435.

## 2. Calling conventions and what they do to codegen

| | System V AMD64 (Linux, macOS x86-64, BSD) | Win64 | AAPCS64 (Linux/Android arm64) |
|---|---|---|---|
| Integer/pointer args | `rdi rsi rdx rcx r8 r9` | `rcx rdx r8 r9` | `x0-x7` |
| Float args | `xmm0-xmm7` (independent counter) | `xmm0-3` (same position as int regs) | `v0-v7` |
| Stack args | right to left, 8-byte slots | right to left, 8-byte slots, **32-byte shadow space always reserved by caller** | 8-byte slots (Apple: packed by natural alignment) |
| Return | `rax`(+`rdx`), `xmm0`(+`xmm1`) | `rax`, `xmm0` | `x0`(+`x1`), `v0..v3` for HFA |
| Callee-saved | `rbx rbp r12-r15` | `rbx rbp rdi rsi rsp r12-r15 xmm6-xmm15` | `x19-x28`, `x29`(fp), `x30`(lr), low 64 bits of `v8-v15` |
| Stack alignment at `call` | 16 bytes (`rsp%16==0` before the call instruction) | 16 bytes | 16 bytes at all times (`sp`) |
| Varargs | `al` = number of vector registers used | float args duplicated in int regs | Linux: like normal; **Apple: all variadic args on the stack** |
| Struct by value | classify each 8-byte chunk: INTEGER/SSE; **> 16 bytes (or unaligned) => MEMORY** copied on the stack; return via hidden pointer in `rdi` (returned in `rax`) | only sizes 1,2,4,8 in a register; **everything else by reference** to a caller-made copy; hidden return pointer in `rcx` | <= 16 bytes in `x` regs; HFA (up to 4 identical floats) in `v` regs; > 16 bytes by reference (caller copy); hidden return pointer in `x8` |
| Reserved/other | red zone 128 bytes below `rsp` (leaf only; signal safe) | none; **stack probes** needed for frames > 4 KB (`__chkstk`) | `x18` reserved on Apple and Windows ARM; `x16/x17` are linker scratch (PLT/veneers) |

Sources for the Win64 column: Microsoft's x64 calling convention page. SysV and AAPCS64 columns are from the psABI documents and Apple's arm64 notes (author knowledge, not re-fetched in this session; the SysV psABI PDF host refused the fetch).

**Consequences for the Yerux backend**

- **Register allocation (N2).** Model every call as clobbering the caller-saved set of the *callee's* convention. Today every vreg lives in memory so this is trivially safe; it becomes a real constraint the day linear scan lands. Design the allocator with a per-call-site `clobber_mask` from the start.
- **Frame layout.** Compute the frame so `rsp` is 16-aligned at every `call`; today that is only coincidentally right for our own runtime routines, which is the first bug FFI will expose (SSE `movaps` inside libc crashes on a misaligned stack, with a confusing SIGSEGV).
- **Internal convention.** Use the SysV registers for our *own* functions too (cheap, makes extern calls and callbacks need no marshalling). On Win64, either use Win64 everywhere or generate a small thunk at each extern boundary. Do not invent a third convention until the allocator exists.
- **Callee-saved registers.** If our code uses `rbx`, `r12-r15` it must save and restore them whenever it can be called from C. Our `_start` never returns (it ends in `exit`), so this does not bite until **callbacks** (C code calling into Yerux, e.g. a Wayland listener or `WndProc`) exist, which require the full callee-saved discipline.
- **Floats.** Today `Int`/`Bool` only. FFI needs `Float32/64` with SSE2 encodings, xmm allocation classes, and `cvt*` instructions. This is the biggest compiler prerequisite, larger than the ELF part.
- **Sized integers.** A C `int` is 32 bits; passing `Int` (64-bit) in `edi` works for arguments (upper bits ignored) but **return values** need explicit sign/zero extension of `eax`. Add `Int8/16/32`, `UInt8/16/32/64` (and `USize`) before FFI.
- **Struct by value.** Implement SysV classification once (about 150 lines), restrict v1 to structs <= 16 bytes or pass-by-pointer; Vulkan, Wayland and Cocoa mostly use pointers to structs (but `CGRect`/`NSPoint` are by value in Cocoa).
- **Stack probing.** Needed on Windows (guard-page protocol) and advisable on Linux for large frames once we have stack arrays.

## 3. Windows: PE and imports

- Format: DOS stub (`MZ`, `e_lfanew`), `PE\0\0`, COFF header (`Machine = 0x8664`), PE32+ optional header (`Magic = 0x20B`), sections (`.text`, `.rdata`, `.data`, `.idata`, optionally `.pdata` and `.reloc`), `SectionAlignment 0x1000`, `FileAlignment 0x200`, `Subsystem` 2 (GUI) or 3 (console).
- **Import directory** (data directory index 1): array of `IMAGE_IMPORT_DESCRIPTOR {OriginalFirstThunk (ILT), TimeDateStamp, ForwarderChain, Name (RVA of "user32.dll"), FirstThunk (IAT)}`, null-terminated. ILT/IAT entries are RVAs of `{u16 hint, name\0}` (or ordinals). The loader overwrites the IAT with real addresses; code does `call [rip+iat_slot]`. This is structurally the same as our `GLOB_DAT` + GOT scheme, which makes the PE writer a straightforward second back end (about 400 lines for the writer).
- Run-time loading: import `LoadLibraryW`/`GetProcAddress` from `kernel32.dll` (the only DLL every process has), then load `vulkan-1.dll` optionally.
- ASLR: set `DYNAMIC_BASE` and emit `.reloc`, or clear the flag and use a fixed `ImageBase`. Fixed is fine for a first version; modern Windows still loads it.
- x64 unwinding: functions that are not leaf need `.pdata` `RUNTIME_FUNCTION` + `UNWIND_INFO` so exceptions, debuggers and some Windows internals can walk our stack. Required once we have callbacks through system code (window procedures) and for crash dumps.
- **Libc-free viability: no, with a twist.** `kernel32.dll` is the stable boundary (it exports `ExitProcess`, `WriteFile`, `VirtualAlloc`, `CreateThread`, `LoadLibrary`...) and there is no CRT requirement: a Win64 program can avoid the C runtime entirely. Raw `syscall` instructions into the kernel are unsupported: syscall numbers change between Windows builds. So Windows is "libc-free, **not** kernel32-free". This is easier than macOS.

## 4. macOS and iOS: Mach-O and dyld

- Format: `mach_header_64`, load commands, segments `__PAGEZERO`, `__TEXT` (`__text`, `__stubs`, `__cstring`), `__DATA_CONST` (`__got`), `__DATA`, `__LINKEDIT`.
- Required load commands for a runnable dynamic executable: `LC_SEGMENT_64`s, `LC_MAIN` (entry offset; dyld calls it like `main`), `LC_LOAD_DYLINKER` (`/usr/lib/dyld`), `LC_LOAD_DYLIB` for `/usr/lib/libSystem.B.dylib` (always) plus frameworks (`AppKit`, `Metal`, `QuartzCore`, ...), `LC_SYMTAB`/`LC_DYSYMTAB`, `LC_UUID`, `LC_BUILD_VERSION`, `LC_DYLD_CHAINED_FIXUPS` and `LC_DYLD_EXPORTS_TRIE` (modern; older `LC_DYLD_INFO_ONLY` with bind opcodes still works), and **`LC_CODE_SIGNATURE`**.
- **Code signing is mandatory on Apple Silicon**: the kernel kills unsigned arm64 binaries; an ad-hoc signature (a hash-only `CS_SuperBlob`) is enough for local runs. Our writer must produce a code directory (SHA-256 per 4 KB/16 KB page). The format is documented in XNU headers; open-source tools (e.g. `rcodesign`) show it can be done without Xcode. Distribution needs a paid Developer ID and notarisation; iOS needs provisioning profiles and Apple's signing even for device testing.
- Page size is 16 KB on arm64, 4 KB on x86-64. Segments must be page-aligned accordingly.
- **Libc-free viability: no.** Apple documents `libSystem` as the only stable kernel interface; syscall numbers and conventions are private and have changed. Static executables are unsupported (no `crt0`, no kernel support for fully static binaries). Go moved macOS to libSystem for this reason (golang/go#36435). On iOS, additionally: no JIT/executable-page creation for apps in the store (affects hot reload), no `fork`.
- Frameworks are Objective-C/Swift APIs. Calling Cocoa/UIKit/Metal from Yerux requires an **Objective-C runtime bridge**: `objc_getClass`, `sel_registerName`, and `objc_msgSend` (call it by casting to the right function pointer type; on x86-64 struct returns use `objc_msgSend_stret`, on arm64 there is no stret). Defining a subclass of `NSView` or an app delegate needs `objc_allocateClassPair` + `class_addMethod` + callback thunks. Metal is an ObjC protocol API (no C ABI), so it goes through this bridge too. Estimated cost: 1.5-2 person-months for a typed bridge generator plus a hand-written subset of AppKit/Metal bindings.
- dyld shared cache: since macOS 11 system dylibs are not files on disk; linking still works by install name, which is all we need, but a tool that wants to *read* symbols from system libraries must use the cache. We never need to.

## 5. Android and iOS in one paragraph each

- **Android.** Raw syscalls work (seccomp allow-list) but there is no UI path without Java. Output must be **shared objects** (`.so`, `ET_DYN`, PIE-style `RELATIVE` relocs, `DT_SONAME`, `DT_HASH`/`DT_GNU_HASH` for the exported entry symbol such as `ANativeActivity_onCreate`) packaged in an APK (zip, 4/16 KB alignment, v2+ signature). The linker is Bionic's `/system/bin/linker64`; libraries: `libandroid.so`, `liblog.so`, `libvulkan.so`, `libEGL.so`. `NativeActivity` + `ANativeWindow_lock` gives a CPU pixel buffer with no GL. IME and accessibility need JNI (`InputConnection`, `AccessibilityNodeProvider`), which means a JNI binding layer (function table of ~230 entries).
- **iOS.** Same as macOS Mach-O but arm64 only, with `UIKit`, `Metal`, `CoreText` available; signing and provisioning are mandatory; no JIT. Requires the aarch64 backend (BACKEND.md N4).

## 6. Proposed Yerux syntax

Today `Int` is the only integer. New primitives first: `Int8 Int16 Int32 Int64 UInt8 UInt16 UInt32 UInt64 USize Float32 Float64 CInt CChar` aliases, `Ptr<T>`, `RawFn(...) -> ...` function pointer type, and `#[repr(C)]`-style `struct` layout attribute (`@c struct`).

```
// Declaration block: where the symbols live, and how they are loaded.
extern "C" from linux "libc.so.6", windows "kernel32.dll", macos "libSystem.B.dylib" {
    fn write(fd: CInt, buf: Ptr<UInt8>, count: USize) -> Int   // returns ssize_t
    fn exit(code: CInt) -> Never
}

// Optional library, loaded at run time (dlopen / LoadLibrary), may fail.
extern "C" optional from linux "libvulkan.so.1", windows "vulkan-1.dll" {
    fn vkGetInstanceProcAddr(instance: Ptr<Void>, name: Ptr<CChar>) -> RawFn
}

// A C layout struct for FFI.
@c struct Rect { x: Float32, y: Float32, w: Float32, h: Float32 }

// Callbacks into Yerux: gets a C-ABI thunk.
extern "C" fn on_ping(data: Ptr<Void>, serial: UInt32) { ... }
let cb: RawFn = on_ping            // taking the address of an extern "C" fn

// Calling:
unsafe { write(1, msg.as_ptr(), msg.len()) }
```

Design points:

- **`from <os> "lib"`** replaces linker flags; the compiler records DT_NEEDED / import entries / `LC_LOAD_DYLIB` per target from this one place. `optional` switches from link-time to run-time resolution and makes lookups return `Option`-like failure (`extern lib handle.is_loaded()`).
- **Names are C names** by default; `@link_name("vkCreateInstance")` renames when a Yerux-style name is wanted.
- **`Ptr<T>`** is the only pointer type in the language. It has no borrow semantics, can be null, can be cast, and its dereference exists only inside `unsafe`. It cannot be stored where the ownership model would require a drop (a `Ptr` has no drop glue). This keeps rule 3 (borrows are second-class, no lifetimes) intact: safe code never sees a pointer; `view`/`mut` parameters lower to `Ptr` only at the call into `extern`.
- **Ownership across the boundary:** passing `view T` / `mut T` of a `@c struct` to an extern function lends a pointer for the call only. `move`-ing a Yerux value into C is only allowed through an explicit `Handle` wrapper whose drop calls the library's destroy function (e.g. `vkDestroyDevice`), written by the binding author with `unsafe`.
- **Strings:** A proposed `CStr` type (literal `c"..."`) and an allocating `to_cstr` would cover owned C string buffers.
- **Errors:** extern calls return raw codes. Safe wrappers convert `VkResult` etc. into `Result<T, E>` (v0.4).

### 6.1 Variadic calls (implemented)

```
extern "C" {
    fn snprintf(buf: Ptr<UInt8>, n: UInt64, fmt: Text, ...) -> Int32
    fn printf(fmt: Text, ...) -> Int32
}
```

- `...` must be the last entry of an `extern "C"` declaration and needs at least one fixed parameter. It is rejected anywhere else.
- Fixed parameters are checked like any extern parameter. Each extra argument must be an integer type, `Bool`, `Char`, `Float`, `Float32`, `Text` or `Ptr<T>`; aggregates, `String`, lists and borrowed values are an error. `Float32` is promoted to `Float` (a C `float` never travels through `...`); narrower integers are passed as full 64-bit words, which satisfies the C default promotions for `%d`/`%u`/`%c`. Use `%d` for `Int32`. For 64-bit values, match the modifier to the C type and platform. Windows C `long` is 32 bits.
- C backend: the prototype ends in `, ...` and is bound to the C symbol with an asm label (`nx_x_<name>`), so a Yerux prototype with `Ptr<UInt8>` never conflicts with a libc header.
- Linux x86-64: SysV. Integers go to `rdi rsi rdx rcx r8 r9`, doubles to `xmm0-7`, the rest to the stack in 8-byte slots (reserved in a multiple of 16 so `rsp` is aligned at the call), and `al` is set to the number of vector registers used (at most 8) before every extern call. Every function containing an extern call keeps a frame so the stack is 16-byte aligned.
- Linux AArch64 uses AAPCS64. Windows PE supports variadic `snprintf` through the Universal CRT. Win64 places the first four arguments in positional registers with home slots and duplicates floating-point varargs in the associated integer registers. Later arguments occupy eight-byte stack slots.
- Tests: `tests/ffi_variadic.yx` (snprintf/printf, output compared native vs C backend), `tests/e50-e53_ffi_variadic_*.yx` (errors), `tests/ffi_native.sh` (custom `va_arg` library, register/stack split).

## 7. Safety model

Two orthogonal checks; both are cheap to implement because Yerux already has a checker pass.

1. **`unsafe` block** (memory safety): required to call any `extern` function, dereference `Ptr`, cast integers to pointers, or transmute. Everything else keeps the language's no-UB promise. A package may say `#![forbid(unsafe)]`. The point: unsafe code is a short audited layer (`yerux.vulkan`, `yerux.wayland`), the UI framework above it is safe.
2. **`ffi` capability** (authority; aligns with SPEC v0.5 effects): a package can only declare `extern` blocks or call `unsafe` if its manifest grants the `ffi` capability *and* the root application allows it. Third-party widgets then cannot silently `dlopen` or issue syscalls. Syscall-level capabilities (`net`, `fs`) subdivide this later (`write` to fd 1 is an `io` effect, `socket` is `net`).

A fully memory-safe FFI is not possible (C is not checkable); the honest guarantee is "all unsafety is lexically marked and capability-gated".

## 8. Staged plan and effort

Estimates are person-weeks for one experienced compiler engineer who already wrote `x64.rs`; elapsed time is 1.3-2x with learning and testing.

| Stage | Deliverable | Depends on | Effort |
|---|---|---|---|
| F0 | `Int32/UInt8/...`, `Ptr<T>`, `unsafe`, `Float64` + SSE2, call lowering for > 6 args and float args, 16-byte stack alignment audit | N2/N3 | 5-7 wk |
| F1 | Dynamic ELF writer (section 9), `extern` parsing + checking, indirect call via GOT; `write` and `exit` from libc | F0 | 2-3 wk |
| F2 | `dlopen`/`dlsym` wrapper, `optional` libs, function-pointer tables; first Vulkan loader call (`vkEnumerateInstanceVersion`) | F1 | 1-2 wk |
| F3 | Callbacks (C-ABI thunks, callee-saved save/restore), SysV struct classification for <= 16 byte structs | F1, allocator | 2-3 wk |
| F4 | PIE/`ET_DYN` + `RELATIVE`, `DT_GNU_HASH`, `DT_SONAME`, shared-object output, `PT_GNU_STACK`, section headers + DWARF stubs | F1 | 3-4 wk |
| F5 | Windows PE writer: imports, `.pdata`, Win64 convention + shadow space + stack probes, `LoadLibrary` | F0-F3 | 5-7 wk |
| F6 | aarch64 backend + AAPCS64 (own work item N4; dominant cost) | N2/N3 | 20-26 wk |
| F7 | Mach-O writer (x86-64 and arm64): load commands, chained fixups, ad-hoc code signature; ObjC bridge (`objc_msgSend`, class definition) | F5-style, F6 | 10-14 wk |
| F8 | Android `.so` + APK packager + JNI table; iOS bundle + signing | F4, F6, F7 | 10-14 wk |
| F9 | Binding generator from C headers/Vulkan `vk.xml`/Wayland XML (generates `extern` blocks) | F1 | 3-4 wk (Vulkan registry parsing is easy; C header parsing is not, hand-write what is needed) |

Linux x86-64 (F0-F4): **about 3 person-months**. All platforms: about **1.2-1.6 person-years**, dominated by the aarch64 backend and Apple signing/ObjC.

## 9. Worked example: dynamic `write` hello world (verified)

Generator: `docs/examples/mk_dyn_hello.py`. Run: `python3 docs/examples/mk_dyn_hello.py hello && chmod +x hello && ./hello` prints `hello`, exit status 0 (glibc, x86-64, kernel as of this session). `readelf -lW` and `readelf -d` accept the file.

Layout (non-PIE `ET_EXEC`, `ImageBase = 0x400000`, file size 0x10a0):

```
file off  vaddr     content
0x0000    0x400000  Elf64_Ehdr (64 B): e_type=2 (EXEC), e_machine=62, e_entry=0x4001d6,
                    e_phoff=64, e_phentsize=56, e_phnum=4, e_shnum=0
0x0040    0x400040  4 x Elf64_Phdr (224 B)
                      [0] PT_INTERP  off 0x120  vaddr 0x400120 size 0x1c   flags R
                      [1] PT_LOAD    off 0      vaddr 0x400000 size 0x1000 flags R+X  align 0x1000
                      [2] PT_LOAD    off 0x1000 vaddr 0x401000 size 0xa0   flags RW   align 0x1000
                      [3] PT_DYNAMIC off 0x1000 vaddr 0x401000 size 0x90   flags RW   align 8
0x0120    0x400120  "/lib64/ld-linux-x86-64.so.2\0"
0x0140    0x400140  .dynsym: 3 x 24 B  [0]=null
                      [1] st_name=11 ("write") st_info=0x12 (GLOBAL FUNC) st_shndx=0 (UND)
                      [2] st_name=17 ("exit")  st_info=0x12             st_shndx=0
0x0188    0x400188  .dynstr (22 B): "\0libc.so.6\0write\0exit\0"
0x01a0    0x4001a0  .rela.dyn: 2 x 24 B
                      {r_offset=0x401090, r_info=(1<<32)|6, r_addend=0}   // GLOB_DAT write
                      {r_offset=0x401098, r_info=(2<<32)|6, r_addend=0}   // GLOB_DAT exit
0x01d0    0x4001d0  "hello\n"
0x01d6    0x4001d6  code (_start), 36 bytes:
                      bf 01 00 00 00                   mov  edi, 1
                      48 be d0 01 40 00 00 00 00 00    mov  rsi, 0x4001d0
                      ba 06 00 00 00                   mov  edx, 6
                      ff 15 a0 0e 00 00                call qword [rip+0xea0]   ; -> GOT 0x401090 (write)
                      31 ff                            xor  edi, edi
                      ff 15 a0 0e 00 00                call qword [rip+0xea0]   ; -> GOT 0x401098 (exit)
                      f4                               hlt
0x1000    0x401000  .dynamic (9 x 16 B):
                      DT_NEEDED   1 (strtab off of "libc.so.6")
                      DT_STRTAB   0x400188      DT_SYMTAB  0x400140
                      DT_STRSZ    22            DT_SYMENT  24
                      DT_RELA     0x4001a0      DT_RELASZ  48     DT_RELAENT 24
                      DT_NULL
0x1090    0x401090  GOT[0] (write), GOT[1] (exit): 8 bytes each, filled by ld.so
```

Notes on what the example demonstrates and where it is deliberately minimal:

- At `_start`, `rsp % 16 == 0`, so the `call` instructions satisfy SysV alignment with no adjustment. Any real prologue that pushes must re-establish it.
- `exit` (libc) rather than the `exit_group` syscall so libc flushes stdio; `write` bypasses stdio buffers anyway.
- Eager binding only: `GLOB_DAT` is processed by `ld.so` before our code runs; no PLT, no `DT_JMPREL`, no `DT_PLTGOT`.
- No `PT_GNU_STACK`, `DT_HASH`, `DT_FLAGS`, section headers or `DT_VERNEED`. It works on glibc; adding `PT_GNU_STACK` (RW) is recommended hygiene so the stack is not executable, and section headers are needed for `strip`, `gdb` symbol display and `objdump -d` convenience.
- Non-PIE: addresses are absolute. Moving to `ET_DYN` replaces `mov rsi, imm64` with `lea rsi, [rip+disp]` and requires `R_X86_64_RELATIVE` for any pointer stored in data.

## 10. Recommendation summary

1. Keep the static no-libc target as the default and as the reference path.
2. Do the milestone "window with software pixels" over Wayland sockets with **no FFI** (see RENDERING.md), in parallel with F0-F1.
3. Add dynamic ELF (F1-F2) when the GPU work starts; load `libvulkan` optionally.
4. Plan Windows (PE) before macOS: same x86-64 backend, import tables are structurally what we already emit, no mandatory signing.
5. Treat aarch64 + Mach-O + signing + ObjC bridge as one large "Apple/mobile" work package; do not start it before the framework is useful on Linux.

## Sources

- Go issue 36435, "runtime: use libc on OpenBSD" (also states macOS uses libc): https://github.com/golang/go/issues/36435
- Microsoft, "x64 calling convention": https://learn.microsoft.com/en-us/cpp/build/x64-calling-convention?view=msvc-170
- Wayland Book, "Wire protocol basics" (Unix socket, fd passing): https://wayland-book.com/protocol-design/wire-protocol.html
- System V AMD64 psABI (not fetched: host blocked automated access; referenced from knowledge): https://refspecs.linuxbase.org/elf/x86_64-abi-0.99.pdf
- Apple, ARM64 function calling conventions; Microsoft PE format; ELF gABI: referenced from knowledge, not re-fetched in this session.
- Empirical: `docs/examples/mk_dyn_hello.py` run against glibc ld.so in the research sandbox.
