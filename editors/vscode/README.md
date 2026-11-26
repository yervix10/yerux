# Yerux for VS Code

Support for [Yerux](https://github.com/yervix10/yerux), a compiled language with ownership and borrow checking.

![Yerux](https://raw.githubusercontent.com/yervix10/yerux/main/editors/vscode/icons/yerux.png)

## What you get

* **Syntax highlighting** for `.yx` files (TextMate grammar plus semantic tokens). It works as soon as the
  extension is installed.
* **Language server** (`yerux lsp`): live diagnostics, completion, outline, go to definition (also into the standard
  library), find references, safe rename, hover types, signature help and formatting.

## Requirements

The extension does **not** include the compiler. Install `yerux` (Linux x86-64 and AArch64):

```sh
curl -fsSL https://raw.githubusercontent.com/yervix10/yerux/main/install.sh | sh
yerux doctor        # checks the installation
```

Without `yerux` on your `PATH` you still get the colors; the language server is off and the extension tells you so.

If VS Code cannot see `yerux` (typical when it is started from the application launcher), set **Yerux: Server Path**
(`yerux.serverPath`) to the full path of the executable, for example `/home/me/.local/bin/yerux`.

## Commands and settings

| | |
|---|---|
| `Yerux: Restart language server` | Command palette. Use it after installing `yerux` or changing the path. |
| `yerux.serverPath` | The `yerux` executable that is started as `yerux lsp`. Default: `yerux`. |
| `yerux.trace.server` | `off`, `messages` or `verbose`: trace the traffic with the server. |

## Troubleshooting

* **No colors, and the status bar says Plain Text:** choose *Yerux* as the language of the file.
* **"the yerux compiler was not found":** install it (above) or set `yerux.serverPath`, then run
  *Yerux: Restart language server*.
* **Anything else:** open *View → Output → Yerux Language Server* and include what it says when you
  [report a problem](https://github.com/yervix10/yerux/issues).

## Install without a marketplace

Each [release](https://github.com/yervix10/yerux/releases) of Yerux has the `.vsix`:

```sh
curl -fsSLO https://github.com/yervix10/yerux/releases/download/v1.0.0/yerux-0.3.0.vsix
code --install-extension yerux-0.3.0.vsix
```

## Building it yourself

From `editors/vscode` in the Yerux repository (needs Node.js 18+):

```sh
npm install
npm run package                 # writes yerux-<version>.vsix
code --install-extension yerux-*.vsix
```

The language server itself is documented in [docs/LSP.md](https://github.com/yervix10/yerux/blob/main/docs/LSP.md).

## License

MIT OR Apache-2.0, at your option. See `LICENSE` and `LICENSE-APACHE`.
