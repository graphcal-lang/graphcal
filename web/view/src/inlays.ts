// The sheet's values as the code pane shows them: one inlay at the end of
// each declaring line of the shown file. Pure.
import { axisName, entryName, type Axis } from "./layout.ts";
import type { ViewModel } from "./model.ts";
import type { Selection } from "./selection.ts";
import { displayOf, valueAt } from "./values.ts";

/** A value shown at the end of a line: the line is 1-based, as the model counts. */
export interface Inlay {
  line: number;
  text: string;
  /** Whether this is the line of the node selected in the sheet. */
  selected: boolean;
}

/**
 * `= 5 W` for a scalar; `[Mode]` for an indexed value, or `[Mode] · Safe = 5 W`
 * for the node whose cell is selected.
 */
export function inlaysOf(
  model: ViewModel,
  axes: ReadonlyMap<string, Axis>,
  selection: Selection | null,
): Inlay[] {
  const file = model.module.file;
  const inlays: Inlay[] = [];
  for (const node of model.nodes) {
    if (node.source === null || node.source.file !== file) continue;
    const picked = selection !== null && selection.id === node.id;
    let text: string;
    if (node.axes.length === 0) {
      text = `= ${displayOf(node.value)}`;
    } else {
      const names = `[${node.axes.map((axis) => axisName(axes, axis)).join(", ")}]`;
      const keys = picked
        ? [selection.row, selection.sub].filter((x): x is string => x !== null)
        : [];
      const entries = keys.map((key, i) => entryName(axes, node.axes[i] ?? "", key));
      text =
        keys.length > 0
          ? `${names} · ${entries.join(", ")} = ${displayOf(valueAt(node.value, keys))}`
          : names;
    }
    inlays.push({ line: node.source.line, text, selected: picked });
  }
  return inlays;
}
