// In-memory copy of a fixture project's sources, and the textual edits the
// view makes to them. Nothing here is ever written back to disk: the sheet is
// a live, unsaved what-if over the sources the fixture was built from.
import type { ProjectFile } from "./engine.ts";
import type { ViewModel } from "./model.ts";

export { EditError, replaceExpr, replaceTableCell } from "./source.ts";

const sources = import.meta.glob<string>("../fixtures/*/**/*.{gcl,toml}", {
  query: "?raw",
  import: "default",
  eager: true,
});

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

/** Find the bundled fixture project whose manifest names the model's package. */
export function projectFor(model: ViewModel): Project | null {
  const byDir = new Map<string, Map<string, string>>();
  for (const [key, content] of Object.entries(sources)) {
    const m = /^\.\.\/fixtures\/([^/]+)\/(.+)$/.exec(key);
    if (!m) continue;
    const dir = m[1] ?? "";
    const files = byDir.get(dir) ?? new Map<string, string>();
    files.set(m[2] ?? "", content);
    byDir.set(dir, files);
  }
  for (const files of byDir.values()) {
    const manifest = files.get("graphcal.toml") ?? "";
    const name = /name\s*=\s*"([^"]+)"/.exec(manifest)?.[1];
    if (name === model.module.package && files.has(model.module.file)) {
      return new Project(name, files);
    }
  }
  return null;
}
