// The changes since the base ref, put on a rendered sheet: frames on
// columns, tint on cells, `was` under boxes, green wires between changed nodes.
import { previousValue, type ModelDiff } from "./compare.ts";
import type { ViewModel } from "./model.ts";
import { displayOf } from "./values.ts";

/**
 * Light the wires between changed nodes in green, the way a selection lights
 * its trace in blue: a change travels down them to the values it altered. A
 * wire the selection lit keeps its blue.
 */
export function markChangedWires(wiresEl: SVGSVGElement, diff: ModelDiff): void {
  for (const w of wiresEl.querySelectorAll<SVGElement>(".wire, .wire-dot")) {
    const { from, to } = w.dataset;
    if (from === undefined || to === undefined) continue;
    if (!diff.nodes.has(from) || !diff.nodes.has(to) || w.classList.contains("lit")) continue;
    w.classList.add("diff");
    if (w.classList.contains("wire")) w.setAttribute("marker-end", "url(#arrow-diff)");
  }
}

/** Put the diff's classes on a rendered sheet: frames on columns, tint on cells, `was` under boxes. */
export function markChanges(viewEl: HTMLElement, model: ViewModel, diff: ModelDiff): void {
  const byId = new Map(model.nodes.map((n) => [n.id, n]));
  for (const col of viewEl.querySelectorAll<HTMLElement>(".col[data-node]")) {
    const id = col.dataset.node ?? "";
    const change = diff.nodes.get(id);
    const node = byId.get(id);
    if (!change || !node) continue;
    col.classList.add(`diff-${change.kind}`);
    const title = col.querySelector(".col-title");
    if (change.kind === "added" && title) {
      const tag = document.createElement("span");
      tag.className = "diff-tag";
      tag.textContent = "new";
      title.append(tag);
    }
    if (change.kind === "added" || change.kind === "downstream") continue;
    for (const cell of col.querySelectorAll<HTMLElement>(".cell[data-row]")) {
      const labels = [cell.dataset.row, cell.dataset.sub].filter(
        (x): x is string => x !== undefined,
      );
      const was = previousValue(change, node.value, labels);
      if (was === undefined) continue;
      cell.classList.add("diff-cell");
      cell.title = `was ${displayOf(was)}`;
    }
    const box = col.querySelector<HTMLElement>(".box-value");
    const was = previousValue(change, node.value, []);
    if (box && was !== undefined) {
      const note = document.createElement("span");
      note.className = "was";
      note.textContent = `was ${displayOf(was)}`;
      box.append(note);
    }
  }
}
