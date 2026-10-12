import type { Layout } from "./layout.ts";
import type { ViewModel } from "./model.ts";

export type TraceMode = "deps" | "uses";

export interface Selection {
  id: string;
  mode: TraceMode;
}

export interface Trace {
  /** Nodes one step away. */
  direct: Set<string>;
  /** Every node reachable, including the direct ones. */
  all: Set<string>;
  /** Edges on the traced paths, keyed `${from}->${to}`, with direct edges flagged. */
  edges: Map<string, boolean>;
}

export function trace(selection: Selection, model: ViewModel, layout: Layout): Trace {
  const byId = new Map(model.nodes.map((n) => [n.id, n]));
  const next = (id: string): string[] =>
    selection.mode === "deps"
      ? (byId.get(id)?.deps ?? []).filter((d) => layout.columns.has(d))
      : (layout.users.get(id) ?? []);
  const direct = new Set(next(selection.id));
  const all = new Set<string>();
  const edges = new Map<string, boolean>();
  const queue = [selection.id];
  while (queue.length > 0) {
    const current = queue.shift() as string;
    for (const neighbour of next(current)) {
      const key =
        selection.mode === "deps" ? `${neighbour}->${current}` : `${current}->${neighbour}`;
      edges.set(key, current === selection.id);
      if (!all.has(neighbour)) {
        all.add(neighbour);
        queue.push(neighbour);
      }
    }
  }
  return { direct, all, edges };
}
