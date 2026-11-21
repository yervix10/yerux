# `yerux lsp`: language server

`yerux lsp` speaks the Language Server Protocol (JSON-RPC 2.0, `Content-Length` framing) on stdin/stdout. It is
part of the compiler binary and reuses the Rust front end; there is no new dependency (`src/json.rs` is a small
JSON reader/writer, `src/lsp.rs` the server). The VS Code client is in [`editors/vscode`](../editors/vscode/README.md).

## What it does

| Request / notification | Behaviour |
|---|---|
| `initialize` | capabilities: full-text sync (`openClose`, `change: 1`, `save`), `documentSymbolProvider`, `definitionProvider`, `hoverProvider`, `documentFormattingProvider` |
| `didOpen` / `didChange` / `didSave` | runs the front end on the buffer and publishes `textDocument/publishDiagnostics` |
| `didClose` | forgets the buffer and clears its diagnostics |
| `textDocument/documentSymbol` | hierarchical outline: `fn`, `struct` (fields), `enum` (variants), `trait` and `impl` (methods), `const`, `type` |
| `textDocument/definition` | local variables, parameters, loop variables, top-level items, enum variants, fields and methods, and `alias.item` of an imported module (its file) |
| `textDocument/hover` | the **checked type** of a `let`/`var` (`let total: Int`), `(parameter) k: Int`, and the declaration of functions, structs, enums, traits, constants and module items |
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
  of the disk, so an edited, unsaved `import`ed file is what the importing file is checked against. The overlay
  is set around each analysis and cleared afterwards.
* **Imports analysed from the file you edit.** The entry is the buffer being edited, so a library file without
  `fn main()` is checked as a library: the "missing `fn main()`" error is dropped when the file declares no
  `main`. An error that is in *another* file (an import) is shown at 1:1 of the edited file with the file, line
  and column in the message.
* **Symbols, definition and hover shape are lexical** (`syntax::lex` + a small item scanner). They keep working
  while the file does not compile, which is exactly when an editor needs them. Definition of a local walks back
  through the enclosing blocks of the enclosing function (a `let` in a closed sibling block is not visible),
  then the parameters, then the file's items.
* **Hover types are the checker's** (`check_trace::typed_bindings`, the same data as `yerux types-file`), keyed by the
  position of the `let`/`var` keyword. They exist only for a buffer that compiles; otherwise the hover shows the
  annotation written in the source (if any). The types are recomputed lazily after an edit.
* **Positions.** The lexer counts characters; LSP counts UTF-16 units. `Doc::to_lsp`/`from_lsp` convert per line.
* A panic inside the front end is caught and shown as a diagnostic instead of killing the server.

## Limits

* Full-text synchronisation: the whole buffer is re-sent and re-analysed on each change (millisecond scale for
  the programs in this repository; there is no debounce or cancellation).
* Hover/definition do not know types: `x.name` jumps to the first method or field called `name` in the file,
  not the one of `x`'s type. Match-arm bindings, closure parameters and tuple patterns are not found as
  declarations. No hover/definition into `std.*` (those modules are embedded in the compiler and have no file).
* No completion, rename, references, signature help, semantic tokens or workspace symbols.
* Formatting ignores the editor's tab settings: the formatter has one style.

## Tests

`cargo test --test lsp` (`tests/lsp.rs`) starts the binary and talks JSON-RPC to it: initialize, didOpen,
publishDiagnostics (clean, error with its range, fixed, parse error, library without main, an import changed in
an unsaved buffer), documentSymbol, definition, hover, formatting, bad input and exit codes. Fixtures are in
`tests/lsp/`. `tests/lsp_vscode.sh` checks the extension's JSON files, that every grammar regex compiles and that
every compiler keyword is highlighted. Both run from `tests/run_all.sh`.
