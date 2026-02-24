import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";
import {
  findEnclosingTable,
  findNextCell,
  findPreviousCell,
  isInsideTable,
} from "./tableNavigation";

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

  // Table cell navigation commands
  context.subscriptions.push(
    vscode.commands.registerCommand(
      "graphcal.jumpToNextTableCell",
      () => jumpTableCell("next"),
    ),
    vscode.commands.registerCommand(
      "graphcal.jumpToPreviousTableCell",
      () => jumpTableCell("previous"),
    ),
  );

  // Update context key when cursor moves in a graphcal file
  context.subscriptions.push(
    vscode.window.onDidChangeTextEditorSelection((e) => {
      if (e.textEditor.document.languageId !== "graphcal") {
        return;
      }
      const doc = e.textEditor.document;
      const text = doc.getText();
      const offset = doc.offsetAt(e.selections[0].active);
      const inTable = isInsideTable(text, offset);
      vscode.commands.executeCommand(
        "setContext",
        "graphcal.cursorInTable",
        inTable,
      );
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

function jumpTableCell(direction: "next" | "previous"): void {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== "graphcal") {
    return;
  }

  const doc = editor.document;
  const text = doc.getText();
  const offset = doc.offsetAt(editor.selection.active);
  const table = findEnclosingTable(text, offset);
  if (!table) {
    return;
  }

  const target =
    direction === "next"
      ? findNextCell(text, offset, table)
      : findPreviousCell(text, offset, table);

  if (target === null) {
    return;
  }

  const newPos = doc.positionAt(target);
  editor.selection = new vscode.Selection(newPos, newPos);
  editor.revealRange(new vscode.Range(newPos, newPos));
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
