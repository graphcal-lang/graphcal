// The VS Code webview's host: the extension sends models (projected by the
// language server) and the sources they came from; edits go back as text
// replacements on the editor's documents. The server re-analyzes the changed
// document and the extension sends the fresh model, so an edit resolves at
// once and the view updates when the model arrives.
import type { EditResult, Host, NoteKind, SourceEdit } from "./host.ts";
import { parseViewModel, type ViewModel } from "./model.ts";

/** Messages from the extension. */
type Inbound =
  | { type: "model"; model: unknown; sources: Record<string, string>; main: boolean }
  | { type: "note"; message: string | null };

/** Messages to the extension. */
type Outbound =
  | { type: "ready" }
  | { type: "edit"; file: string; start: number; end: number; text: string }
  | { type: "request"; module: string };

interface VsCodeApi {
  postMessage(message: Outbound): void;
}

declare function acquireVsCodeApi(): VsCodeApi;

/** Whether this page runs inside a VS Code webview. */
export function inVsCode(): boolean {
  return typeof acquireVsCodeApi === "function";
}

/** The smallest replacement turning `before` into `after`: shared prefix and suffix trimmed. */
export function minimalEdit(
  before: string,
  after: string,
): { start: number; end: number; text: string } {
  let start = 0;
  const max = Math.min(before.length, after.length);
  while (start < max && before[start] === after[start]) start += 1;
  let endBefore = before.length;
  let endAfter = after.length;
  while (endBefore > start && endAfter > start && before[endBefore - 1] === after[endAfter - 1]) {
    endBefore -= 1;
    endAfter -= 1;
  }
  return { start, end: endBefore, text: after.slice(start, endAfter) };
}

export class VsCodeHost implements Host {
  readonly modules: readonly string[] = [];
  readonly edited = false;
  private readonly api = acquireVsCodeApi();
  private readonly models = new Map<string, ViewModel>();
  private readonly sources = new Map<string, string>();
  private readonly modelListeners: ((models: ViewModel[]) => void)[] = [];
  private readonly openListeners: ((module: string) => void)[] = [];
  private readonly noteListeners: ((message: string | null, kind: NoteKind) => void)[] = [];
  /** The module shown as the main sheet; a fresh model of it updates in place. */
  private mainModule: string | null = null;

  constructor() {
    window.addEventListener("message", (event: MessageEvent<Inbound>) => this.receive(event.data));
    this.api.postMessage({ type: "ready" });
  }

  private receive(message: Inbound): void {
    switch (message.type) {
      case "model": {
        let model: ViewModel;
        try {
          model = parseViewModel(message.model);
        } catch (error) {
          this.note(error instanceof Error ? error.message : String(error));
          return;
        }
        this.models.set(model.module.id, model);
        for (const [file, text] of Object.entries(message.sources)) this.sources.set(file, text);
        if (message.main && this.mainModule !== model.module.id) {
          this.mainModule = model.module.id;
          for (const listener of this.openListeners) listener(model.module.id);
        } else {
          for (const listener of this.modelListeners) listener([model]);
        }
        return;
      }
      case "note":
        this.note(message.message);
        return;
      default:
        return;
    }
  }

  private note(message: string | null): void {
    for (const listener of this.noteListeners) listener(message, "error");
  }

  model(module: string): ViewModel | null {
    return this.models.get(module) ?? null;
  }

  source(_model: ViewModel, file: string): string | null {
    return this.sources.get(file) ?? null;
  }

  edit(_model: ViewModel, edit: SourceEdit): Promise<EditResult> {
    const before = this.sources.get(edit.file);
    if (before === undefined)
      return Promise.resolve({ ok: false, message: `unknown file ${edit.file}` });
    const { start, end, text } = minimalEdit(before, edit.next);
    this.sources.set(edit.file, edit.next);
    this.api.postMessage({ type: "edit", file: edit.file, start, end, text });
    return Promise.resolve({ ok: true, models: [] });
  }

  request(module: string): void {
    this.api.postMessage({ type: "request", module });
  }

  reset(): void {
    // The editor owns the sources; there is nothing to discard here.
  }

  onModels(listener: (models: ViewModel[]) => void): void {
    this.modelListeners.push(listener);
  }

  onOpen(listener: (module: string) => void): void {
    this.openListeners.push(listener);
  }

  onNote(listener: (message: string | null, kind: NoteKind) => void): void {
    this.noteListeners.push(listener);
  }
}
