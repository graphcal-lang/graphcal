// Panes: the sheets open beside the main one, and where they go. A pane shows
// one module as read by the main sheet: an include instance (`eps.gcl as eps`)
// or an imported module. The main sheet is the rightmost column; a pane sits
// one column left of its deepest open consumer, and a module read from two
// places opens once.
import type { ViewModel } from "./model.ts";

export interface PaneKey {
  module: string;
  /** The include instance, or null for an imported module. */
  instance: string | null;
}

export function sameKey(a: PaneKey, b: PaneKey): boolean {
  return a.module === b.module && a.instance === b.instance;
}

/** The panes a module reads from: one per include instance, one per imported module. */
export function references(model: ViewModel): PaneKey[] {
  const out: PaneKey[] = [];
  const add = (key: PaneKey): void => {
    if (!out.some((k) => sameKey(k, key))) out.push(key);
  };
  for (const node of model.nodes) {
    if (node.kind === "external") add({ module: node.module, instance: node.instance });
  }
  for (const index of model.indexes) {
    if (index.module !== model.module.id) add({ module: index.module, instance: null });
  }
  return out;
}

export interface OpenSheet {
  key: PaneKey;
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
        if (j !== i && refs[j].some((k) => sameKey(k, sheet.key))) d = Math.max(d, depth[j] + 1);
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
