// The base side of a comparison, from what was read at the base ref: which
// repository files differ, and per project file its text and its model, or
// why the base could not give them. Pure: the host reads, this assembles.
import type { BaseSide, Compare } from "./host.ts";
import type { ViewModel } from "./model.ts";

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
    model: (head) =>
      // The base models are of modules alone: an instance has none to compare with.
      head.target.kind === "instance"
        ? { kind: "failed", message: "an include instance is not compared" }
        : (read.models.get(head.module.file) ?? { kind: "absent" }),
    source: (file) => {
      const text = read.sources.get(file);
      return text === undefined ? { kind: "absent" } : { kind: "present", value: text };
    },
    changedFiles: read.changedFiles,
  };
}
