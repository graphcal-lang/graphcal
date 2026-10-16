// What changed between two models of the same file (the sheet at `base`
// and at `head`), and how the sheet shows it: every changed node in a green
// frame, new ones tagged, cells whose value changed tinted, removed ones
// named. A node counts as changed in its expression when its definition
// differs (text, kind, axes or type), and in its value only when just the
// value differs (something upstream changed). Pure: compareMarks.ts puts
// the result on a rendered sheet.
import type { Value, ViewModel, ViewNode } from "./model.ts";
import { displayOf, oneLine, valueAt } from "./values.ts";

export type NodeChange =
  | { kind: "added" }
  | { kind: "expr"; before: ViewNode }
  | { kind: "value"; before: ViewNode }
  /** Reads a changed node, directly or through others, but came out the same. */
  | { kind: "downstream"; before: ViewNode };

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
    a.origin.kind === b.origin.kind &&
    a.dimension === b.dimension &&
    a.unit === b.unit &&
    a.axes.length === b.axes.length &&
    a.axes.every((axis, i) => axis === b.axes[i]) &&
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
  // Everything downstream of a change is part of it, as a selection's trace
  // reaches every dependent: a node whose inputs changed is lit even when its
  // own value came out the same.
  let grew = true;
  while (grew) {
    grew = false;
    for (const node of head.nodes) {
      if (nodes.has(node.id) || !node.deps.some((dep) => nodes.has(dep))) continue;
      const old = before.get(node.id);
      if (old) {
        nodes.set(node.id, { kind: "downstream", before: old });
        grew = true;
      }
    }
  }
  const ids = new Set(head.nodes.map((n) => n.id));
  return { nodes, removed: base.nodes.filter((n) => !ids.has(n.id)) };
}

/** `+2 · ~3 · −1 (x)`: the summary for a pane head. */
export function summarize(diff: ModelDiff): string {
  let added = 0;
  let changed = 0;
  let downstream = 0;
  for (const change of diff.nodes.values()) {
    if (change.kind === "added") added += 1;
    else if (change.kind === "downstream") downstream += 1;
    else changed += 1;
  }
  const parts: string[] = [];
  if (added > 0) parts.push(`${added} new`);
  if (changed > 0) parts.push(`${changed} changed`);
  if (downstream > 0) parts.push(`${downstream} downstream`);
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
  if (!change || change.kind === "added" || change.kind === "downstream") return undefined;
  const before = valueAt(change.before.value, labels);
  return sameValue(before, valueAt(value, labels)) ? undefined : before;
}

/** What the formula bar says about a changed node's cell at `keys`. */
export function changeNote(
  change: NodeChange,
  value: Value | null,
  keys: readonly string[],
): string {
  if (change.kind === "added") return "new since the base";
  if (change.kind === "downstream") return "reads a change; value unchanged";
  const was = previousValue(change, value, keys);
  if (was !== undefined) return `was ${displayOf(was)}`;
  return change.kind === "expr"
    ? `was ${oneLine(change.before.expr ?? "")}`
    : "value unchanged here";
}
