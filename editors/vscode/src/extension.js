// VS Code client for `yerux lsp`: starts the server over stdio and lets vscode-languageclient do the rest.
const vscode = require("vscode");
const { LanguageClient } = require("vscode-languageclient/node");

let client;

function activate(context) {
  const cfg = vscode.workspace.getConfiguration("yerux");
  const command = cfg.get("serverPath") || "yerux";
  const serverOptions = {
    // No `transport`: for a command, vscode-languageclient then talks over stdio without appending `--stdio`.
    run: { command, args: ["lsp"] },
    debug: { command, args: ["lsp"] },
  };
  const clientOptions = {
    documentSelector: [{ scheme: "file", language: "yerux" }],
    outputChannelName: "Yerux Language Server",
  };
  client = new LanguageClient("yerux", "Yerux Language Server", serverOptions, clientOptions);
  client.start().catch((err) => {
    vscode.window.showErrorMessage(
      `Could not start \`${command} lsp\`: ${err.message}. Set "yerux.serverPath" to the yerux executable.`
    );
  });
  context.subscriptions.push(
    vscode.commands.registerCommand("yerux.restartServer", async () => {
      if (client) await client.stop();
      client = new LanguageClient("yerux", "Yerux Language Server", serverOptions, clientOptions);
      await client.start();
    })
  );
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
