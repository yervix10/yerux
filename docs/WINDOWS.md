# Windows target (x86-64, PE32+)

`yerux build prog.yx --target windows [-o prog.exe] [--native-debug-alloc] [--no-opt]` writes a console
`.exe` with no linker, no libc and no C runtime. `--target windows` implies `--native`; the default output
name gets `.exe`. `yerux run ... --target windows` runs the result through `wine` when it is not on Windows.

The IR -> x86-64 code generator is the same as on Linux (`src/x64.rs`). Only the OS boundary is
abstracted by `x64::Os::{Linux, Windows}`. The compiler checks reachable code for PE support,
so a module import can include platform-specific functions that the program never calls.

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

Imports from `KERNEL32.dll` include `GetStdHandle`, `ReadFile`, `WriteFile`, `GetLastError`,
`ExitProcess`, `GetCurrentProcessId`, `VirtualAlloc`, `VirtualFree`, and `SetConsoleOutputCP`
(the startup code requests UTF-8 output).

## Runtime (replaces the raw Linux syscalls)

* Startup and the runtime stubs call Windows APIs using the Microsoft x64 convention: arguments in rcx, rdx, r8, r9, 32 bytes of shadow space, and rsp realigned to 16
  (`and rsp,-16`) *inside the stub*, because the Yerux calling convention (arguments on the stack) does not keep
  rsp aligned at runtime call sites. Non-volatile rbx, rsi, rdi, rbp are untouched. The program's own functions
  keep the internal Yerux convention; only the OS boundary uses Win64.
* Console I/O uses `GetStdHandle(-10 - fd)` with `ReadFile` for stdin and `WriteFile` for stdout
  and stderr. The stubs return the transfer count and translate common errors into the codes
  expected by `std.io`. A closed input pipe returns EOF. This covers line and byte reads,
  prompts, raw standard output, and explicit process exit.
* Panics print the message and, when the compiler emits source locations, the source position to
  stderr before calling `ExitProcess(101)`.
* Allocator: the same size-class free lists + 1 MiB arenas as on Linux, with `mmap` replaced by
  `VirtualAlloc(NULL, n, MEM_COMMIT|MEM_RESERVE, PAGE_READWRITE)` (NULL -> "out of memory" panic) and `munmap` by
  `VirtualFree(p, 0, MEM_RELEASE)` for large blocks.
* `--native-debug-alloc` works unchanged (allocs / frees / live on stderr, double free detection).

## Corpus on Windows Server 2022

`python3 tests/windows_vm_corpus.py` cross-builds the top-level `tests/*.yx` that have
both `.out` and `.code` goldens and runs the PE binaries on Windows Server 2022 x86-64
(build 20348) in QEMU. It compares redirected stdout byte for byte, the exit status,
and stderr for tests with a `.stderr` golden. It feeds the same `.stdin` and `.args`
fixtures as the Linux suite. The driver lists the 12 excluded POSIX-specific goldens
and counts compiler rejections among the 402 eligible tests.

| Windows Server 2022 run | PE binaries built | Stdout and exit code match | All available goldens match |
|---|---:|---:|---:|
| Before console runtime changes (`b92021b`) | 337 | 337/402 | 239/402 |
| With console I/O and panic source locations (`a05427a`) | 353 | 353/402 | 353/402 |

In the baseline, 98 panics matched stdout and exit status but lacked the source
location in the stderr golden. After the change, every built program matched all
its goldens. The 49 compiler rejections group by the first reported unsupported
feature.

| Feature | Rejections |
|---|---:|
| Threads, channels, locks and atomics | 21 |
| Filesystem and paths | 7 |
| Clocks and timers | 6 |
| Process arguments and environment | 5 |
| Extern C | 5 |
| Sockets | 4 |
| Raw Linux memory syscalls | 1 |

The driver boots `~/vm-windows/run-vm.sh` and uses `host.py` and the guest agent.
If the agent was configured only for the first logon, run the following in a guest
PowerShell after sign-in while the driver waits. The driver submits
`Stop-Computer -Force` and waits for QEMU to exit, including on job failure.

```powershell
Invoke-Expression (Invoke-RestMethod 'http://10.0.2.2:8000/agent.ps1')
```

## Other verification

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

* Earlier loader and unwind smoke on Windows Server 2022 (build 20348) in a QEMU VM, driven by `tests/windows_vm_smoke.sh`
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
  Verification uses `tests/pe_layout.sh` in `run_all.sh` and the Windows VM smoke test.
* The stack is committed up front (8 MiB) instead of using the guard page, so large frames need no `__chkstk` probes.
  Deep recursion beyond 8 MiB crashes with an access violation instead of a clean message (Linux: SIGSEGV too).
* `std.io` retries short standard-output writes. The built-in `print` uses a single synchronous write.
* Redirected I/O preserves bytes. Console rendering depends on the code page and font. Interactive Unicode input
  needs separate verification.
* PE supports the console subsystem. Filesystem access and Windows path handling, command-line arguments, the
  environment, clocks, subprocesses, sockets, threads, extern C, and runtime backtraces remain outside this support.
* The code is the unoptimised "every vreg in a stack slot" output (see BACKEND.md); no Windows-specific speed work.
* `aarch64` Windows and 32-bit x86 are out of scope.


## Running the compiler on Windows (host support)

The `windows-check` CI job cross-checks the Rust compiler with
`cargo check --tests --target x86_64-pc-windows-gnu`. The `windows` job runs the compiler on
`windows-latest` under Git Bash. The VM corpus above measures the generated PE binaries.

| Topic | Behaviour |
|---|---|
| Native backend on a Windows host | `yerux build prog.yx --native` (and `run`) produce a Windows PE `.exe` (target defaults to `windows` when the host is Windows); the default output name gets `.exe` |
| C compiler | `$YERUX_CC`, else the first of `cc`, `gcc`, `clang`, `x86_64-w64-mingw32-gcc` that runs. Extra flags: `$YERUX_CC_FLAGS`. MSVC `cl` is not supported (the generated C uses POSIX threads/sockets; MinGW-w64 and clang provide them) |
| No C compiler | build/run fall back to the native backend with a note; `check`, `c`, `ir`, `fmt`, `lsp` need none |
| `yerux doctor` | reports the compiler found (or MISSING and why), wine, temp dir, platform, and compiles + runs a program with each backend; exit 1 only if no backend works |
| Paths and command lines | temp files use `std::env::temp_dir()`; file arguments may contain spaces and either slash; CRLF and BOM sources are accepted (checked by `tests/win_smoke.sh`) |
| Limits | the native Windows backend has the limits listed above (extern C and OS intrinsics outside the supported console subset are rejected with `not supported on the windows target yet`); programs using them need the C backend (MinGW) or Linux |

`tests/win_smoke.sh` runs on Linux and Windows. It checks `--version`, `doctor`, `run` on a CRLF
source in a directory with spaces, the default output name, and native output against the
goldens for examples, `tests/fast.list` and dedicated console regressions. `--all` covers
every program with a golden.
