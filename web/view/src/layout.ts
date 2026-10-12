import type { ViewModel, ViewNode } from "./model.ts";

// Layout grammar: x = role (const → param → node, with outputs as the right
// sub-zone of node), y = shape. An axis gets a band of its own only when two or
// more nodes share it as their first axis; otherwise the node stays in the
// no-axis band as a labelled box.

export type Role = "const" | "param" | "node" | "output";
export const ROLES: readonly Role[] = ["const", "param", "node", "output"];

export interface Column {
  node: ViewNode;
  role: Role;
  depth: number;
  bandIndex: number;
  /** Index among the band's columns in render order. */
  position: number;
  /** Row labels this column renders (band labels, or its own first axis). */
  rows: string[];
  /** Sub-column labels (second axis), if any. */
  subs: string[];
  /** Folded by default: a private intermediate with a single consumer. */
  foldedByDefault: boolean;
}

export interface Band {
  index: number;
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

  // An axis is unrolled only when shared as the first axis by several nodes.
  const firstAxisCount = new Map<string, number>();
  for (const node of model.nodes) {
    const axis = node.axes[0];
    if (axis !== undefined) firstAxisCount.set(axis, (firstAxisCount.get(axis) ?? 0) + 1);
  }
  const bandAxisOf = (node: ViewNode): string | null => {
    const axis = node.axes[0];
    return axis !== undefined && (firstAxisCount.get(axis) ?? 0) >= 2 ? axis : null;
  };

  const grouped = new Map<string | null, ViewNode[]>();
  for (const node of model.nodes) {
    const axis = bandAxisOf(node);
    grouped.set(axis, [...(grouped.get(axis) ?? []), node]);
  }
  const axisOrder = new Map(model.indexes.map((i, k) => [i.name, k]));
  const bandAxes = [...grouped.keys()].sort((a, b) => {
    if (a === null) return -1;
    if (b === null) return 1;
    return (axisOrder.get(a) ?? 0) - (axisOrder.get(b) ?? 0);
  });

  const roleRank = new Map(ROLES.map((r, i) => [r, i]));
  const bands: Band[] = [];
  const columns = new Map<string, Column>();
  bandAxes.forEach((axis, bandIndex) => {
    const nodes = [...(grouped.get(axis) ?? [])];
    const band: Band = {
      index: bandIndex,
      axis,
      labels: axis ? (labelsOf.get(axis) ?? []) : [],
      columns: [],
    };
    const roleOfNode = (node: ViewNode): Role =>
      roleOf(node, (users.get(node.id) ?? []).length > 0);
    nodes.sort((a, b) => {
      const byRole = (roleRank.get(roleOfNode(a)) ?? 0) - (roleRank.get(roleOfNode(b)) ?? 0);
      if (byRole !== 0) return byRole;
      const byDepth = (depths.get(a.id) ?? 0) - (depths.get(b.id) ?? 0);
      if (byDepth !== 0) return byDepth;
      return (order.get(a.id) ?? 0) - (order.get(b.id) ?? 0);
    });
    nodes.forEach((node, position) => {
      const ownAxis = axis === null ? node.axes[0] : undefined;
      const subAxis = axis === null ? node.axes[1] : node.axes[1];
      const consumers = users.get(node.id) ?? [];
      const column: Column = {
        node,
        role: roleOfNode(node),
        depth: depths.get(node.id) ?? 0,
        bandIndex,
        position,
        rows: axis !== null ? band.labels : ownAxis ? (labelsOf.get(ownAxis) ?? []) : [],
        subs: subAxis ? (labelsOf.get(subAxis) ?? []) : [],
        foldedByDefault:
          axis !== null &&
          node.kind === "node" &&
          !node.pub &&
          consumers.length === 1 &&
          bandAxisOf(model.nodes.find((n) => n.id === consumers[0]) ?? node) === axis,
      };
      band.columns.push(column);
      columns.set(node.id, column);
    });
    bands.push(band);
  });
  return { bands, columns, users };
}

export const BOX_GAP = 10;

/**
 * Split the no-axis band into tiers so every box fits within its zone's width.
 * Each tier becomes a band of its own, so wires between tiers use the channel
 * between them and never cross a box.
 */
export function splitScalarBand(
  layout: Layout,
  boxWidth: (id: string) => number,
  zoneWidth: Record<Role, number>,
): Layout {
  const scalar = layout.bands.find((b) => b.axis === null);
  if (!scalar) return layout;

  const tiersByRole = new Map<Role, Column[][]>();
  let tierCount = 1;
  for (const role of ROLES) {
    const rows: Column[][] = [];
    let current: Column[] = [];
    let width = 0;
    for (const column of scalar.columns.filter((c) => c.role === role)) {
      const w = boxWidth(column.node.id);
      if (current.length > 0 && width + BOX_GAP + w > zoneWidth[role]) {
        rows.push(current);
        current = [];
        width = 0;
      }
      width += (current.length > 0 ? BOX_GAP : 0) + w;
      current.push(column);
    }
    if (current.length > 0) rows.push(current);
    tiersByRole.set(role, rows);
    tierCount = Math.max(tierCount, rows.length);
  }

  const bands: Band[] = [];
  const columns = new Map<string, Column>();
  for (let tier = 0; tier < tierCount; tier++) {
    const band: Band = { index: bands.length, axis: null, labels: [], columns: [] };
    for (const role of ROLES) {
      for (const column of tiersByRole.get(role)?.[tier] ?? []) {
        const placed: Column = { ...column, bandIndex: band.index, position: band.columns.length };
        band.columns.push(placed);
        columns.set(placed.node.id, placed);
      }
    }
    bands.push(band);
  }
  for (const band of layout.bands) {
    if (band.axis === null) continue;
    const index = bands.length;
    const moved: Band = { ...band, index, columns: [] };
    for (const column of band.columns) {
      const placed: Column = { ...column, bandIndex: index };
      moved.columns.push(placed);
      columns.set(placed.node.id, placed);
    }
    bands.push(moved);
  }
  return { bands, columns, users: layout.users };
}
