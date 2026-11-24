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
* Fatal panics print the message and, when available, the source position to stderr before calling
  `ExitProcess(101)`. A scoped worker can recover from a panic, drop its live values and return the
  failure to its joiner. Windows threads use `CreateThread` and `WaitForSingleObject`, a nonvolatile
  register for runtime TLS and `WaitOnAddress` / `WakeByAddress*` for contended locks. The internal wake
  primitive returns zero because Win32 provides no wake count.
* File operations include basic directory creation and removal, existence checks, working-directory
  changes and path or handle metadata. `current_dir` converts the Win32 UTF-16 path to UTF-8. For
  metadata, sizes and modification times come from the file handle. Permissions are synthesized from
  the file kind and read-only attribute. A no-follow metadata query treats reparse points as symlink-like.
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
and counts compiler rejections among the eligible tests (402 at the first two rows, 417 in the third).

| Windows Server 2022 run | PE binaries built | Stdout and exit code match | All available goldens match |
|---|---:|---:|---:|
| Before console runtime changes (`b92021b`) | 337 | 337/402 | 239/402 |
| With console I/O and panic source locations (`a05427a`) | 353 | 353/402 | 353/402 |
| With clocks, sleep, arguments, environment and file handles (`5f976aa`) | 377 | 377/417 | 377/417 |

In the baseline, 98 panics matched stdout and exit status but lacked the source
location in the stderr golden. After the change, every built program matched all
its goldens. The 49 compiler rejections of the first two rows were reduced to 40 (of 417 eligible) by mapping more Linux
system calls onto Win32 imports: `clock_gettime` (`QueryPerformanceCounter` for the monotonic clock,
`GetSystemTimePreciseAsFileTime` for the wall clock), `nanosleep` (`Sleep`, rounded up to milliseconds), process
arguments (`GetCommandLineW` converted to UTF-8 and split on blanks outside double quotes; backslash escapes are not
interpreted) and environment (`GetEnvironmentStringsA`), both laid out like the Linux initial stack so `std.env`
is unchanged, and `open`/`close`/`unlink`/`rename` (`CreateFileA`, `CloseHandle`, `DeleteFileA`,
`MoveFileExA`; the handle is the descriptor, `read`/`write` accept it; `/tmp/...` paths resolve to `C:\tmp`, which
the corpus job creates). The 40 rejections in that historical run group by the first reported unsupported feature.

| Feature | Rejections |
|---|---:|
| Threads, channels, locks and atomics (need `CreateThread`, TLS and a futex replacement) | 23 |
| Extern C (need DLL imports and a Win64 call shim) | 6 |
| Sockets | 4 |
| `getcwd` and directory iteration | 4 |
| `statx` | 2 |
| Raw Linux memory syscalls | 1 |

The driver boots `~/vm-windows/run-vm.sh` and uses `host.py` and the guest agent.
It runs sequential jobs of 32 programs by default. Use `--batch-size` to set
the number of programs per job. On hosts with limited memory, configure QEMU
with `-m 2560 -smp 2` and share the Linux suite's lock:

```sh
flock /tmp/yerux-run_all.lock python3 tests/windows_vm_corpus.py
```

With this VM configuration, all 353 built programs matched their goldens
again across 12 sequential jobs, and all 377 built programs of the 417 eligible tests matched in the last VM run.

If the agent was configured only for the first logon, run the following in a guest
PowerShell after sign-in while the driver waits. The driver submits
`Stop-Computer -Force` and waits for QEMU to exit, including on job failure.

```powershell
Invoke-Expression (Invoke-RestMethod 'http://10.0.2.2:8000/agent.ps1')
```

## Current cross-build audit and Wine verification

`python3 tests/windows_pe_audit.py` builds the runtime goldens in `tests/` and the standalone programs
under `examples/`. Intentional compile-error examples and modules without `main` are excluded. The
[audit snapshot](windows-pe-rejections.tsv) records each rejected source and its exact first diagnostic
before and after the changes. The baseline is `2b8acbb`.

| Same corpus before and after | Before | With threads and filesystem changes |
|---|---:|---:|
| Test goldens rejected | 56/446 | 34/446 |
| Example entry points rejected | 19/67 | 13/67 |
| Total rejected | **75/513** | **47/513** |

The two new filesystem regressions also build, for **468/515** PE builds in the updated corpus. The
counts measure compiler acceptance. A program can use several unsupported features, so removing its
first diagnostic may reveal another.

| First compiler rejection | Before | After |
|---|---:|---:|
| Thread runtime and atomics | 31 | 0 |
| Sockets (`setsockopt`) | 9 | 16 |
| Directory iteration (`getdents64`) | 0 | 13 |
| Process handling (`wait4`) | 8 | 8 |
| Extern C | 6 | 6 |
| Working directory (`getcwd`) | 10 | 0 |
| Metadata (`statx`) | 7 | 0 |
| Existence (`access`) and mkdir | 3 | 0 |
| Other filesystem operations (`lseek`, `readlink`, `chmod`) | 0 | 3 |
| Raw Linux memory mapping | 1 | 1 |

Wine 10.0 matched the C backend's stdout and exit status on all 28 newly buildable programs and both
new regressions. This included scoped panic and cleanup, nested workers, contended locks, a four-worker
`schedsim` run, a scripted `edlite` session and a `tasks` listing from a prepopulated file. The Windows
CI smoke suite requires the thread and filesystem regressions to build and run. Windows Server figures
above are from the earlier native Windows run.

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
* Executables use the console subsystem and the shared x86-64 register allocator. Subprocesses,
  sockets, extern C and runtime backtraces remain unavailable.
* Path-taking Win32 file calls still use the ANSI code page. Native path syntax and separator rules
  differ from the POSIX `std.path` conventions. Directory enumeration and handle-relative traversal,
  including recursive tree removal, still require unsupported syscalls. Windows ACLs, full POSIX
  mode changes, and Linux `/proc/self/exe` semantics are outside the metadata subset.
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
| Limits | the native Windows backend has the limits listed above (extern C and unsupported OS intrinsics are rejected with `not supported on the windows target yet`); programs using them need the C backend (MinGW) or Linux |

`tests/win_smoke.sh` runs on Linux and Windows. It checks `--version`, `doctor`, `run` on a CRLF
source in a directory with spaces, the default output name, and native output against the
goldens for examples, `tests/fast.list` and dedicated console regressions. `--all` covers
every program with a golden.
