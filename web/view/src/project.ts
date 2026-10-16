// In-memory copy of a project's sources, and the textual edits the view
// makes to them. Nothing here is ever written back: the sheet is a live,
// unsaved what-if over the sources the host read.
import type { ProjectFile } from "./engine.ts";

export class Project {
  readonly files = new Map<string, string>();
  constructor(files: Iterable<[string, string]>) {
    for (const [path, content] of files) this.files.set(path, content);
  }

  request(): ProjectFile[] {
    return [...this.files].map(([path, content]) => ({ path, content }));
  }
}
