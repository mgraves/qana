// ChartLang extension: thin LanguageClient over the qana-lsp binary.
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
    for (const rel of ["target/release/qana-lsp", "target/debug/qana-lsp"]) {
      const p = path.join(ws, rel);
      if (fs.existsSync(p)) return p;
    }
  }
  return "qana-lsp"; // PATH fallback
}

function activate(context) {
  const command = serverPath(context);
  client = new LanguageClient(
    "chartlang",
    "ChartLang (qana)",
    { command, transport: TransportKind.stdio },
    {
      documentSelector: [{ language: "chartlang" }, { language: "qana-grammar" }],
      // Let the server see language-definition saves quickly (it also
      // polls). Any `.qana` in the workspace root can be the language
      // definition, so watch them all rather than one fixed name.
      synchronize: {
        fileEvents: vscode.workspace.createFileSystemWatcher("**/*.{qana,toml}"),
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
