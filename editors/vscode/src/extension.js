// VS Code client for `yerux lsp`: starts the server over stdio and lets vscode-languageclient do the rest.
const vscode = require("vscode");
const { LanguageClient } = require("vscode-languageclient/node");
const { probe } = require("./server");

const INSTALL_URL = "https://github.com/yervix10/yerux#install-and-run";

let client;

function newClient(command) {
  const serverOptions = {
    // No `transport`: for a command, vscode-languageclient then talks over stdio without appending `--stdio`.
    run: { command, args: ["lsp"] },
    debug: { command, args: ["lsp"] },
  };
  const clientOptions = {
    documentSelector: [{ scheme: "file", language: "yerux" }],
    outputChannelName: "Yerux Language Server",
  };
  return new LanguageClient("yerux", "Yerux Language Server", serverOptions, clientOptions);
}

// Starts the language server, or explains what to do when `yerux` is not installed. Highlighting works either way.
async function start() {
  const command = vscode.workspace.getConfiguration("yerux").get("serverPath") || "yerux";
  const found = probe(command);
  if (!found.ok) {
    const text =
      found.reason === "missing"
        ? `Yerux: the \`${command}\` compiler was not found, so the language server is off (syntax colors still work). ` +
          `Install it, or set "yerux.serverPath" to the executable.`
        : `Yerux: \`${command} --version\` did not answer like yerux (${found.detail}). Check "yerux.serverPath".`;
    const pick = await vscode.window.showWarningMessage(text, "How to install", "Open settings");
    if (pick === "How to install") vscode.env.openExternal(vscode.Uri.parse(INSTALL_URL));
    if (pick === "Open settings") vscode.commands.executeCommand("workbench.action.openSettings", "yerux.serverPath");
    return;
  }
  client = newClient(command);
  try {
    await client.start();
  } catch (err) {
    vscode.window.showErrorMessage(`Could not start \`${command} lsp\`: ${err.message}. Set "yerux.serverPath" to the yerux executable.`);
  }
}

function activate(context) {
  start();
  context.subscriptions.push(
    vscode.commands.registerCommand("yerux.restartServer", async () => {
      if (client) {
        await client.stop().catch(() => {});
        client = undefined;
      }
      await start();
    })
  );
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
