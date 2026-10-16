// Panes: the sheets open beside the main one, and where they go. A pane shows
// what a sheet reads: an include instance as the root evaluates it
// (`eps.gcl as eps`, with the include's arguments) or an imported module
// alone. The main sheet is the rightmost column; a pane sits one column left
// of its deepest open reader, and a target read from two places opens once.
import { isExternal, type ViewModel } from "./model.ts";
import { moduleTarget, readTarget, sameTarget, targetOf, type ViewTarget } from "./target.ts";

/** The targets a model reads from: one per include instance, one per imported module. */
export function references(model: ViewModel): ViewTarget[] {
  const reader = targetOf(model);
  const out: ViewTarget[] = [];
  const add = (target: ViewTarget | null): void => {
    if (target !== null && !out.some((t) => sameTarget(t, target))) out.push(target);
  };
  for (const node of model.nodes) {
    if (isExternal(node)) add(readTarget(reader, node));
  }
  for (const index of model.indexes) {
    if (index.module !== null && index.module !== model.module.id) {
      add(moduleTarget(index.module));
    }
  }
  return out;
}

export interface OpenSheet {
  key: ViewTarget;
  model: ViewModel;
}

/**
 * Column depth of each open sheet: 0 for the main sheet (the first), otherwise
 * one more than the deepest sheet that reads it, or 1 when nothing open does.
 */
export function depthsOf(sheets: OpenSheet[]): number[] {
  const refs = sheets.map((s) => references(s.model));
  const depth: number[] = sheets.map((_, i) => (i === 0 ? 0 : 1));
  // Includes cannot be cyclic, so this settles within one pass per column.
  for (let round = 0; round < sheets.length; round++) {
    let changed = false;
    sheets.forEach((sheet, i) => {
      if (i === 0) return;
      let d = 1;
      sheets.forEach((_, j) => {
        if (j !== i && refs[j].some((k) => sameTarget(k, sheet.key))) {
          d = Math.max(d, depth[j] + 1);
        }
      });
      if (d !== depth[i]) {
        depth[i] = d;
        changed = true;
      }
    });
    if (!changed) break;
  }
  return depth;
}
