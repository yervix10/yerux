# Capabilities for the standard library

Status (0.9.0-rc1): **only the library `std.caps` is implemented** (src/std/caps.yx): capability values
with narrowing and **run-time** checks (stages 2, values only, and 3 of section 10). **There is no effect
system and the compiler enforces no capability.** Sections 1-10 are the design; whatever they describe
beyond section 0 is not implemented.

| Piece | State |
|---|---|
| `Caps`/`Fs`/`Net`/`Process`/`Env` values, narrowing, run-time refusal | implemented, library only (tests/caps_grant.yx, tests/caps_deny.yx) |
| Effect annotations or inference in function signatures | not implemented |
| Compile-time rejection of stdlib calls without a capability | not implemented |
| `[capabilities]` in `yerux.toml`, grants by the root, needs of a dependency | not implemented (the section is ignored without a warning) |
| `Caps` as a parameter of `main` | not implemented (`main` takes no parameters) |
| `ffi` gating of `extern`/`unsafe`; a `threads` capability | not implemented |

So today:

- `std.fs`, `std.net`, `std.process` and `std.env` are callable from any module of any package with no
  check, and a dependency may declare `extern` blocks and use `unsafe`.
- Any code can call `caps.all()`, so receiving a narrow value restricts only code that uses nothing else.
- The reserved intrinsics (`__syscall` and the others, docs/STDLIB_IO.md) are also callable from user
  code, without `unsafe` and without an import.

`std.caps` therefore prevents mistakes in cooperating code. It is **not a security boundary** against
untrusted dependencies.

## 0. What exists today (`std.caps`)

```
import std.caps
let granted = caps.all()                                 // Caps { fs, net, process, env }: the full grant
let data = granted.fs.narrow("data", false)?             // Fs below ./data, read-only
let text = data.read_to_string("notes.txt")?             // PermissionDenied for "../x", "/etc/passwd", links
```

- `caps.all()` is what every program has today; `caps.none()` refuses everything.
- `Fs.narrow(root, write)` (root relative to the current root; errors with `PermissionDenied` if it would
  widen), `Fs.read_only()`, `Net.restrict(hosts)`, `Process.restrict(programs)`, `Env.restrict(names, write)`.
  `restrict` keeps only the entries the receiver already allows, so a value is never widened.
- `Fs` methods mirror std.fs (`read_to_string`, `read_lines`, `read_bytes`, `read_dir`, `exists`, `is_file`,
  `is_dir`, `file_size`, `open`, `write`, `write_bytes`, `append`, `create`, `create_dir`, `create_dir_all`,
  `remove_file`, `remove_dir`, `rename`, `copy`, `set_permissions`); `Net` has `connect`, `connect_timeout`,
  `listen`; `Process` has `run`, `run_timeout`, `run_command`, `start`; `Env` has `get`, `get_or`, `has`,
  `require`, `vars`, `set`, `remove`, `current_dir`, `set_current_dir`, `current_exe`.
- Tests: tests/caps_grant.yx (what a grant allows) and tests/caps_deny.yx (every refusal and its message).

Corrections to the design that the implementation forced:

1. **No `canonicalize` in the stdlib**, so containment is *lexical* (`path.normalize` of root + path must stay
   under the root, `..` cannot climb out) **plus a refusal of any symbolic link on the way below the root**.
   Links inside a root are therefore unusable under a narrowed `Fs`; this is conservative on purpose.
2. **One module, not `std.fs.safe` / `std.net.safe`**: the second layer is `std.caps`, because a method on a
   value (`data.read_to_string(p)`) reads better than a parallel set of functions and needs no new names.
3. **`Caps` is obtained with `caps.all()`**, not as a parameter of `main` (open question 1): changing the entry
   point needs compiler work that belongs to stage 2.
4. **Open question 4 is answered conservatively**: a restricted `Process` refuses any `Command` that uses
   `set_dir`, because the child could otherwise leave an `Fs` root. An unrestricted `Process` allows it.
5. **`Env` splits in two**: variable access (`get`, `set`, `vars`, ...) obeys the name lists; the working
   directory, `current_exe` and `exit` are whole-process authority and need an unrestricted `Env`. `vars()` returns
   only the readable variables.
6. **Handles are not re-checked**: a `File` or `TcpStream` returned by a checked call carries its own authority;
   what the grant controls is how one is obtained. Hand a handle to untrusted code only if that is intended.
7. **Without compile-time checks the layer is advisory**: code can still `import std.fs` directly. It is a
   real boundary only for code that is given a value instead of the modules (stage 2 makes the direct import
   an error for packages that were not granted it).

## 1. Problem

> Sections 1-10 are the design. Only what section 0 lists is implemented.

A package can read any file, open any socket, run any program and read every environment
variable that the application can. A third-party dependency therefore has the same authority
as the application that imports it. The goal is to make authority explicit and checkable,
without changing the meaning of programs that do not use it.

Memory safety is a separate concern (`unsafe`, docs/FFI.md section 7). Capabilities only
cover authority: what the program may ask the operating system to do.

## 2. What is granted

(Design. `Fs` root and read-only, `Net` host lists, `Process` program lists and `Env` name lists are
implemented in `std.caps`; write-only path sets and `canonicalize` are not.)

Four capability families, one per stdlib module that touches the outside world:

| Capability | Covers | Stdlib entry points |
|---|---|---|
| `fs` | paths, file contents, directories, links, permissions | `std.fs` (`read_to_string`, `write`, `open`, `create_dir_all`, `remove_file`, `set_permissions`, `symlink`, ...) |
| `net` | TCP sockets, address resolution | `std.net` (`connect`, `listen`, `accept`, `connect_timeout`, `address`) |
| `process` | starting child processes and sending signals to them | `std.process` (`start`, `run`, `run_command`, `run_timeout`) |
| `env` | environment variables, working directory, current executable, exit | `std.env` (`get`, `set`, `remove`, `vars`, `set_current_dir`, `current_exe`, `exit`) |

Pure modules (strings, lists, collections, JSON, compression, formatting) need no capability.
`std.io` is an error type plus traits; it grants nothing by itself, but reading from stdin and
writing to stdout/stderr are treated as always granted (see section 7).

Each capability can be narrowed. The first version should support:

- `fs` with a **root path**: all access is resolved under a directory, and `..` or symlinks
  that leave it are rejected (checked after `canonicalize`).
- `fs` read-only, or write-only to a set of paths.
- `net` restricted to a list of host:port pairs, or to loopback.
- `process` restricted to a list of program paths (absolute paths, no `PATH` search).
- `env` restricted to a list of variable names for read and for write.

Narrowing is a value, not a global flag, so the same process can hold a wide grant for one
module and a narrow one for another.

## 3. How a grant is passed

**Not implemented**: the manifest grants, the build error and the `main(caps: Caps)` entry point. Passing
`Fs`/`Net`/`Process`/`Env` values as ordinary parameters works today with `std.caps`.

There are two parts: who may grant, and how the grant travels.

**Granting.** Only the root application (the package that builds the executable) can grant
capabilities. Its manifest (docs/PACKAGES.md) lists them:

```
[capabilities]
fs = { root = "./data", write = true }
net = { hosts = ["127.0.0.1:8080"] }
```

A dependency declares which capabilities it *needs*, also in its manifest. The build fails if a
needed capability is not granted by the root, and the error names the dependency and the
capability. A dependency cannot grant itself anything.

**Passing.** Inside the program, a capability is a value. Possession is the authority: code can
only call a stdlib function that needs `fs` if it has an `Fs` value, and it can only get one as
a parameter or from the root. There is no ambient global. Example shape:

```
fn load_config(fs: view Fs) -> Result<String, IoError> {
  return fs.read_to_string("config.toml")
}

fn main(caps: Caps) -> Result<Unit, IoError> {
  let cfg = load_config(caps.fs)?
  ...
}
```

Because the value is a normal parameter, the existing ownership rules already apply: `view`
for read-only use, `move` to hand it on, and a narrowed `Fs` is a new value made by
`caps.fs.narrow(...)` that the caller may pass down.

## 4. Proposed API

Proposal. `narrow`, `read_only` and the four `restrict` methods exist in `std.caps` (section 0 is the
reference); `main(caps: Caps)`, optional fields and a capability parameter on the existing `std`
functions are not implemented.

```
// the root receives all granted capabilities at once
fn main(caps: Caps) -> ...

Caps { fs: Fs, net: Net, process: Process, env: Env }   // fields exist only if granted

Fs.narrow(root: String, write: Bool) -> Result<Fs, IoError>
Fs.read_only() -> Fs
Net.restrict(hosts: List<String>) -> Net
Process.restrict(programs: List<String>) -> Process
Env.restrict(names: List<String>, write: Bool) -> Env
```

Narrowing only ever reduces authority. `narrow` on a grant that is already narrower than the
request returns `PermissionDenied`.

The existing functions stay as they are. The proposed module form would add a capability parameter, for example `fs.read_to_string(f: view Fs, path: view String)`. The same check
can be expressed as a method call (`f.read_to_string(path)`) so that call sites read the same.

Errors use the existing `IoErrorKind.PermissionDenied` (src/std/io.yx). The message names
the capability and the value that was refused, for example:
`fs: path '../x' is outside the root 'data'`.

## 5. Enforcement

Two layers, both cheap because the checker already tracks ownership:

1. **Compile time.** (Not implemented.) A call to a stdlib function that needs a capability is rejected if no
   `Fs`/`Net`/`Process`/`Env` value of the right kind is in scope. This catches a dependency
   that reaches for a file without being handed one.
2. **Run time.** (Implemented in `std.caps`.) The narrowing checks (root containment, host list, program list, variable
   names) run inside the stdlib call. Compile time cannot know the actual path string, so this
   layer is required for the narrowed forms.

Code that does not use `std.caps` is not checked at all.

## 6. Interaction with `unsafe` and `extern`

**Not implemented.** An `extern` call, a `Ptr` operation or a reserved intrinsic can make an `fs` or `net` syscall without going through
the stdlib. Therefore an `extern` block needs the `ffi` capability (docs/FFI.md section 7), and
`unsafe` is still required for the call itself. For a package with neither capability to be unable to
reach the OS, the reserved intrinsics (`__syscall` and the others) would have to be gated too.

## 7. stdin, stdout and stderr

`print` and reading standard input are not gated. Gating `print` would break every example and
test, and a program's own console is not a meaningful isolation boundary for the threat model
here (a dependency that wants to exfiltrate data would use `net` or `fs`, which are gated). This
is a deliberate exclusion and should be revisited only if a use case needs it.

## 8. Compatibility with existing code

(Design for the unimplemented stages. There is no `--caps` flag.)

- **Source compatibility.** Existing programs keep compiling in the first version only if the
  root has a default grant of "everything", which is what they get today. A flag such as
  `yerux build --caps=none` runs the new checks; the default stays permissive until a major
  version.
- **Stdlib modules.** The modules in `src/std/*.yx` keep their current signatures. The
  capability layer is `std.caps` (section 0), so
  code that imports `std.fs` directly is unchanged. This also avoids changing the golden
  outputs in `tests/expected/`.
- **Tests.** `tests/` programs run as the root and get the full grant. A new test category
  checks that a denied capability produces the expected error.
- **Selfhost.** The compiler itself (selfhost/) reads files and runs the C compiler, so it runs
  with the full grant.
- **Dependencies.** Existing packages without a `[capabilities]` section are treated as needing
  nothing new, so the build keeps working. Only packages that declare a need are checked.

## 9. Open questions

1. Whether `Caps` is one value passed to `main`, or each capability comes from a
   `caps.request(...)` call that the runtime resolves from the manifest. (Today: `caps.all()`.)
2. Whether narrowing is also checked at compile time when the path is a literal.
3. Whether `env` reads should be grouped with `process` (the child inherits the environment).
4. How `fs` interacts with `std.process` child working directories (`set_dir`): a child started
   with `Process` must not escape the `fs` root through `set_dir`. (Answered in section 0, point 4.)
5. Whether a capability can be revoked after a value has been handed out. Proposed answer: no;
   use a narrower value instead.

## 10. Staging

1. Manifest syntax and the root grant table, with a build error for missing grants (no runtime
   checks yet). *Not implemented.*
2. Capability values (`Caps`, `Fs`, `Net`, `Process`, `Env`) and compile-time checks for the
   stdlib entry points. *Values implemented (`std.caps`); compile-time checks not implemented.*
3. Narrowing with runtime checks, starting with `fs.root` and `process` program lists. *Implemented for all
   four capabilities.*
4. `ffi` gating for `extern` and `unsafe` (and the reserved intrinsics). *Not implemented; FFI itself works
   (docs/FFI.md).*
