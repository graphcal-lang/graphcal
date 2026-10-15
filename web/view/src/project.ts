// In-memory copy of a project's sources, and the textual edits the view
// makes to them. Nothing here is ever written back: the sheet is a live,
// unsaved what-if over the sources the host read.
import type { ProjectFile } from "./engine.ts";
import type { ViewModel } from "./model.ts";

export { EditError, replaceExpr, replaceRange, replaceTableCell } from "./source.ts";

export class Project {
  readonly packageName: string;
  readonly files = new Map<string, string>();
  constructor(packageName: string, files: Iterable<[string, string]>) {
    this.packageName = packageName;
    for (const [path, content] of files) this.files.set(path, content);
  }

  /** `pkg.a.b` lives at `src/pkg/a/b.gcl`; the entry module is the model's file. */
  fileOf(model: ViewModel, module: string): string | null {
    if (module === model.module.id) return model.module.file;
    const path = `src/${module.split(".").join("/")}.gcl`;
    return this.files.has(path) ? path : null;
  }

  request(): ProjectFile[] {
    return [...this.files].map(([path, content]) => ({ path, content }));
  }
}
