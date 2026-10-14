// The VS Code webview's host: the extension sends models (projected by the
// language server) and copies of the sources they came from; edits go back as
// text replacements on those copies. Each copy comes with its project and a
// revision, and an edit names both, so the extension applies it to the
// document the copy was read from, and only while that copy is current. The
// extension answers every edit; the copy takes the edit once it is applied,
// and the view updates when the server's fresh model arrives.
import type { EditResult, Host, NoteKind, SourceEdit } from "./host.ts";
import { parseViewModel, type ViewModel } from "./model.ts";

/** Messages from the extension. */
type Inbound =
  | {
      type: "model";
      model: unknown;
      /** The project root the model's file paths are relative to. */
      project: string;
      sources: Record<string, string>;
      /** The revision of these copies, which an edit on one names. */
      revision: number;
      main: boolean;
    }
  | { type: "edited"; id: number; ok: true }
  | { type: "edited"; id: number; ok: false; message: string }
  | { type: "note"; message: string | null };

/** Messages to the extension. */
type Outbound =
  | { type: "ready" }
  | {
      type: "edit";
      id: number;
      project: string;
      file: string;
      revision: number;
      start: number;
      end: number;
      text: string;
    }
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

/** A file as this webview holds it, and the revision of the copy it started from. */
interface Copy {
  text: string;
  revision: number;
}

/** The key of a copy: two projects may both have `src/main.gcl`. */
function copyKey(project: string, file: string): string {
  return JSON.stringify([project, file]);
}

export class VsCodeHost implements Host {
  readonly modules: readonly string[] = [];
  readonly edited = false;
  private readonly api = acquireVsCodeApi();
  private readonly models = new Map<string, ViewModel>();
  /** The project each model came from; its file paths are relative to that root. */
  private readonly projects = new WeakMap<ViewModel, string>();
  private readonly copies = new Map<string, Copy>();
  /** Edits sent and not answered yet, by id. */
  private readonly pending = new Map<number, (result: EditResult) => void>();
  private edits = 0;
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
        this.projects.set(model, message.project);
        for (const [file, text] of Object.entries(message.sources)) {
          this.copies.set(copyKey(message.project, file), { text, revision: message.revision });
        }
        if (message.main && this.mainModule !== model.module.id) {
          this.mainModule = model.module.id;
          for (const listener of this.openListeners) listener(model.module.id);
        } else {
          for (const listener of this.modelListeners) listener([model]);
        }
        return;
      }
      case "edited": {
        const settle = this.pending.get(message.id);
        this.pending.delete(message.id);
        settle?.(message.ok ? { ok: true, models: [] } : { ok: false, message: message.message });
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

  source(model: ViewModel, file: string): string | null {
    const project = this.projects.get(model);
    if (project === undefined) return null;
    return this.copies.get(copyKey(project, file))?.text ?? null;
  }

  edit(model: ViewModel, edit: SourceEdit): Promise<EditResult> {
    const project = this.projects.get(model);
    const key = project === undefined ? null : copyKey(project, edit.file);
    const copy = key === null ? undefined : this.copies.get(key);
    if (project === undefined || key === null || copy === undefined) {
      return Promise.resolve({ ok: false, message: `unknown file ${edit.file}` });
    }
    const { start, end, text } = minimalEdit(copy.text, edit.next);
    this.edits += 1;
    const id = this.edits;
    return new Promise((resolve) => {
      this.pending.set(id, (result) => {
        // Applied: the copy is the edited text, unless a fresh copy came first.
        if (result.ok && this.copies.get(key) === copy) {
          this.copies.set(key, { text: edit.next, revision: copy.revision });
        }
        resolve(result);
      });
      this.api.postMessage({
        type: "edit",
        id,
        project,
        file: edit.file,
        revision: copy.revision,
        start,
        end,
        text,
      });
    });
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
