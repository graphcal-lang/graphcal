// What changed between two models of the same file (the sheet at `base`
// and at `head`), and how the sheet shows it: every changed node in a green
// frame, new ones tagged, cells whose value changed tinted, removed ones
// named. A node counts as changed in its expression when its definition
// differs (text, kind, axes or type), and in its value only when just the
// value differs (something upstream changed).
import type { Value, ViewModel, ViewNode } from "./model.ts";
import { displayOf, oneLine, valueAt } from "./values.ts";

export type NodeChange =
  | { kind: "added" }
  | { kind: "expr"; before: ViewNode }
  | { kind: "value"; before: ViewNode };

export interface ModelDiff {
  nodes: Map<string, NodeChange>;
  removed: ViewNode[];
}

function sameValue(a: Value | null, b: Value | null): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

function sameDefinition(a: ViewNode, b: ViewNode): boolean {
  return (
    a.kind === b.kind &&
    a.dimension === b.dimension &&
    a.unit === b.unit &&
    a.axes.join("\0") === b.axes.join("\0") &&
    (a.expr === null ? null : oneLine(a.expr)) === (b.expr === null ? null : oneLine(b.expr))
  );
}

/** The changes from `base` to `head`; with no base (a new file) every node is added. */
export function diffModels(base: ViewModel | null, head: ViewModel): ModelDiff {
  const nodes = new Map<string, NodeChange>();
  if (base === null) {
    for (const node of head.nodes) nodes.set(node.id, { kind: "added" });
    return { nodes, removed: [] };
  }
  const before = new Map(base.nodes.map((n) => [n.id, n]));
  for (const node of head.nodes) {
    const old = before.get(node.id);
    if (!old) nodes.set(node.id, { kind: "added" });
    else if (!sameDefinition(old, node)) nodes.set(node.id, { kind: "expr", before: old });
    else if (!sameValue(old.value, node.value)) nodes.set(node.id, { kind: "value", before: old });
  }
  const ids = new Set(head.nodes.map((n) => n.id));
  return { nodes, removed: base.nodes.filter((n) => !ids.has(n.id)) };
}

/** `+2 · ~3 · −1 (x)`: the summary for a pane head. */
export function summarize(diff: ModelDiff): string {
  let added = 0;
  let changed = 0;
  for (const change of diff.nodes.values()) {
    if (change.kind === "added") added += 1;
    else changed += 1;
  }
  const parts: string[] = [];
  if (added > 0) parts.push(`${added} new`);
  if (changed > 0) parts.push(`${changed} changed`);
  if (diff.removed.length > 0) {
    parts.push(`${diff.removed.length} removed (${diff.removed.map((n) => n.name).join(", ")})`);
  }
  return parts.length === 0 ? "no change" : parts.join(" · ");
}

/** The base value of the node's cell at `labels`, when it differs from the head's; else null. */
export function previousValue(
  change: NodeChange | undefined,
  value: Value | null,
  labels: readonly string[],
): Value | null | undefined {
  if (!change || change.kind === "added") return undefined;
  const before = valueAt(change.before.value, labels);
  return sameValue(before, valueAt(value, labels)) ? undefined : before;
}

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
    if (change.kind === "added") continue;
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
