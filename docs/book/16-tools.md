# 16. Tools

Everything below is part of the one `yerux` binary.

| Command | What it does |
|---|---|
| `yerux check file.yx` | type- and ownership-check, print every error, generate nothing |
| `yerux build file.yx [-o out] [--debug\|--release] [--native] [--target linux\|aarch64\|windows]` | compile to an executable |
| `yerux run file.yx [--native]` | build to a temporary file and run it |
| `yerux c file.yx` / `yerux ir file.yx` | print the generated C / the optimised IR |
| `yerux new NAME` / `yerux build` / `yerux run` / `yerux test` | project commands (chapter 13) |
| `yerux test [file.yx\|dir] [--native] [--filter text]` | run `test "name" { .. }` blocks |
| `yerux fmt [--check\|--stdout] file.yx\|dir` | the formatter |
| `yerux add NAME --path DIR\|--git URL [--version REQ]` / `yerux remove NAME` | edit the dependencies of `yerux.toml`, resolve versions, update `yerux.lock` |
| `yerux doc [--html] [-o DIR] [dir]` | API pages from the `///` comments of public items |
| `yerux bench [file.yx\|dir] [--runs N]` | run the `bench "name" { .. }` blocks and report min/median/mean |
| `yerux lsp` | the language server |
| `yerux --version` / `yerux doctor` | the version / a self-diagnosis of the installation (C compiler, wine, both backends) |
| `yerux tokens`, `ast-file`, `resolve-file`, `types-file`, `locals-file` | dumps used by the self-hosting comparison tests |

The four tools added last (`add`/`remove`, `doc`, `bench`, and `fmt --check` for CI) are described in [TOOLS.md](../TOOLS.md).

### Build profiles

`yerux build` defaults to the release profile: the C backend uses `-O2`, and the native backend runs its
optimizer. `--release` selects that behavior explicitly. `--debug` keeps native runtime checks enabled and
disables native optimization; with the C backend it uses `-O0 -g` so C debuggers can read generated code and
symbols. Choose only one profile. `--no-opt` remains available as a low-level alias for disabling native
optimization.

## Tests: `yerux test`

A `test "name" { .. }` block at the top level of any `.yx` file is a test: its body runs as `main`, and a
failed `assert` (exit code 101) fails it. `yerux test` finds the blocks in the files you name (or in a folder,
recursively), builds one program per test and reports each. Test blocks are dropped from normal builds.

```yerux test
fn clamp(x: Int, lo: Int, hi: Int) -> Int {
  if x < lo { return lo }
  if x > hi { return hi }
  return x
}

test "clamp keeps values inside" {
  assert(clamp(5, 0, 10) == 5)
  assert(clamp(-3, 0, 10) == 0)
  assert(clamp(99, 0, 10) == 10)
}

test "asserts compare values" {
  assert_eq(clamp(7, 7, 7), 7)
}
```

```sh
yerux test tests/ --filter clamp
```

## Formatting: `yerux fmt`

One style, no options. `yerux fmt file.yx` rewrites in place, `--check` lists the files that would change (exit
code 1 if any), `--stdout` prints the result. The formatter works on tokens, so it also formats code the
parser rejects. A self-hosted formatter (`selfhost/fmt.yx`, written in Yerux) is compared against it on the whole
repository by `tests/selfhost_fmt.sh`.

## The language server and VS Code

`yerux lsp` speaks the Language Server Protocol on stdin/stdout and reuses the compiler's own front end:

* live **diagnostics** (the checker's errors, with ranges, for the unsaved buffer and its unsaved imports);
* **completion**: members by the *type* of the receiver, module items, variants, locals in scope, `import` paths,
  snippets; **signature help**;
* **go to definition** and **hover** (checked types), also into `std.*`, the prelude and the compiler's builtins;
* **references** and a **safe rename** that refuses what it cannot prove;
* outline, **semantic tokens**, **formatting** (`yerux fmt`).

The extension in `editors/vscode/` adds the syntax highlighting and starts the server:

```sh
cd editors/vscode
npm install && npm run package          # writes yerux-0.2.0.vsix
code --install-extension yerux-0.2.0.vsix
```

If `yerux` is not on the `PATH` VS Code sees, set `yerux.serverPath`. Full design and limits:
[LSP.md](../LSP.md); installation without `vsce`: `editors/vscode/README.md`.

## Self-hosting

The compiler is a Rust program, but a growing part of it is rewritten in Yerux in `selfhost/` and compared with
the Rust original on every tracked `.yx` file of the repository: lexer, parser (source AST), module and name
resolution, lexical bindings, a type and ownership checker, a formatter, and a C code generator.
`tests/selfhost_codegen.sh` is the bootstrap: the Rust compiler builds the stages (`lexer`, `parser`,
`resolve`, `types`, `codegen`, `fmt`), those stages compile their own sources to C, and the binaries built from
that C must compile the sources again to **byte-identical C**. It covers the subset of the language the stages use,
not yet the whole language; the status and the remaining work are in [SELFHOST.md](../SELFHOST.md).

## Keeping this book honest

`bash tests/doctests.sh` compiles and runs every `yerux` block of this book and of `docs/GUIDE.md`, and
`tests/run_all.sh` runs it with the rest of the suite. If a snippet stops compiling, or prints something other
than its `output` block, the test fails: fix the text or the compiler.

That is the end of the book. Where to go next: [GUIDE.md](../GUIDE.md) (tour with more detail on strings, `Map`,
modules, `Box`, numbers), [SPEC.md](../../SPEC.md), the focused documents listed in the
[README](../../README.md), and the `examples/` and `tests/` folders.

Next: [Windows and other targets](17-windows-and-targets.md).
