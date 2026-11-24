# Windows target (x86-64, PE32+)

`yerux build prog.yx --target windows [-o prog.exe] [--native-debug-alloc] [--no-opt]` writes a console
`.exe` with no linker, no libc and no C runtime. `--target windows` implies `--native`; the default output
name gets `.exe`. `yerux run ... --target windows` runs the result through `wine` when it is not on Windows.

The IR -> x86-64 code generator is the same as on Linux (`src/x64.rs`). Only the OS boundary is
abstracted by `x64::Os::{Linux, Windows}`: write, exit, and map/unmap memory. The Linux output is
byte-identical to before the change (checked on all native-supported programs, normal and debug-alloc).

## What is generated

| Part | Content |
|---|---|
| Headers | DOS header + "cannot be run in DOS mode" stub, PE signature, COFF header (AMD64, `EXECUTABLE_IMAGE`, `LARGE_ADDRESS_AWARE`), PE32+ optional header, 16 data directories (import table, exception table, base relocations, IAT), subsystem console, 6.0 versions |
| `.text` | code (R+X) |
| `.rdata` | string/text constants (R) |
| `.data` | heap state: free lists and debug counters, zero-filled (R+W) |
| `.idata` | import directory, ILT, IAT, hint/name table for `KERNEL32.dll` (R+W) |
| `.pdata` | one `RUNTIME_FUNCTION` per generated function, sorted (R); exception directory |
| `.xdata` | `UNWIND_INFO` records (R) |
| `.reloc` | empty base relocation directory (R, discardable): the image is position independent |

Preferred image base `0x140000000` (ASLR may load it elsewhere), section alignment 0x1000, file alignment 0x200, timestamp 0 (reproducible output),
checksum 0, stack 8 MiB reserve **and** commit.

Imports (all from kernel32.dll): `GetStdHandle`, `WriteFile`, `ExitProcess`, `VirtualAlloc`, `VirtualFree`,
`SetConsoleOutputCP` (called once at start with 65001 so UTF-8 text prints correctly in a console).

## Runtime (replaces the raw Linux syscalls)

* Every Windows API call goes through four small stubs (`os_write`, `os_exit`, `os_alloc`, `os_free`) that follow the
  Microsoft x64 convention: arguments in rcx, rdx, r8, r9, 32 bytes of shadow space, and rsp realigned to 16
  (`and rsp,-16`) *inside the stub*, because the Yerux calling convention (arguments on the stack) does not keep
  rsp aligned at runtime call sites. Non-volatile rbx, rsi, rdi, rbp are untouched. The program's own functions
  keep the internal Yerux convention; only the OS boundary uses Win64.
* print int / bool / text: `GetStdHandle(-10 - fd)` + `WriteFile`. fd 2 (stderr) is used by panics and the debug report.
* Panic: `panic: <message>\n` on stderr, then `ExitProcess(101)`. Same messages and exit code as Linux.
* Allocator: the same size-class free lists + 1 MiB arenas as on Linux, with `mmap` replaced by
  `VirtualAlloc(NULL, n, MEM_COMMIT|MEM_RESERVE, PAGE_READWRITE)` (NULL -> "out of memory" panic) and `munmap` by
  `VirtualFree(p, 0, MEM_RELEASE)` for large blocks.
* `--native-debug-alloc` works unchanged (allocs / frees / live on stderr, double free detection).

## Verification (no Windows machine was available)

Everything below was run on Linux. **The result has not been run on real Windows.**

1. **Wine 9.0** (`apt install wine64`; the binary is `/usr/lib/wine/wine64`): `tests/wine_all.sh` builds every
   native-supported program in `examples/` and `tests/` with `--target windows`, runs it under wine and compares
   stdout and exit code with the same goldens as the Linux backends; also panic stderr + exit 101 and
   `--native-debug-alloc` (`live: 0`). Result: 40 pass, 0 fail, 7 skipped (programs the native backend does not
   support yet). A 50000-deep recursion and non-ASCII text were also checked by hand.
2. **`tests/pe_check.py`** (needs `pip install pefile capstone unicorn`):
   * structure with pefile (alignment, section order and permissions, entry point, import table, IAT directory, file size);
   * capstone linear disassembly of `.text` (no invalid opcodes, branch targets on instruction boundaries, rip-relative
     targets inside the image, every indirect call goes through an IAT slot);
   * emulation of the real `.exe` in Unicorn with a mini PE loader (W^X enforced) and kernel32 hooks that **enforce
     Win64**: rsp = 8 mod 16 at every API entry, volatile registers and the shadow space are overwritten on return,
     argument checks for `WriteFile` / `VirtualAlloc` / `VirtualFree`, no double release.
     Output and exit code are compared with the goldens; `--fuzz N` adds random programs compared with the Linux
     native build (stdout, stderr, exit code). Result: 38 goldens + 150 fuzz programs + debug-alloc pass, 0 fail
     (`02_fast`, a 100M-iteration loop, exceeds the instruction limit and is skipped there; it passes under wine).
   * Mutation check: removing the realignment from the stubs makes it fail with a message naming the API.
3. `llvm-objdump -x` and pefile parse the file without complaints.

Not verified / known gaps:

* Real Windows, one machine: Windows Server 2022 (build 20348) in a QEMU VM, driven by `tests/windows_vm_smoke.sh`
  (`~/vm-windows/run-vm.sh` boots it, `host.py` and the guest's `agent.ps1` pass jobs). Measured there with
  `tests/windows_vm_smoke.ps1`: 5 programs (`01_lists`, fib, sort, textops, hashmap) print exactly the C backend's
  output with exit code 0; every image has the ASLR flags; the loader relocated the image to `0x7FF6E88D0000` (it
  asks for `0x140000000`; Windows picks the bias once per boot, so one boot gives one base); and for every function of
  the 6 images `RtlLookupFunctionEntry` finds its entry and `RtlVirtualUnwind`, started after the prologue on a
  synthetic frame, returns the planted caller rip/rsp/rbp. Result: 0 failures. Not tested on other Windows versions.
* Other Windows versions: wine implements kernel32 but is not the Windows loader. Things only real Windows
  checks include: Windows Defender / SmartScreen reaction to an unsigned hand-made PE, the strictness of the loader
  on header fields, and behaviour under exploit protection.
* **ASLR and unwind information (done).** The image sets `DYNAMIC_BASE | HIGH_ENTROPY_VA | NX_COMPAT`. The code holds
  no absolute address: the heap state (`.data`), text literals, vtables and the IAT are reached with
  `lea r, [rip+disp]` / rip-relative operands, so the base relocation directory (`.reloc`) is present but empty (one
  block header) and the loader can rebase the image anywhere without patching code. `.pdata` has one
  `RUNTIME_FUNCTION` per generated function and `.xdata` the `UNWIND_INFO` (shared between functions with the same
  frame): a framed function (`push rbp; mov rbp, rsp; sub rsp, N`) has the codes ALLOC (small, or large with a 16- or
  32-bit size), SET_FPREG (rbp) and PUSH_NONVOL (rbp); a frameless one has none. Limits: the callee-saved registers
  the prologue stores with `mov` are not described (an unwinder recovers rip, rsp and rbp exactly, not rbx/r12-r15 of
  the caller); the runtime helpers (allocator, API stubs, panic routines) have no entry and are treated as leaf
  code; epilogues (`leave; ret`) are not in the unwinder's recognised epilogue forms, so an asynchronous unwind landing
  exactly there is wrong; shrink-wrapping is off on Windows (code before the prologue would be described by the
  prologue's unwind codes). Programs do not use SEH: panics still end the process.
  Verification: `tests/pe_layout.sh` (in `run_all.sh`; `tests/pe_check.py` checks flags, `.reloc`, sorted and
  in-range `.pdata`, well-formed `.xdata`, no `movabs` of an image address) and, on a real Windows Server 2022 VM,
  `tests/windows_vm_smoke.sh` / `tests/windows_vm_smoke.ps1` (see below).
* The stack is committed up front (8 MiB) instead of using the guard page, so large frames need no `__chkstk` probes.
  Deep recursion beyond 8 MiB crashes with an access violation instead of a clean message (Linux: SIGSEGV too).
* `WriteFile` is not retried on a short write (does not happen on console, files and pipes in synchronous mode).
  Output goes straight to the handle (no buffering), as on Linux. No check of `WriteFile` errors (closed pipe).
* Console: UTF-8 output depends on `SetConsoleOutputCP(65001)`; fonts and redirection to a file give raw UTF-8 bytes.
* Only the console subsystem; no GUI entry, no command line arguments, no environment, no files (same as Linux).
* The code is the unoptimised "every vreg in a stack slot" output (see BACKEND.md); no Windows-specific speed work.
* `aarch64` Windows and 32-bit x86 are out of scope.


## Running the compiler on Windows (host support)

The compiler itself is portable Rust with no OS-specific code besides POSIX file permissions, now behind
`#[cfg(unix)]`. **Verified:** `cargo check --tests --target x86_64-pc-windows-gnu` passes (CI job
`windows-check`). **Not verified on real Windows** (none was available): the behaviour below is implemented
and covered by the `windows` CI job (`.github/workflows/ci.yml`, `windows-latest`, Git Bash), which will be the
first real run.

| Topic | Behaviour |
|---|---|
| Native backend on a Windows host | `yerux build prog.yx --native` (and `run`) produce a Windows PE `.exe` (target defaults to `windows` when the host is Windows); the default output name gets `.exe` |
| C compiler | `$YERUX_CC`, else the first of `cc`, `gcc`, `clang`, `x86_64-w64-mingw32-gcc` that runs. Extra flags: `$YERUX_CC_FLAGS`. MSVC `cl` is not supported (the generated C uses POSIX threads/sockets; MinGW-w64 and clang provide them) |
| No C compiler | build/run fall back to the native backend with a note; `check`, `c`, `ir`, `fmt`, `lsp` need none |
| `yerux doctor` | reports the compiler found (or MISSING and why), wine, temp dir, platform, and compiles + runs a program with each backend; exit 1 only if no backend works |
| Paths and command lines | temp files use `std::env::temp_dir()`; file arguments may contain spaces and either slash; CRLF and BOM sources are accepted (checked by `tests/win_smoke.sh`) |
| Limits | the native Windows backend has the limits listed above (no extern C, OS intrinsics such as files, processes and sockets are rejected with `not supported on the windows target yet`); programs using them need the C backend (MinGW) or Linux |

`tests/win_smoke.sh` is host-agnostic (passes on Linux too: 33 programs): `--version`, `doctor`, `run` on a CRLF
source in a directory with spaces, the default output name, and every example plus `tests/fast.list` through
the native backend, compared with the goldens. `--all` covers every program with a golden.
