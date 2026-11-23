# Capabilities for the standard library

Status: **proposal**. Nothing in this document is implemented. Today `std.fs`, `std.net`,
`std.process` and `std.env` (src/std/*.yx) are callable from any module with no check. The
v0.6 item in SPEC.md ("no network/disk access without permission") and the `ffi` / `threads`
capabilities mentioned in docs/FFI.md and docs/CONCURRENCY.md are the places this design fits.

## 1. Problem

A package can read any file, open any socket, run any program and read every environment
variable that the application can. A third-party dependency therefore has the same authority
as the application that imports it. The goal is to make authority explicit and checkable,
without changing the meaning of programs that do not use it.

Memory safety is a separate concern (`unsafe`, docs/FFI.md section 7). Capabilities only
cover authority: what the program may ask the operating system to do.

## 2. What is granted

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

All names are provisional.

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

The existing functions stay as they are, but gain a capability parameter in the proposed
module form, for example `fs.read_to_string(f: view Fs, path: view String)`. The same check
can be expressed as a method call (`f.read_to_string(path)`) so that call sites read the same.

Errors use the existing `IoErrorKind::PermissionDenied` (src/std/io.yx:13). The message names
the capability and the value that was refused, for example:
`fs: path '/etc/passwd' is outside the root './data' granted in yerux.toml`.

## 5. Enforcement

Two layers, both cheap because the checker already tracks ownership:

1. **Compile time.** A call to a stdlib function that needs a capability is rejected if no
   `Fs`/`Net`/`Process`/`Env` value of the right kind is in scope. This catches a dependency
   that reaches for a file without being handed one.
2. **Run time.** The narrowing checks (root containment, host list, program list, variable
   names) run inside the stdlib call. Compile time cannot know the actual path string, so this
   layer is required for the narrowed forms.

Nothing is enforced for code compiled before this feature exists, see section 8.

## 6. Interaction with `unsafe` and `extern`

An `extern` call or a `Ptr` operation can make an `fs` or `net` syscall without going through
the stdlib. Therefore an `extern` block needs the `ffi` capability (docs/FFI.md section 7), and
`unsafe` is still required for the call itself. A package with neither capability cannot reach
the OS by any path.

## 7. stdin, stdout and stderr

`print` and reading standard input are not gated. Gating `print` would break every example and
test, and a program's own console is not a meaningful isolation boundary for the threat model
here (a dependency that wants to exfiltrate data would use `net` or `fs`, which are gated). This
is a deliberate exclusion and should be revisited only if a use case needs it.

## 8. Compatibility with existing code

- **Source compatibility.** Existing programs keep compiling in the first version only if the
  root has a default grant of "everything", which is what they get today. A flag such as
  `yerux build --caps=none` runs the new checks; the default stays permissive until a major
  version.
- **Stdlib modules.** The modules in `src/std/*.yx` keep their current signatures. The
  capability parameters are added as a second layer (`std.fs.safe`, `std.net.safe`, ...), so
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
   `caps.request(...)` call that the runtime resolves from the manifest.
2. Whether narrowing is also checked at compile time when the path is a literal.
3. Whether `env` reads should be grouped with `process` (the child inherits the environment).
4. How `fs` interacts with `std.process` child working directories (`set_dir`): a child started
   with `Process` must not escape the `fs` root through `set_dir`.
5. Whether a capability can be revoked after a value has been handed out. Proposed answer: no;
   use a narrower value instead.

## 10. Staging

1. Manifest syntax and the root grant table, with a build error for missing grants (no runtime
   checks yet).
2. Capability values (`Caps`, `Fs`, `Net`, `Process`, `Env`) and compile-time checks for the
   stdlib entry points.
3. Narrowing with runtime checks, starting with `fs.root` and `process` program lists.
4. `ffi` gating for `extern` and `unsafe`, after the FFI work in docs/FFI.md lands.
