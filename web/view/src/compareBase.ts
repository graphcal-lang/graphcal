// The base side of a comparison, from what was read at the base ref: which
// repository files differ, and per project file its text and its model, or
// why the base could not give them. Pure: the host reads, this assembles.
import type { TreeEntry } from "./github.ts";
import type { BaseSide, Compare } from "./host.ts";
import type { ViewModel } from "./model.ts";

/** Repository paths whose content differs between the two trees (added, removed or changed). */
export function changedPaths(head: readonly TreeEntry[], base: readonly TreeEntry[]): Set<string> {
  const headSha = new Map(head.map((e) => [e.path, e.sha]));
  const baseSha = new Map(base.map((e) => [e.path, e.sha]));
  const changed = new Set<string>();
  for (const [path, sha] of headSha) if (baseSha.get(path) !== sha) changed.add(path);
  for (const path of baseSha.keys()) if (!headSha.has(path)) changed.add(path);
  return changed;
}

/** What was read at the base ref. */
export type BaseRead =
  /** The base as a whole could not be read (no such ref, a tree too large, a project the engine refuses). */
  | { kind: "failed"; message: string }
  | {
      kind: "read";
      changedFiles: ReadonlySet<string>;
      /** Base texts by project file; a file not here is new. */
      sources: ReadonlyMap<string, string>;
      /** Base models by project file: a model, or why the file did not evaluate there. */
      models: ReadonlyMap<string, BaseSide<ViewModel>>;
    };

/** The comparison against `ref`, from what was read there. */
export function compareOf(ref: string, read: BaseRead): Compare {
  if (read.kind === "failed") {
    const failed = { kind: "failed", message: read.message } as const;
    return { ref, model: () => failed, source: () => failed, changedFiles: new Set() };
  }
  return {
    ref,
    model: (head) => read.models.get(head.module.file) ?? { kind: "absent" },
    source: (file) => {
      const text = read.sources.get(file);
      return text === undefined ? { kind: "absent" } : { kind: "present", value: text };
    },
    changedFiles: read.changedFiles,
  };
}
