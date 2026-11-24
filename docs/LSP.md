# `yerux lsp`: language server

`yerux lsp` speaks the Language Server Protocol (JSON-RPC 2.0, `Content-Length` framing) on stdin/stdout. It is
part of the compiler binary and reuses the Rust front end; there is no new dependency (`src/json.rs` is a small
JSON reader/writer, `src/lsp.rs` the server). The VS Code client is in [`editors/vscode`](../editors/vscode/README.md).

## What it does

| Request / notification | Behaviour |
|---|---|
| `initialize` | capabilities: incremental sync (`change: 2`, `openClose`, `save`), completion (trigger `.`), signature help (`(` and `,`), definition, hover, code actions, references, rename (with `prepareProvider`), document symbols, formatting, semantic tokens (full) |
| `didOpen` / `didChange` / `didSave` / `didClose` | `didOpen`/`didSave` run the front end at once; `didChange` applies the ranged edits and waits for a typing pause (150 ms, `YERUX_LSP_DEBOUNCE_MS`) before publishing, so a burst of edits costs one analysis |
| `textDocument/publishDiagnostics` | the compiler's errors with ranges and notes |
| `textDocument/documentSymbol` | hierarchical outline: `fn`, `struct` (fields), `enum` (variants), `trait` and `impl` (methods), `const`, `type` |
| `textDocument/definition` | locals, parameters, loop variables, tuple-pattern names, closure parameters, `match` arm bindings, items, variants, fields and methods **by the type of the receiver**, `alias.item` of an imported module, and items of `std.*`, the prelude and the compiler builtins |
| `textDocument/hover` | the checked type of a `let`/`var` (`let total: Int`), the declaration of parameters, functions, methods, types and module items; contiguous `///` comments above declarations are included as Markdown documentation |
| `textDocument/codeAction` | quick fixes to import a missing public standard-library function or derive `Clone`, `Equal` or `Show` for a local struct/enum when the diagnostic names the missing trait |
| `textDocument/completion` | after `.`: fields and methods of the receiver's type (generics applied: `names.` on `List<String>`), items of a module (`fs.`), variants and associated functions of a type (`Shape.`); elsewhere: locals and parameters in scope, the file's items, imported modules, prelude items, types, snippets (`fn`, `struct`, `enum`, `match`, `for`, `if`, `while`, `impl`, `trait`, `test`) and keywords; after `import`: module names (`std.` lists the standard library) |
| `textDocument/signatureHelp` | the callee's signature (function, module function or method) with the active parameter |
| `textDocument/references` | every use of a declaration in the open files, scope-aware (a shadowing `let` is another declaration) |
| `textDocument/prepareRename`, `rename` | `WorkspaceEdit` over the open files; refused for std/prelude/builtin names, keywords, invalid names, members shared by several types, uses whose receiver type is unknown, and when the new name would clash (another local in scope, another item, or hiding an item) |
| `textDocument/semanticTokens/full` | namespace, type, struct, enum, enumMember, function, method, parameter, variable, property, keyword, interface; modifiers `declaration` and `defaultLibrary` |
| `textDocument/formatting` | one edit replacing the whole buffer with `yerux fmt --stdout`'s output (the same `fmt::format_source`); no edits when already formatted; an error response when the text does not parse |
| `shutdown` / `exit` | exit code 0 after `shutdown`, 1 without it |

Any other request gets a `-32601` error; an unparsable message is skipped.

## Design

* **Diagnostics are the compiler's.** `analyze` calls `modules::load` → `Parser::program` → `check::check_program`
  exactly like `yerux check`, and turns each rendered error (`error: msg`, `--> file:line:col`, the caret line,
  `= note` lines) back into a diagnostic: the range is the caret run, the notes are appended to the message,
  severity is Error. The compiler has no warnings, so there are no warning diagnostics yet. The checker
  reports every error it finds, the loader/parser stop at the first one.
* **Unsaved buffers.** `modules::set_overlay` makes the loader read the editor's open buffers (by path) instead
  of the disk, so an edited, unsaved `import`ed file is what the importing file is checked against.
* **Library files.** A file without `fn main()` is checked as a library: the "missing `fn main()`" error is
  dropped when the file declares no `main`. An error in *another* file (an import) is shown at 1:1 of the edited
  file with the file, line and column in the message.
* **Names are resolved lexically, types come from the checker.** `lsp.rs` has a lexer-based item scanner and the
  scope rules (`decl_at`, `visible_locals`: the locals of the enclosing blocks of a function, skipping closed
  blocks and their headers, then the parameters; closure parameters live to the end of the closure, match-arm
  and tuple names are declarations). That keeps symbols, definition and completion working while the file does not
  compile, which is when an editor needs them. `lsp/ide.rs` adds the cross-file part:
  * `Ctx` is the set of files a request can see: the edited buffer, its imports (transitively; open buffers win over
    the disk), the compiler builtins and the prelude.
  * **Types as text.** A variable's type is the checker's (`check_trace::typed_bindings`, the data of
    `yerux types-file`) when the buffer compiles; otherwise it is inferred from the declaration (parameter
    annotation, `let x: T`, or the initializer evaluated as a chain of calls / fields / methods / indexing / `?`).
    `chain_type` then walks `a.b(c).d` through `members_of`, which reads the struct fields and the `impl` blocks of
    every visible file and substitutes the impl's generics (`impl<T> List<T>` with `List<String>` gives `T = String`).
  * **Completion while typing `p.na`** leaves a file that does not compile; the server then checks the buffer
    without the line being typed, which gives the types of everything declared above it.
  * **The standard library index.** `std.*` sources are embedded in the compiler (`stdlib.rs`); `lib_doc` writes them,
    the prelude and a `builtins.yx` (signatures for the methods the checker implements itself: `List.push`,
    `String.push_str`, ...) as real files under `$TMPDIR/yerux-lsp-<version>/`, so definition works in any
    editor and the parsed documents are cached. Those files are read-only for rename.
* **Incremental sync** applies each ranged change to the stored text (UTF-16 columns), then rebuilds the document once.
  A reader thread feeds the main loop, which wakes up for the debounce deadline; requests are answered in order
  and never wait for diagnostics.
* **Positions.** The lexer counts characters; LSP counts UTF-16 units. `Doc::to_lsp`/`from_lsp` convert per line.
* A panic inside the front end is caught and shown as a diagnostic instead of killing the server.

## Limits

* Resolution is by name and receiver type, not a full type checker: generic parameters of *methods* are not
  inferred, `x.name` with an unknown receiver type jumps to the first member of the file with that name (such
  uses are guesses, and rename refuses them), trait-bound generics (`T: Show`) offer no members, and expressions
  with operators or closures in the middle of a chain are not typed.
* References and rename see the open files only; a closed file that imports the renamed item is not edited.
* Pattern names inside struct patterns are not recognised as declarations.
* No workspace symbols, call hierarchy or range-formatting; code actions cover standard-library function imports and the built-in derives `Clone`, `Equal` and `Show`; semantic tokens are `full` only and
  skip string/number literals and comments (the TextMate grammar colors those).
* The formatter has one style, so formatting ignores the editor's tab settings.

## Tests

`cargo test --test lsp` (`tests/lsp.rs`) starts the binary and talks JSON-RPC to it, one or more tests per
capability: initialize, didOpen, publishDiagnostics (clean, error with its range, fixed, parse error, library
without main, an import changed in an unsaved buffer), documentSymbol, definition (locals, items, fields, modules,
match/closure/tuple bindings, `std.*` files, builtins), hover, completion (by receiver type, modules, variants,
scope, imports, snippets, prefix filter), signature help, references, rename (accepted, refused, across files),
semantic tokens (decoded and checked token by token), incremental edits (ranges, UTF-16, burst of edits gives one
publication), formatting, bad input and exit codes. Fixtures are in `tests/lsp/`. `tests/lsp_vscode.sh` checks the
extension's JSON files, that every grammar regex compiles and that every compiler keyword is highlighted. Both
run from `tests/run_all.sh`. `tests/lsp_real.sh` (`tests/lsp_session.py`) drives scripted sessions over every program of
`examples/real/` (23 programs, 728 items): didOpen without error diagnostics, documentSymbol, formatting equal to `yerux fmt`,
hover/definition/references of every item (each use must resolve back to its declaration), renames of items and locals whose
WorkspaceEdit is applied to a copy and must still compile (a sample by default, `--all` renames everything: 2374 renames), didChange
sessions with syntax and type errors (diagnostic on the right line with a range, cleared by the fix), completion after `.`,
signature help and refused renames. The VS Code package was built with `vsce` and installed with
`code --install-extension` (see `editors/vscode/README.md`).
