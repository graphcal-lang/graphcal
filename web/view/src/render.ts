import type { Band, Column, Layout } from "./layout.ts";
import type { NodeKind, ReduceOp } from "./model.ts";
import { displayOf, valueAt } from "./values.ts";

const MARK: Record<NodeKind, string> = {
  param: "◇",
  const: "○",
  node: "●",
  assert: "✓",
  external: "⤴",
};

const REDUCE_SYMBOL: Record<ReduceOp, string> = {
  sum: "Σ",
  maximum: "max",
  minimum: "min",
  count: "count",
  rss: "rss",
  mean: "mean",
};

export const RULER_WIDTH = 136;

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function shapeText(column: Column): string {
  const { axes, reduce } = column.node;
  const shape = axes.length === 0 ? "[] scalar" : `[${axes.join(", ")}]`;
  if (reduce) return `${shape} ← ${REDUCE_SYMBOL[reduce.op]} ${reduce.axis}`;
  return shape;
}

function dimensionClass(dimension: string | null): string {
  switch (dimension) {
    case "Dimensionless":
    case "Int":
    case "Bool":
      return `dim-${dimension.toLowerCase()}`;
    case null:
      return "";
    default:
      return "dim-quantity";
  }
}

function renderHead(column: Column): HTMLElement {
  const { node } = column;
  const head = el("div", `col-head kind-${node.kind}`);
  head.dataset.node = node.id;
  const title = el("div", "col-title");
  title.append(el("span", "mark", MARK[node.kind]), el("span", "name", node.name));
  const tag = node.kind === "external" ? "import" : node.pub ? "pub" : node.kind;
  title.append(el("span", "tag", tag));
  head.append(title);
  if (node.kind === "external") {
    const origin = node.module.split(".").at(-1) ?? node.module;
    head.append(el("div", "col-origin", `← ${origin}.gcl`));
  } else {
    head.append(el("div", "col-shape", shapeText(column)));
  }
  const type =
    node.dimension === null ? "" : node.unit ? `${node.dimension} · ${node.unit}` : node.dimension;
  head.append(el("div", `col-type ${dimensionClass(node.dimension)}`, type));
  return head;
}

function cellText(column: Column, labels: string[]): string {
  const value = valueAt(column.node.value, labels);
  if (value && value.kind === "quantity") return value.magnitude;
  return displayOf(value);
}

/** A column of an axis band, or a labelled box in the no-axis band. */
function renderTable(column: Column, band: Band): HTMLElement {
  const root = el("div", `col role-${column.role} table`);
  root.dataset.node = column.node.id;
  root.append(renderHead(column));
  const inBand = band.axis !== null;
  const hasSub = inBand ? band.columns.some((c) => c.subs.length > 0) : column.subs.length > 0;
  if (hasSub) {
    const sub = el("div", "col-sub");
    sub.style.gridTemplateColumns = `repeat(${Math.max(1, column.subs.length)}, minmax(0, 1fr))`;
    for (const label of column.subs) sub.append(el("span", "sub-label", label));
    root.append(sub);
  }
  const body = el("div", "col-body");
  const subCount = Math.max(1, column.subs.length);
  body.style.gridTemplateColumns = `${inBand ? "" : "auto "}repeat(${subCount}, minmax(0, 1fr))`;
  for (const row of column.rows) {
    if (!inBand) body.append(el("span", "row-key", row));
    const keys = column.subs.length > 0 ? column.subs : [null];
    for (const sub of keys) {
      const cell = el("div", "cell", cellText(column, [row, ...(sub === null ? [] : [sub])]));
      cell.dataset.node = column.node.id;
      cell.dataset.row = row;
      if (sub !== null) cell.dataset.sub = sub;
      body.append(cell);
    }
  }
  root.append(body);
  return root;
}

/** A scalar box: one value, unit set apart. */
function renderBox(column: Column): HTMLElement {
  const root = el("div", `col role-${column.role} box`);
  root.dataset.node = column.node.id;
  root.append(renderHead(column));
  const value = column.node.value;
  const body = el("div", "box-value");
  body.dataset.node = column.node.id;
  if (value && value.kind === "quantity" && value.unit) {
    body.append(el("span", "magnitude", value.magnitude), el("span", "unit", value.unit));
  } else if (value && value.kind === "bool" && column.node.kind === "assert") {
    body.append(el("span", value.value ? "pass" : "fail", value.display.toLowerCase()));
  } else {
    body.append(el("span", "magnitude", displayOf(value)));
  }
  root.append(body);
  return root;
}

/** A folded column: a thin strip that keeps its wires and row positions. */
function renderFolded(column: Column, band: Band): HTMLElement {
  const root = el("div", `col role-${column.role} folded`);
  root.dataset.node = column.node.id;
  root.title = `${column.node.name} · click to expand`;
  const head = el("div", "col-head fold-head");
  head.dataset.node = column.node.id;
  head.append(el("span", "mark", MARK[column.node.kind]));
  root.append(head);
  const strip = el("div", "fold-strip");
  strip.append(el("span", "fold-name", column.node.name));
  const ticks = el("div", "fold-ticks");
  if (band.axis !== null) {
    if (band.columns.some((c) => c.subs.length > 0)) ticks.append(el("span", "fold-sub"));
    for (const row of column.rows) {
      const tick = el("span", "fold-tick");
      tick.dataset.node = column.node.id;
      tick.dataset.row = row;
      ticks.append(tick);
    }
  }
  strip.append(ticks);
  root.append(strip);
  return root;
}

export function renderColumn(column: Column, band: Band, folded: boolean): HTMLElement {
  if (folded) return renderFolded(column, band);
  if (column.rows.length === 0) return renderBox(column);
  return renderTable(column, band);
}

function renderRuler(band: Band): HTMLElement {
  const ruler = el("div", "ruler");
  ruler.style.width = `${RULER_WIDTH}px`;
  if (band.axis === null) {
    if (band.index === 0) ruler.append(el("div", "ruler-none", "no axis  [ ]"));
    return ruler;
  }
  const box = el("div", "ruler-box");
  box.append(
    el("div", "ruler-axis", band.axis),
    el("div", "ruler-note", `index · ${band.labels.length} labels`),
    el("div", "ruler-unrolled", "↕ unrolled as rows"),
  );
  ruler.append(box);
  if (band.columns.some((c) => c.subs.length > 0)) ruler.append(el("div", "ruler-sub"));
  for (const label of band.labels) {
    const row = el("div", "row-label", label);
    row.dataset.row = label;
    ruler.append(row);
  }
  return ruler;
}

export interface RenderOptions {
  folded: ReadonlySet<string>;
}

function renderBand(band: Band, options: RenderOptions): HTMLElement {
  const section = el("section", `band ${band.axis === null ? "band-scalar" : "band-axis"}`);
  section.dataset.band = String(band.index);
  section.append(renderRuler(band));
  const zones = el("div", "zones");
  const parts: [string, Column[]][] = [
    ["const", band.columns.filter((c) => c.role === "const")],
    ["param", band.columns.filter((c) => c.role === "param")],
    ["node", band.columns.filter((c) => c.role === "node")],
    ["output", band.columns.filter((c) => c.role === "output")],
  ];
  for (const [role, columns] of parts) {
    const zone = el("div", `zone zone-${role}`);
    zone.dataset.role = role;
    for (const column of columns) {
      zone.append(renderColumn(column, band, options.folded.has(column.node.id)));
    }
    zones.append(zone);
  }
  section.append(zones);
  return section;
}

function renderZoneHead(): HTMLElement {
  const head = el("div", "zone-head");
  head.append(el("div", "ruler-spacer"));
  const zones = el("div", "zones");
  for (const [role, label] of [
    ["const", "const"],
    ["param", "param · inputs"],
    ["node", "node · derived"],
    ["output", "outputs"],
  ] as const) {
    const zone = el("div", `zone zone-${role}`);
    zone.dataset.role = role;
    const text = el("span", `zone-label label-${role}`, label);
    if (role === "output") text.title = "nodes nothing in this file reads, pub nodes, and asserts";
    zone.append(text);
    zones.append(zone);
  }
  head.append(zones);
  return head;
}

function renderLegend(): HTMLElement {
  const legend = el("div", "legend");
  for (const [cls, text] of [
    ["swatch-idle", "idle"],
    ["swatch-direct", "direct"],
    ["swatch-transitive", "transitive"],
  ] as const) {
    legend.append(el("span", `swatch ${cls}`), el("span", "legend-text", text));
  }
  legend.append(
    el("span", "legend-text", "◇ param  ○ const  ● node  ✓ assert  ⤴ import   [axis] = shape"),
  );
  return legend;
}

export function renderView(container: HTMLElement, layout: Layout, options: RenderOptions): void {
  container.replaceChildren();
  container.append(renderZoneHead());
  layout.bands.forEach((band, i) => {
    const channel = el("div", "channel");
    channel.dataset.channel = String(i);
    container.append(channel, renderBand(band, options));
  });
  const last = el("div", "channel");
  last.dataset.channel = String(layout.bands.length);
  container.append(last, renderLegend());
  alignZones(container);
}

/** Give each role zone the same width in every band so roles line up. */
function alignZones(container: HTMLElement): void {
  for (const role of ["const", "param", "node", "output"]) {
    const zones = [...container.querySelectorAll<HTMLElement>(`.zone[data-role="${role}"]`)];
    for (const zone of zones) zone.style.minWidth = "";
    const width = Math.max(...zones.map((z) => z.getBoundingClientRect().width));
    for (const zone of zones) zone.style.minWidth = `${Math.ceil(width)}px`;
  }
}
