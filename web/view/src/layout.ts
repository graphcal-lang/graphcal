import type { ViewModel, ViewNode } from "./model.ts";

// Layout grammar: x = role (const → param → node → outputs), y = shape.
// A band is one unrolled axis (or none); every node whose first axis is that
// axis becomes a column of the band. Remaining axes become sub-columns.

export type Role = "const" | "param" | "node" | "output";
export const ROLES: readonly Role[] = ["const", "param", "node", "output"];

export interface Column {
  node: ViewNode;
  role: Role;
  depth: number;
  /** Axes after the band's unrolled axis; rendered as sub-columns. */
  subAxes: string[];
}

export interface Band {
  /** Unrolled axis, or null for the no-axis band. */
  axis: string | null;
  labels: string[];
  columns: Column[];
}

export interface Layout {
  bands: Band[];
  columns: Map<string, Column>;
  /** Direct consumers of each node id within the module. */
  users: Map<string, string[]>;
}

function roleOf(node: ViewNode, hasUsers: boolean): Role {
  switch (node.kind) {
    case "const":
      return "const";
    case "param":
    case "external":
      return "param";
    case "assert":
      return "output";
    case "node":
      return node.pub || !hasUsers ? "output" : "node";
  }
}

function computeDepths(model: ViewModel): Map<string, number> {
  const byId = new Map(model.nodes.map((n) => [n.id, n]));
  const depths = new Map<string, number>();
  const visiting = new Set<string>();
  const visit = (id: string): number => {
    const known = depths.get(id);
    if (known !== undefined) return known;
    if (visiting.has(id)) return 0; // cycles are rejected by the compiler; stay total anyway
    visiting.add(id);
    const node = byId.get(id);
    let depth = 0;
    if (node) {
      for (const dep of node.deps) {
        if (byId.has(dep)) depth = Math.max(depth, visit(dep) + 1);
      }
    }
    visiting.delete(id);
    depths.set(id, depth);
    return depth;
  };
  for (const node of model.nodes) visit(node.id);
  return depths;
}

export function computeLayout(model: ViewModel): Layout {
  const users = new Map<string, string[]>();
  for (const node of model.nodes) {
    for (const dep of node.deps) {
      const list = users.get(dep) ?? [];
      list.push(node.id);
      users.set(dep, list);
    }
  }
  const depths = computeDepths(model);
  const labelsOf = new Map(model.indexes.map((i) => [i.name, i.labels]));
  const order = new Map(model.nodes.map((n, i) => [n.id, i]));

  const bands = new Map<string | null, Band>();
  const columns = new Map<string, Column>();
  for (const node of model.nodes) {
    const axis = node.axes[0] ?? null;
    let band = bands.get(axis);
    if (!band) {
      band = { axis, labels: axis ? (labelsOf.get(axis) ?? []) : [], columns: [] };
      bands.set(axis, band);
    }
    const column: Column = {
      node,
      role: roleOf(node, (users.get(node.id) ?? []).length > 0),
      depth: depths.get(node.id) ?? 0,
      subAxes: node.axes.slice(1),
    };
    band.columns.push(column);
    columns.set(node.id, column);
  }

  const roleRank = new Map(ROLES.map((r, i) => [r, i]));
  for (const band of bands.values()) {
    band.columns.sort((a, b) => {
      const byRole = (roleRank.get(a.role) ?? 0) - (roleRank.get(b.role) ?? 0);
      if (byRole !== 0) return byRole;
      if (a.depth !== b.depth) return a.depth - b.depth;
      return (order.get(a.node.id) ?? 0) - (order.get(b.node.id) ?? 0);
    });
  }

  // No-axis band first, then axis bands in declaration order of their indexes.
  const axisOrder = new Map(model.indexes.map((i, k) => [i.name, k]));
  const sorted = [...bands.values()].sort((a, b) => {
    if (a.axis === null) return -1;
    if (b.axis === null) return 1;
    return (axisOrder.get(a.axis) ?? 0) - (axisOrder.get(b.axis) ?? 0);
  });
  return { bands: sorted, columns, users };
}
