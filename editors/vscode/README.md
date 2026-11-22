# Yerux for VS Code

Syntax highlighting (TextMate grammar plus semantic tokens) and a client for `yerux lsp`: live diagnostics,
completion, outline, go to definition (also into the standard library), find references, safe rename, hover
types, signature help and formatting. The design of the server is in `docs/LSP.md` of the Yerux repository.

## Install

Everything below is run from the root of the Yerux repository.

1. **Build the compiler** and put it on your `PATH`:

   ```sh
   cargo build --release                      # target/release/yerux
   export PATH="$PWD/target/release:$PATH"
   yerux lsp </dev/null; echo $?              # prints 1: the server starts and stops at end of input
   ```

2. **Package the extension** (needs Node.js 18+ and npm):

   ```sh
   cd editors/vscode
   npm install                                # installs vscode-languageclient and @vscode/vsce
   npm run package                            # writes yerux-0.2.0.vsix
   ```

3. **Install it**:

   ```sh
   code --install-extension yerux-0.2.0.vsix
   ```

   Reload VS Code and open any `.yx` file. (For VSCodium use `codium --install-extension`; for Cursor and other
   forks, *Extensions: Install from VSIX...* in the command palette.)

4. If `yerux` is not on the `PATH` that VS Code sees (typical on macOS when started from the dock), set
   **Yerux: Server Path** (`yerux.serverPath`) to the executable, e.g. `/home/me/yerux/target/release/yerux`.

### Without `vsce` (no network, or `npm run package` is not available)

The extension is plain JavaScript plus JSON; only `vscode-languageclient` has to be installed. Either:

* **Copy the folder** into the extensions directory, after `npm install --omit=dev` inside it:

  ```sh
  cd editors/vscode && npm install --omit=dev
  mkdir -p ~/.vscode/extensions && cp -r . ~/.vscode/extensions/yerux.yerux-0.2.0
  ```

  and restart VS Code (use `~/.vscode-oss/extensions` for VSCodium).

* **Or build the `.vsix` by hand**: a `.vsix` is a zip with an `extension/` folder holding these files and two
  files at the root (tested with `code --install-extension`; `python3 -m zipfile` replaces `zip`):

  ```sh
  cd editors/vscode && npm install --omit=dev
  rm -rf /tmp/yx && mkdir -p /tmp/yx/extension && cp -r package.json README.md LICENSE language-configuration.json src syntaxes node_modules /tmp/yx/extension/
  cat > "/tmp/yx/[Content_Types].xml" <<'XML'
  <?xml version="1.0" encoding="utf-8"?>
  <Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension=".json" ContentType="application/json"/><Default Extension=".js" ContentType="application/javascript"/><Default Extension=".md" ContentType="text/markdown"/><Default Extension=".vsixmanifest" ContentType="text/xml"/><Default Extension="" ContentType="text/plain"/></Types>
  XML
  cat > /tmp/yx/extension.vsixmanifest <<'XML'
  <?xml version="1.0" encoding="utf-8"?>
  <PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011"><Metadata><Identity Language="en-US" Id="yerux" Version="0.2.0" Publisher="yerux"/><DisplayName>Yerux</DisplayName><Description xml:space="preserve">Yerux language support</Description></Metadata><Installation><InstallationTarget Id="Microsoft.VisualStudio.Code"/></Installation><Assets><Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true"/></Assets></PackageManifest>
  XML
  (cd /tmp/yx && python3 -m zipfile -c yerux-0.2.0.vsix '[Content_Types].xml' extension.vsixmanifest extension) && code --install-extension /tmp/yx/yerux-0.2.0.vsix
  ```

  (The folder copy is the more robust of the two.)

To try the extension without installing anything: open `editors/vscode` in VS Code after `npm install` and
press **F5** (Extension Development Host).

## Features

| Feature | How |
|---|---|
| Diagnostics as you type | the compiler's own errors, with ranges; unsaved imported files count; recomputed after a short typing pause |
| Completion (`.` and Ctrl+Space) | members by the **type** of the receiver (`names.` on a `List<String>`), module items (`fs.`), enum variants (`Shape.`), locals and parameters in scope, items, standard-library modules after `import std.`, keywords and snippets (`fn`, `struct`, `enum`, `match`, `for`, `if`, `while`, `impl`, `trait`, `test`) |
| Go to definition (F12) | locals, parameters, `match` arm and closure/tuple bindings, items, fields, methods by receiver type, `module.item`, and into `std.*`, the prelude and compiler builtins (read-only copies in the temp directory) |
| Hover | checked type of a variable, signature of functions/methods/types, and adjacent `///` documentation |
| Quick fixes | import a missing public standard-library function; derive `Clone`, `Equal` or `Show` for a struct or enum |
| Signature help | on `(` and `,`, with the active parameter |
| References (Shift+F12) | scope-aware, across the open files |
| Rename (F2) | refuses std/prelude/builtin names, keywords, bad names and clashes; edits every open file that uses the item |
| Outline / breadcrumbs | functions, structs and fields, enums and variants, traits, impls |
| Semantic highlighting | namespaces, types, structs, enums, functions, methods, parameters, variables, properties; std items carry `defaultLibrary` |
| Format Document | `yerux fmt` |

Command **Yerux: Restart language server**; setting `yerux.trace.server` shows the JSON-RPC traffic.

## Files

| File | Purpose |
|---|---|
| `package.json` | extension manifest, settings, `npm run package` |
| `language-configuration.json` | comments, brackets, auto-closing, indentation |
| `syntaxes/yerux.tmLanguage.json` | TextMate grammar (`source.yerux`) |
| `src/extension.js` | starts `yerux lsp` with `vscode-languageclient` |

`tests/lsp_vscode.sh` (in the repository) validates the JSON files and the grammar's regexes without VS Code, and
`tests/lsp.rs` exercises the server over JSON-RPC.
