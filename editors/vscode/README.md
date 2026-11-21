# Yerux for VS Code

Syntax highlighting (TextMate grammar) and a client for `yerux lsp`: live diagnostics, outline, go to definition,
hover types and formatting. Design of the server: [docs/LSP.md](../../docs/LSP.md).

## Install

1. Build the compiler and put it on your `PATH` (or note its path):

   ```sh
   cargo build --release          # produces target/release/yerux
   export PATH="$PWD/target/release:$PATH"
   yerux lsp </dev/null           # starts and exits at end of input: the server is there
   ```

2. Install the extension's one dependency and package it:

   ```sh
   cd editors/vscode
   npm install
   npx vsce package               # writes yerux-0.1.0.vsix
   code --install-extension yerux-0.1.0.vsix
   ```

   To try it without packaging, open `editors/vscode` in VS Code after `npm install` and press **F5**
   (Extension Development Host), then open a `.yx` file.

3. If `yerux` is not on the `PATH` VS Code sees, set **Yerux: Server Path** (`yerux.serverPath`) to the
   executable, e.g. `/home/me/yerux/target/release/yerux`.

## Features

* Highlighting for keywords, types, functions, numbers (`0xFF`, `1_000`, `200u8`, `1.5f32`), strings with
  `{interpolation}` and `{{` escapes, characters and `//` comments.
* Errors from the compiler as you type (the file does not need to be saved; unsaved imported files count).
* Outline / breadcrumbs / `Ctrl+Shift+O`: functions, structs and fields, enums and variants, traits, impls.
* `F12` go to definition (locals, parameters, items, fields, methods, `module.item`).
* Hover: the checked type of a variable, the signature of a function or type.
* Format Document (`yerux fmt`).
* Command **Yerux: Restart language server**; `yerux.trace.server` shows the JSON-RPC traffic.

## Files

| File | Purpose |
|---|---|
| `package.json` | extension manifest, settings |
| `language-configuration.json` | comments, brackets, auto-closing, indentation |
| `syntaxes/yerux.tmLanguage.json` | TextMate grammar (`source.yerux`) |
| `src/extension.js` | starts `yerux lsp` with `vscode-languageclient` |

`tests/lsp_vscode.sh` validates the JSON files and the grammar's regexes without needing VS Code.
