// The GitHub host's in-memory copy of a project's sources, by revision.
// Nothing here is ever written back: the sheet is a live, unsaved what-if
// over the sources the host read. Every applied edit makes a new revision
// of the whole project; each file keeps the history its late edits are
// placed through.
import type { ProjectFile } from "./engine.ts";
import { historyOf, type FileHistory, type Revised } from "./revisions.ts";

/** The engine's request for files given as path and text. */
export function requestOf(files: Iterable<[string, string]>): ProjectFile[] {
  return [...files].map(([path, content]) => ({ path, content }));
}

export class Sources {
  /** The revision of the project as a whole. */
  revision: number;
  private readonly histories = new Map<string, FileHistory>();

  constructor(files: Iterable<[string, string]>, revision: number) {
    this.revision = revision;
    for (const [path, text] of files) this.histories.set(path, historyOf(text, revision));
  }

  /** Every file's current text. */
  texts(): Map<string, string> {
    return new Map([...this.histories].map(([path, history]) => [path, history.text]));
  }

  /** A file's current text at the project's revision. */
  source(file: string): Revised | null {
    const history = this.histories.get(file);
    return history === undefined ? null : { text: history.text, revision: this.revision };
  }

  history(file: string): FileHistory | null {
    return this.histories.get(file) ?? null;
  }

  /** The engine's request, with `file` read as `history` says when given (an edit being tried). */
  request(change?: { file: string; history: FileHistory }): ProjectFile[] {
    return requestOf(
      [...this.histories].map(([path, history]): [string, string] => [
        path,
        change?.file === path ? change.history.text : history.text,
      ]),
    );
  }

  /** Keep an edit: `file` is now as `history` says, and the project at its revision. */
  commit(file: string, history: FileHistory): void {
    this.histories.set(file, history);
    this.revision = history.revision;
  }
}
