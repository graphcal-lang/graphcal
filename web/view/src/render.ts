import type { Band, Column, Layout, Role } from "./layout.ts";
import { ROLES } from "./layout.ts";
import type { NodeKind, ViewModel } from "./model.ts";
import { displayOf, valueAt } from "./values.ts";

const MARK: Record<NodeKind, string> = {
  param: "◇",
  const: "○",
  node: "●",
  assert: "✓",
  external: "⤴",
};

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
  return `[${column.node.axes.join(", ")}]`;
}

function typeText(column: Column): string {
  const { dimension, unit } = column.node;
  if (dimension === null) return "";
  return unit ? `${dimension} · ${unit}` : dimension;
}

function renderHead(column: Column, model: ViewModel): HTMLElement {
  const head = el("div", `col-head kind-${column.node.kind}`);
  head.dataset.node = column.node.id;
  const title = el("div", "col-title");
  title.append(el("span", "mark", MARK[column.node.kind]), el("span", "name", column.node.name));
  if (column.node.pub && column.node.kind !== "external") title.append(el("span", "tag", "pub"));
  head.append(title);
  const meta = el("div", "col-meta");
  meta.append(el("span", "shape", shapeText(column)), el("span", "type", typeText(column)));
  head.append(meta);
  if (column.node.kind === "external") {
    const origin = column.node.module.split(".").at(-1) ?? column.node.module;
    void model;
    head.append(el("div", "col-origin", `← ${origin}.gcl`));
  }
  return head;
}

function subLabels(column: Column, model: ViewModel): string[] {
  const axis = column.subAxes[0];
  if (axis === undefined) return [];
  return model.indexes.find((i) => i.name === axis)?.labels ?? [];
}

function renderColumn(column: Column, band: Band, model: ViewModel, hasSub: boolean): HTMLElement {
  const root = el("div", `col role-${column.role}`);
  root.dataset.node = column.node.id;
  root.append(renderHead(column, model));

  const subs = subLabels(column, model);
  if (hasSub) {
    const sub = el("div", "col-sub");
    if (subs.length > 0) {
      sub.style.gridTemplateColumns = `repeat(${subs.length}, minmax(0, 1fr))`;
      for (const label of subs) sub.append(el("span", "sub-label", label));
    }
    root.append(sub);
  }

  const body = el("div", "col-body");
  const width = Math.max(1, subs.length);
  body.style.gridTemplateColumns = `repeat(${width}, minmax(0, 1fr))`;
  const rows = band.axis === null ? [null] : band.labels;
  for (const row of rows) {
    const keys = subs.length > 0 ? subs : [null];
    for (const sub of keys) {
      const labels = [row, sub].filter((x): x is string => x !== null);
      const cell = el("div", "cell", displayOf(valueAt(column.node.value, labels)));
      cell.dataset.node = column.node.id;
      if (row !== null) cell.dataset.row = row;
      if (sub !== null) cell.dataset.sub = sub;
      body.append(cell);
    }
  }
  root.append(body);
  return root;
}

function renderZone(
  role: Role,
  columns: Column[],
  band: Band,
  model: ViewModel,
  hasSub: boolean,
): HTMLElement {
  const zone = el("div", `zone zone-${role}${columns.length === 0 ? " empty" : ""}`);
  zone.dataset.role = role;
  zone.append(el("div", "zone-label", role === "output" ? "outputs" : role));
  const cols = el("div", "zone-cols");
  for (const column of columns) cols.append(renderColumn(column, band, model, hasSub));
  zone.append(cols);
  return zone;
}

function renderBand(band: Band, model: ViewModel): HTMLElement {
  const section = el("section", `band ${band.axis === null ? "band-scalar" : "band-axis"}`);
  section.dataset.axis = band.axis ?? "";
  const head = el("div", "band-head");
  head.append(
    el("span", "band-shape", band.axis === null ? "[ ]" : `[${band.axis}]`),
    el(
      "span",
      "band-note",
      band.axis === null ? "no axis" : `unroll ${band.axis} · ${band.labels.length} rows`,
    ),
  );
  section.append(head);

  const hasSub = band.columns.some((c) => c.subAxes.length > 0);
  const body = el("div", "band-body");
  if (band.axis !== null) {
    const ruler = el("div", "ruler");
    ruler.append(el("div", "ruler-head", band.axis));
    if (hasSub) ruler.append(el("div", "ruler-sub"));
    for (const label of band.labels) ruler.append(el("div", "row-label", label));
    body.append(ruler);
  }
  for (const role of ROLES) {
    const columns = band.columns.filter((c) => c.role === role);
    body.append(renderZone(role, columns, band, model, hasSub));
  }
  section.append(body);
  return section;
}

export function renderView(container: HTMLElement, model: ViewModel, layout: Layout): void {
  container.replaceChildren();
  for (const band of layout.bands) container.append(renderBand(band, model));
}
