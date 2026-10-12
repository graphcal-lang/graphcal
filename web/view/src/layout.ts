import type { ViewModel, ViewNode } from "./model.ts";

// Layout grammar: x = role (const → param → node, with outputs as the right
// sub-zone of node), y = shape. An axis gets a band of its own only when two or
// more nodes share it as their first axis; otherwise the node stays in the
// no-axis band as a labelled box.

export type Role = "const" | "param" | "node" | "output";
export const ROLES: readonly Role[] = ["const", "param", "node", "output"];

/** Outputs are the right end of the node zone, so zones are three. */
export type Zone = "const" | "param" | "node";
export const ZONES: readonly Zone[] = ["const", "param", "node"];
export function zoneOf(role: Role): Zone {
  return role === "output" ? "node" : role;
}

export interface Column {
  node: ViewNode;
  role: Role;
  depth: number;
  bandIndex: number;
  /** Index among the band's columns in render order (for no-axis boxes: the stack). */
  position: number;
  /** Row within the stack; 0 for the top box and for every axis-band column. */
  tierRow: number;
  /** Row labels this column renders (band labels, or its own first axis). */
  rows: string[];
  /** Sub-column labels (second axis), if any. */
  subs: string[];
  /** May be folded by fit mode: an intermediate node (never an output). */
  foldable: boolean;
  /** How much the node matters to the rest of the file; low activity folds first. */
  activity: number;
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
        tierRow: 0,
        rows: axis !== null ? band.labels : ownAxis ? (labelsOf.get(ownAxis) ?? []) : [],
        subs: subAxis ? (labelsOf.get(subAxis) ?? []) : [],
        foldable: node.kind === "node" && roleOfNode(node) === "node",
        // Consumers first, then how far the value travels (depth from params).
        activity: consumers.length * 10 + (depths.get(node.id) ?? 0),
      };
      band.columns.push(column);
      columns.set(node.id, column);
    });
    bands.push(band);
  });
  return { bands, columns, users };
}

export const BOX_GAP = 10;
export const STACK_GAP = 8;

export interface BoxSize {
  width: number;
  height: number;
}

export interface PackedZone {
  stacks: Column[][];
  width: number;
  height: number;
}

/**
 * Shelf packing of one zone's boxes: keep their order, fill stacks top to
 * bottom up to a height cap, and raise the cap until the stacks fit the zone
 * width. A tall box ends up beside a stack of short ones. Output boxes never
 * share a stack with other node boxes, so the outputs halo stays honest.
 */
export function packZone(
  boxes: Column[],
  size: (id: string) => BoxSize,
  width: number,
  folded: ReadonlySet<string>,
): PackedZone {
  if (boxes.length === 0) return { stacks: [], width: 0, height: 0 };
  const tallest = Math.max(...boxes.map((b) => size(b.node.id).height));
  let cap = tallest;
  for (;;) {
    const stacks: Column[][] = [];
    const heights: number[] = [];
    const widths: number[] = [];
    const groups: boolean[] = [];
    for (const box of boxes) {
      const { width: w, height: h } = size(box.node.id);
      const isOutput = box.role === "output";
      const chip = folded.has(box.node.id);
      let i = heights.findIndex(
        (used, k) => (chip || groups[k] === isOutput) && used + STACK_GAP + h <= cap,
      );
      if (i === -1) {
        i = stacks.length;
        stacks.push([]);
        heights.push(-STACK_GAP);
        widths.push(0);
        groups.push(isOutput);
      }
      stacks[i].push(box);
      heights[i] += STACK_GAP + h;
      widths[i] = Math.max(widths[i], w);
    }
    const total = widths.reduce((sum, w) => sum + w, 0) + (stacks.length - 1) * BOX_GAP;
    if (total <= width || cap > tallest * 4) {
      return { stacks, width: total, height: Math.max(...heights) };
    }
    cap += 8;
  }
}

/** Re-create the no-axis band with its boxes packed into stacks per zone. */
export function packScalarBand(
  layout: Layout,
  size: (id: string) => BoxSize,
  zoneWidth: Record<Zone, number>,
  folded: ReadonlySet<string>,
): { layout: Layout; used: Record<Zone, number>; stacked: Record<Zone, boolean> } {
  const used = { const: 0, param: 0, node: 0 };
  const stacked = { const: false, param: false, node: false };
  const scalar = layout.bands.find((b) => b.axis === null);
  if (!scalar) return { layout, used, stacked };
  const columns = new Map(layout.columns);
  const packedColumns: Column[] = [];
  let position = 0;
  for (const zone of ZONES) {
    // Node zone order: full node boxes, then outputs, then folded chips, which
    // slip into whatever stack has room (typically under the outputs).
    const rank = (c: Column): number => (c.role === "output" ? 1 : folded.has(c.node.id) ? 2 : 0);
    const boxes = scalar.columns
      .filter((c) => zoneOf(c.role) === zone)
      .map((c, i) => ({ c, i }))
      .sort((a, b) => rank(a.c) - rank(b.c) || a.i - b.i)
      .map(({ c }) => c);
    const packed = packZone(boxes, size, zoneWidth[zone], folded);
    used[zone] = packed.width;
    stacked[zone] = packed.stacks.some((s) => s.length > 1);
    for (const stack of packed.stacks) {
      stack.forEach((column, tierRow) => {
        const placed: Column = { ...column, position, tierRow };
        packedColumns.push(placed);
        columns.set(placed.node.id, placed);
      });
      position++;
    }
  }
  const bands = layout.bands.map((b) => (b === scalar ? { ...b, columns: packedColumns } : b));
  return { layout: { bands, columns, users: layout.users }, used, stacked };
}

/**
 * Barycenter ordering: within each zone, sort columns by the mean x of the
 * columns they are wired to, so wires run short and cross less. Roles stay in
 * their zones; ties keep the depth / declaration order.
 */
export function orderByBarycenter(layout: Layout, model: ViewModel): Layout {
  const roleRank = new Map(ROLES.map((r, i) => [r, i]));
  const neighbours = new Map<string, string[]>();
  for (const node of model.nodes) {
    const list = [...node.deps, ...(layout.users.get(node.id) ?? [])].filter((id) =>
      layout.columns.has(id),
    );
    neighbours.set(node.id, list);
  }
  const x = new Map<string, number>();
  const assign = (bands: Band[]): void => {
    for (const band of bands) {
      for (const role of ROLES) {
        const zone = band.columns.filter((c) => c.role === role);
        zone.forEach((c, i) =>
          x.set(c.node.id, (roleRank.get(role) ?? 0) * 1000 + ((i + 0.5) / zone.length) * 1000),
        );
      }
    }
  };
  let bands = layout.bands;
  assign(bands);
  for (let round = 0; round < 3; round++) {
    bands = bands.map((band) => {
      const ordered = [...band.columns].sort((a, b) => {
        const byRole = (roleRank.get(a.role) ?? 0) - (roleRank.get(b.role) ?? 0);
        if (byRole !== 0) return byRole;
        const key = (c: Column): number => {
          const others = (neighbours.get(c.node.id) ?? []).map((id) => x.get(id) ?? 0);
          return others.length === 0
            ? (x.get(c.node.id) ?? 0)
            : others.reduce((s, v) => s + v, 0) / others.length;
        };
        const byKey = key(a) - key(b);
        if (byKey !== 0) return byKey;
        return a.position - b.position;
      });
      return { ...band, columns: ordered.map((c, position) => ({ ...c, position })) };
    });
    assign(bands);
  }
  const columns = new Map<string, Column>();
  for (const band of bands) for (const c of band.columns) columns.set(c.node.id, c);
  return { bands, columns, users: layout.users };
}
