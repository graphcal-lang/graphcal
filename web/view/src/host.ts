// The host a view runs in: where its models come from and where its edits go.
// The dev shell is hosted by bundled fixtures and the in-browser engine; the
// VS Code webview is hosted by the extension, which talks to the language
// server. The sheet code only ever sees this interface.
import type { ViewModel } from "./model.ts";

/** A file of a module's project rewritten in full. */
export interface SourceEdit {
  file: string;
  next: string;
}

export type EditResult =
  /** Applied; `models` are the modules the host could re-project at once. */
  | { ok: true; models: ViewModel[] }
  /** Refused or failed; nothing changed. */
  | { ok: false; message: string };

export interface Host {
  /** Modules the host offers in a picker, by id; empty when the host chooses what is shown. */
  readonly modules: readonly string[];
  /** Whether the shown sources carry unsaved what-if edits the host can discard. */
  readonly edited: boolean;
  /** The current model of a module, if the host has one. */
  model(module: string): ViewModel | null;
  /** The text of a file of the model's project, when the host has it (edits need it). */
  source(model: ViewModel, file: string): string | null;
  /**
   * Apply an edit. `open` lists the modules on screen so the host can
   * re-project them; a host that updates asynchronously returns no models
   * and pushes them through `onModels` later.
   */
  edit(model: ViewModel, edit: SourceEdit, open: readonly string[]): Promise<EditResult>;
  /** Ask for a module the host does not have yet (to pin a pane); it arrives through `onModels`. */
  request(module: string): void;
  /** Discard what-if edits, when the host keeps any. */
  reset(): void;
  /** The main sheet now shows `module`; a host may reflect it (in the address, say). */
  shown?(module: string): void;
  /** Fresh models, pushed by the host: after an edit, an external change, or a request. */
  onModels(listener: (models: ViewModel[]) => void): void;
  /** The host asks to show another module as the main sheet. */
  onOpen(listener: (module: string) => void): void;
  /** The host has something to say in the formula bar's note slot; `busy` is progress, not a fault. */
  onNote(listener: (message: string | null, kind: NoteKind) => void): void;
}

export type NoteKind = "error" | "busy";
