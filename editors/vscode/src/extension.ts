import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export async function activate(
  context: vscode.ExtensionContext,
): Promise<void> {
  context.subscriptions.push(
    vscode.commands.registerCommand("graphcal.restartServer", async () => {
      if (!client) {
        return;
      }
      if (client.isRunning()) {
        await client.restart();
      } else {
        await client.start();
      }
    }),
  );

  const config = vscode.workspace.getConfiguration("graphcal.lsp");
  if (!config.get<boolean>("enabled", true)) {
    return;
  }

  client = createLanguageClient(config);
  if (client) {
    await client.start();
  }
}

export async function deactivate(): Promise<void> {
  if (client) {
    await client.stop();
  }
}

function createLanguageClient(
  config: vscode.WorkspaceConfiguration,
): LanguageClient | undefined {
  const command = resolveGraphcalPath(config);

  const serverOptions: ServerOptions = {
    command,
    args: ["lsp"],
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "graphcal" }],
  };

  return new LanguageClient(
    "graphcal-lsp",
    "Graphcal Language Server",
    serverOptions,
    clientOptions,
  );
}

function resolveGraphcalPath(
  config: vscode.WorkspaceConfiguration,
): string {
  const configured = config.get<string>("path", "");
  if (configured) {
    return configured;
  }
  return "graphcal";
}
