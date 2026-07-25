// ChartLang extension: thin LanguageClient over the rantlr-lsp binary.
const path = require("path");
const fs = require("fs");
const vscode = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

let client;

function serverPath(context) {
  const configured = vscode.workspace.getConfiguration("chartlang").get("serverPath");
  if (configured && fs.existsSync(configured)) return configured;
  const ws = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (ws) {
    for (const rel of ["target/release/rantlr-lsp", "target/debug/rantlr-lsp"]) {
      const p = path.join(ws, rel);
      if (fs.existsSync(p)) return p;
    }
  }
  return "rantlr-lsp"; // PATH fallback
}

function activate(context) {
  const command = serverPath(context);
  client = new LanguageClient(
    "chartlang",
    "ChartLang (rantlr)",
    { command, transport: TransportKind.stdio },
    {
      documentSelector: [{ language: "chartlang" }, { language: "rantlr-grammar" }],
      // Let the server see language-definition saves quickly (it also
      // polls). Any `.rg` in the workspace root can be the language
      // definition, so watch them all rather than one fixed name.
      synchronize: {
        fileEvents: vscode.workspace.createFileSystemWatcher("**/*.{rg,toml}"),
      },
    }
  );
  client.start();
  context.subscriptions.push({ dispose: () => client && client.stop() });
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
