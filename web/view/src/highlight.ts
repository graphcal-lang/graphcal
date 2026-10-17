// What a sheet lights: the selected node and its trace (columns, related
// cells, wires), the changes since the base ref, and the rows picked in a
// ruler. The matching of related cells is pure; the rest puts classes on the
// rendered sheet.
import { markChangedWires } from "./compareMarks.ts";
import type { ModelDiff } from "./compare.ts";
import type { ViewNode } from "./model.ts";
import type { Sheet } from "./sheet.ts";
import { trace, type Selection } from "./selection.ts";

/** The selected cell's entry key per axis of the selected node. */
export function selectedEntries(
  selection: Selection,
  node: ViewNode | undefined,
): Map<string, string> {
  const entries = new Map<string, string>();
  if (!node) return entries;
  const [first, second] = node.axes;
  if (first !== undefined && selection.row !== null) entries.set(first, selection.row);
  if (second !== undefined && selection.sub !== null) entries.set(second, selection.sub);
  return entries;
}

/**
 * Whether a cell of a related node (its entry key per axis) relates to the
 * selected cell: it agrees on every axis the two nodes share, and they share
 * one (a sum over Component still shares Mode).
 */
export function relatesTo(
  selected: ReadonlyMap<string, string>,
  axes: readonly string[],
  cell: (axis: string) => string | undefined,
): boolean {
  const shared = axes.filter((axis) => selected.has(axis));
  return shared.length > 0 && shared.every((axis) => cell(axis) === selected.get(axis));
}

function clearHighlights(s: Sheet): void {
  for (const n of s.viewEl.querySelectorAll<HTMLElement>(
    ".sel, .rel-direct, .rel-all, .row-sel, .rel-cell",
  )) {
    n.classList.remove("sel", "rel-direct", "rel-all", "row-sel", "rel-cell");
  }
  for (const w of s.wiresEl.querySelectorAll<SVGElement>(".wire, .wire-dot")) {
    w.classList.remove("lit", "direct");
    if (w.classList.contains("wire")) w.setAttribute("marker-end", "url(#arrow)");
  }
}

/** Light the sheet's selection and its trace; `diff` lights the changed wires under it. */
export function applySelection(s: Sheet, diff: ModelDiff | null): void {
  clearHighlights(s);
  if (diff) markChangedWires(s.wiresEl, diff);
  const { selection, model, layout, viewEl, wiresEl } = s;
  if (!selection) return;

  const result = trace(selection, model, layout);
  const entries = selectedEntries(
    selection,
    model.nodes.find((n) => n.id === selection.id),
  );
  const mark = (id: string, cls: string): void => {
    const column = layout.columns.get(id);
    const col = viewEl.querySelector<HTMLElement>(`.col[data-node="${id}"]`);
    if (!col || !column) return;
    col.classList.add(cls);
    if (id === selection.id) return;
    const [first, second] = column.node.axes;
    for (const cell of col.querySelectorAll<HTMLElement>(".cell")) {
      const keyOf = (axis: string): string | undefined =>
        axis === first ? cell.dataset.row : axis === second ? cell.dataset.sub : undefined;
      if (relatesTo(entries, column.node.axes, keyOf)) cell.classList.add("rel-cell");
    }
  };
  mark(selection.id, "sel");
  for (const id of result.all) mark(id, result.direct.has(id) ? "rel-direct" : "rel-all");
  if (selection.row !== null) {
    const band = viewEl.querySelector(
      `.band[data-band="${layout.columns.get(selection.id)?.bandIndex}"]`,
    );
    band?.querySelector(`.row-label[data-row="${selection.row}"]`)?.classList.add("row-sel");
    if (selection.sub !== null) {
      viewEl
        .querySelector(
          `.cell[data-node="${selection.id}"][data-row="${selection.row}"][data-sub="${selection.sub}"]`,
        )
        ?.classList.add("sel");
    } else {
      viewEl
        .querySelector(`.cell[data-node="${selection.id}"][data-row="${selection.row}"]`)
        ?.classList.add("sel");
    }
  }
  for (const w of wiresEl.querySelectorAll<SVGElement>(".wire, .wire-dot")) {
    const { from, to } = w.dataset;
    if (from === undefined || to === undefined) continue;
    const direct = result.edges.get(from, to);
    if (direct === undefined) continue;
    w.classList.remove("diff");
    w.classList.add("lit");
    if (direct) w.classList.add("direct");
    if (w.classList.contains("wire")) w.setAttribute("marker-end", "url(#arrow-lit)");
  }
}

function bandElementOf(s: Sheet, axis: string): HTMLElement | null {
  const index = s.layout.bands.find((b) => b.axis === axis)?.index;
  return index === undefined ? null : s.viewEl.querySelector(`.band[data-band="${index}"]`);
}

/** Picked row labels: the label and the whole row across the band, like a sheet. */
export function applyPicked(s: Sheet): void {
  for (const n of s.viewEl.querySelectorAll<HTMLElement>(".picked, .row-picked")) {
    n.classList.remove("picked", "row-picked");
  }
  const picked = s.picked;
  if (!picked) return;
  for (const label of picked.labels) {
    for (const n of s.viewEl.querySelectorAll<HTMLElement>(`.row-label[data-row="${label}"]`)) {
      if (n.dataset.axis === picked.axis) n.classList.add("picked");
    }
    const band = bandElementOf(s, picked.axis);
    for (const cell of band?.querySelectorAll<HTMLElement>(`.cell[data-row="${label}"]`) ?? []) {
      cell.classList.add("row-picked");
    }
  }
}
