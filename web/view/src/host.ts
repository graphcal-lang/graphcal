// The host a view runs in: where its models come from and where its edits go.
// The web page is hosted by a GitHub repository and the in-browser engine; the
// VS Code webview is hosted by the extension, which talks to the language
// server. The sheet code only ever sees this interface.
import type { Patch } from "./diff.ts";
import type { ViewModel } from "./model.ts";

/** A file of a module's project rewritten in full. */
export interface SourceEdit {
  file: string;
  next: string;
}

export type EditResult =
  /**
   * Applied; `models` are the modules the host could re-project at once.
   * `note` says why the shown ones have no values, when they have none.
   */
  | { ok: true; models: ViewModel[]; note: string | null }
  /** Refused or failed; nothing changed. */
  | { ok: false; message: string };

export interface Host {
  /** Modules the host offers in a picker, by id; empty when the host chooses what is shown. */
  readonly modules: readonly string[];
  /** Every file the explorer lists, in the host's own paths (a whole repository, say). */
  readonly files?: readonly string[];
  /** Open a listed `.gcl` file: the host shows it, reading another project if it must. False when it cannot. */
  openFile?(path: string): boolean;
  /** The listed path of a module's file, to mark it in the explorer. */
  pathOf?(module: string): string | null;
  /** The what-if edits as a patch to carry elsewhere (`git apply`, the VS Code extension). */
  patch?(): Patch;
  /** What the shown ref is compared against, when the address names a base; null until it is read. */
  readonly compare?: Compare | null;
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

/** One thing as it was at the base ref. */
export type BaseSide<T> =
  /** Not there at the base: everything in it is new. */
  | { kind: "absent" }
  /** The base could not be read or did not evaluate: nothing can be marked as changed. */
  | { kind: "failed"; message: string }
  | { kind: "present"; value: T };

/** The base side of a comparison: the same project as read at another ref. */
export interface Compare {
  /** The base ref. */
  ref: string;
  /** The base model of the module `head` shows, found by its file. */
  model(head: ViewModel): BaseSide<ViewModel>;
  /** The base text of a project file. */
  source(file: string): BaseSide<string>;
  /** Repository paths whose content differs between the two refs (added, removed or changed). */
  changedFiles: ReadonlySet<string>;
}
