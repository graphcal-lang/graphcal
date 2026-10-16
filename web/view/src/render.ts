import type { Axis, Band, Column, Layout, Role, Zone } from "./layout.ts";
import { axisName, entryName, ZONES, zoneOf } from "./layout.ts";
import { isExternal, type NodeKind, type ViewNode } from "./model.ts";
import { rowRuns, type Hidden } from "./rows.ts";
import { displayOf, shortMagnitude, valueAt } from "./values.ts";

const MARK: Record<NodeKind, string> = {
  param: "◇",
  const: "○",
  node: "●",
  assert: "✓",
};

/** A value taken from another file: a diamond with a dot inside. */
export const EXTERNAL_MARK = "⟐";

function markOf(node: ViewNode): string {
  return isExternal(node) ? EXTERNAL_MARK : MARK[node.kind];
}

/** The head class: `kind-external` for a value read from another file, else the declaration kind. */
function kindClass(node: ViewNode): string {
  return `kind-${isExternal(node) ? "external" : node.kind}`;
}

function fileOf(module: string): string {
  return `${module.split(".").at(-1) ?? module}.gcl`;
}

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

/**
 * The two-line head: mark, name and (for an external) its origin on the first
 * line; the type on the second. Folded, the value joins the second line, so the
 * values of a stack of folded boxes line up at their right edge.
 */
function renderHead(column: Column, folded = false): HTMLElement {
  const { node } = column;
  const head = el("div", `col-head ${kindClass(node)}`);
  head.dataset.node = node.id;
  // An external names the file it is taken from in small type above its name.
  if (isExternal(node)) head.append(el("div", "col-origin", fileOf(node.module)));
  const title = el("div", "col-title");
  title.append(el("span", "mark", markOf(node)), el("span", "name", node.name));
  // The first line of the `///` doc sits after the name, cut short; folded, there is no room.
  const doc = node.doc?.split("\n")[0]?.trim() ?? "";
  if (doc !== "" && !folded) {
    const note = el("span", "col-doc", doc);
    note.title = node.doc ?? "";
    title.append(note);
  }
  // A display unit that could not be applied: the value is shown in SI.
  if (node.presentation.length > 0) {
    const warn = el("span", "col-presentation", "⚠");
    warn.title = node.presentation.join("\n");
    title.append(warn);
  }
  head.append(title);
  const meta = el("div", "col-meta");
  if (node.dimension !== null) {
    // Folded, the unit travels with the value instead.
    const type = node.unit && !folded ? `${node.dimension} · ${node.unit}` : node.dimension;
    meta.append(el("span", "col-type", type));
  }
  if (folded) {
    const slot = el("span", "head-value");
    slot.append(...valueSpans(column));
    meta.append(slot);
  }
  head.append(meta);
  return head;
}

/** A value as spans: magnitude with its unit set apart, pass / fail for an assert. */
function valueSpans(column: Column): HTMLElement[] {
  const value = column.node.value;
  if (value && value.kind === "quantity" && value.unit) {
    return [
      el("span", "magnitude", shortMagnitude(value.magnitude)),
      el("span", "unit", value.unit),
    ];
  }
  if (value && value.kind === "bool" && column.node.kind === "assert") {
    return [el("span", value.value ? "pass" : "fail", value.display.toLowerCase())];
  }
  return [el("span", "magnitude", displayOf(value))];
}

function cellText(column: Column, labels: string[]): string {
  const value = valueAt(column.node.value, labels);
  if (value && value.kind === "quantity") return shortMagnitude(value.magnitude);
  return displayOf(value);
}

/** The entry keys a gap element hides, as `renderGap` recorded them. */
export function gapKeys(gap: HTMLElement): string[] {
  const raw: unknown = JSON.parse(gap.dataset.keys ?? "[]");
  return Array.isArray(raw) ? raw.filter((key): key is string => typeof key === "string") : [];
}

/** A collapsed run of hidden rows: a thin dotted gap that keeps the rows aligned. */
function renderGap(keys: string[], axis: string, axes: ReadonlyMap<string, Axis>): HTMLElement {
  const gap = el("div", "row-gap");
  gap.dataset.axis = axis;
  // The keys are opaque, so they travel as a JSON array rather than joined.
  gap.dataset.keys = JSON.stringify(keys);
  const names = keys.map((key) => entryName(axes, axis, key));
  gap.title = `${keys.length} hidden: ${names.join(", ")} · click to show`;
  return gap;
}

/** A column of an axis band, or a labelled box in the no-axis band. */
function renderTable(
  column: Column,
  band: Band,
  hidden: Hidden,
  axes: ReadonlyMap<string, Axis>,
): HTMLElement {
  const root = el("div", `col role-${column.role} table`);
  root.dataset.node = column.node.id;
  root.append(renderHead(column));
  const inBand = band.axis !== null;
  const hasSub = inBand ? band.columns.some((c) => c.subs.length > 0) : column.subs.length > 0;
  if (hasSub) {
    const sub = el("div", "col-sub");
    sub.style.gridTemplateColumns = `repeat(${Math.max(1, column.subs.length)}, minmax(0, 1fr))`;
    const subAxis = column.node.axes[1] ?? "";
    for (const key of column.subs) {
      sub.append(el("span", "sub-label", entryName(axes, subAxis, key)));
    }
    root.append(sub);
  }
  const body = el("div", "col-body");
  const subCount = Math.max(1, column.subs.length);
  body.style.gridTemplateColumns = `${inBand ? "" : "auto "}repeat(${subCount}, minmax(0, 1fr))`;
  const rowAxis = column.node.axes[0] ?? "";
  for (const run of rowRuns(column.rows, hidden.get(rowAxis))) {
    if (run.kind === "gap") {
      const gap = renderGap(run.labels, rowAxis, axes);
      gap.style.gridColumn = "1 / -1";
      body.append(gap);
      continue;
    }
    const row = run.label;
    const stripe = run.visible % 2 === 1 ? " stripe" : "";
    if (!inBand) body.append(el("span", `row-key${stripe}`, entryName(axes, rowAxis, row)));
    const keys = column.subs.length > 0 ? column.subs : [null];
    for (const sub of keys) {
      const cell = el(
        "div",
        `cell${stripe}`,
        cellText(column, [row, ...(sub === null ? [] : [sub])]),
      );
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
  const body = el("div", "box-value");
  body.dataset.node = column.node.id;
  body.append(...valueSpans(column));
  root.append(body);
  return root;
}

/**
 * A folded column. A scalar box keeps its two-line head with the value on
 * the second line; a table becomes a narrow vertical strip (mark on top,
 * the name written downwards, one tick per visible row), so folding it
 * gives the width back while its rows stay aligned with the band's.
 */
function renderFolded(column: Column, band: Band, hidden: Hidden): HTMLElement {
  const root = el("div", `col role-${column.role} folded`);
  root.dataset.node = column.node.id;
  root.title = `${column.node.name} · click to expand`;
  if (column.rows.length === 0) {
    root.append(renderHead(column, true));
    return root;
  }
  root.classList.add("strip");
  const head = el("div", `col-head fold-head ${kindClass(column.node)}`);
  head.dataset.node = column.node.id;
  head.append(el("span", "mark", markOf(column.node)));
  root.append(head);
  const strip = el("div", "fold-strip");
  strip.append(el("span", "fold-name", column.node.name));
  const ticks = el("div", "fold-ticks");
  const inBand = band.axis !== null;
  const hasSub = inBand ? band.columns.some((c) => c.subs.length > 0) : column.subs.length > 0;
  if (hasSub) ticks.append(el("span", "fold-sub"));
  for (const run of rowRuns(column.rows, hidden.get(column.node.axes[0] ?? ""))) {
    if (run.kind === "gap") {
      ticks.append(el("span", "fold-gap"));
      continue;
    }
    const tick = el("span", "fold-tick");
    tick.dataset.node = column.node.id;
    tick.dataset.row = run.label;
    ticks.append(tick);
  }
  strip.append(ticks);
  root.append(strip);
  return root;
}

function renderColumn(
  column: Column,
  band: Band,
  options: RenderOptions,
  axes: ReadonlyMap<string, Axis>,
): HTMLElement {
  if (options.folded.has(column.node.id)) return renderFolded(column, band, options.hidden);
  if (column.rows.length === 0) return renderBox(column);
  return renderTable(column, band, options.hidden, axes);
}

/**
 * The ruler reserves its width at the left of the band; its labels are placed
 * right against the band's first table (see placeRulers), not at the far left.
 */
function renderRuler(band: Band, hidden: Hidden, axes: ReadonlyMap<string, Axis>): HTMLElement {
  const ruler = el("div", "ruler");
  ruler.style.width = `${RULER_WIDTH}px`;
  if (band.axis === null) return ruler;
  const inner = el("div", "ruler-inner");
  inner.style.width = `${RULER_WIDTH}px`;
  const hiddenCount = hidden.get(band.axis)?.size ?? 0;
  const box = el("div", "ruler-box");
  box.dataset.axis = band.axis;
  const note = el(
    "div",
    "ruler-note",
    hiddenCount > 0
      ? `${band.labels.length - hiddenCount} / ${band.labels.length}`
      : String(band.labels.length),
  );
  const origin = axes.get(band.axis)?.origin ?? null;
  if (origin !== null) {
    // Where the axis is declared, in small type above its name as on an external's head.
    box.classList.add("imported");
    box.append(el("div", "ruler-origin", fileOf(origin)));
  }
  box.append(el("div", "ruler-axis", axisName(axes, band.axis)), note);
  inner.append(box);
  if (band.columns.some((c) => c.subs.length > 0)) inner.append(el("div", "ruler-sub"));
  for (const run of rowRuns(band.labels, hidden.get(band.axis))) {
    if (run.kind === "gap") {
      const gap = renderGap(run.labels, band.axis, axes);
      gap.classList.add("ruler-gap");
      gap.textContent = `⋯ ${run.labels.length} hidden`;
      inner.append(gap);
      continue;
    }
    const row = el(
      "div",
      `row-label${run.visible % 2 === 1 ? " stripe" : ""}`,
      entryName(axes, band.axis, run.label),
    );
    row.dataset.row = run.label;
    row.dataset.axis = band.axis;
    inner.append(row);
  }
  ruler.append(inner);
  return ruler;
}

/** Move each axis band's labels to sit right against the band's first table. */
function placeRulers(container: HTMLElement): void {
  const origin = container.getBoundingClientRect();
  const k = container.offsetWidth > 0 ? origin.width / container.offsetWidth : 1;
  for (const band of container.querySelectorAll<HTMLElement>(".band-axis")) {
    const inner = band.querySelector<HTMLElement>(".ruler-inner");
    if (!inner) continue;
    const bandLeft = band.getBoundingClientRect().left;
    const lefts = [...band.querySelectorAll<HTMLElement>(".col")].map(
      (c) => (c.getBoundingClientRect().left - bandLeft) / k,
    );
    const first = lefts.length > 0 ? Math.min(...lefts) : RULER_WIDTH;
    inner.style.left = `${Math.max(0, first - RULER_WIDTH)}px`;
  }
}

export interface RenderOptions {
  folded: ReadonlySet<string>;
  hidden: Hidden;
}

function renderBand(
  band: Band,
  options: RenderOptions,
  axes: ReadonlyMap<string, Axis>,
): HTMLElement {
  const section = el("section", `band ${band.axis === null ? "band-scalar" : "band-axis"}`);
  section.dataset.band = String(band.index);
  section.append(renderRuler(band, options.hidden, axes));
  const zones = el("div", "zones");
  for (const zone of ZONES) {
    const columns = band.columns.filter((c) => zoneOf(c.role) === zone);
    const zoneEl = el("div", `zone zone-${zone}`);
    zoneEl.dataset.zone = zone;
    // Each element is a column or, in the no-axis band, a stack of columns;
    // its role is that of its top box.
    const items: { role: Role; element: HTMLElement }[] = [];
    if (band.axis === null) {
      // Boxes are packed into stacks; a stack is one position in the band.
      const stacks = new Map<number, Column[]>();
      for (const column of columns) {
        stacks.set(column.position, [...(stacks.get(column.position) ?? []), column]);
      }
      for (const [, stack] of [...stacks].sort(([a], [b]) => a - b)) {
        const stackEl = el("div", "stack");
        const ordered = stack.sort((a, b) => a.tierRow - b.tierRow);
        for (const column of ordered) {
          stackEl.append(renderColumn(column, band, options, axes));
        }
        items.push({ role: ordered[0].role, element: stackEl });
      }
    } else {
      for (const column of columns) {
        items.push({
          role: column.role,
          element: renderColumn(column, band, options, axes),
        });
      }
    }
    if (zone === "node") {
      // Outputs are the aligned right end of the node zone, so one halo can
      // enclose them across every band without covering other node columns.
      for (const group of ["node", "output"] as const) {
        const groupEl = el("div", `group group-${group}`);
        groupEl.dataset.group = group;
        for (const item of items) {
          if ((item.role === "output") === (group === "output")) groupEl.append(item.element);
        }
        zoneEl.append(groupEl);
      }
    } else {
      for (const item of items) zoneEl.append(item.element);
    }
    zones.append(zoneEl);
  }
  section.append(zones);
  return section;
}

function renderZoneHead(): HTMLElement {
  const head = el("div", "zone-head");
  head.append(el("div", "ruler-spacer"));
  const zones = el("div", "zones");
  const labels: Record<Zone, string> = {
    const: "const",
    param: "param · inputs",
    node: "node · derived",
  };
  for (const zone of ZONES) {
    const zoneEl = el("div", `zone zone-${zone}`);
    zoneEl.dataset.zone = zone;
    zoneEl.append(el("span", `zone-label label-${zone}`, labels[zone]));
    zones.append(zoneEl);
  }
  head.append(zones);
  return head;
}

/**
 * One faint halo behind all output boxes. The output groups are aligned across
 * bands, so the halo spans bands without covering other node columns. Call it
 * after the wires are drawn: channel heights move the lower bands.
 */
export function placeHalo(container: HTMLElement): void {
  for (const halo of container.querySelectorAll(".outputs-halo")) halo.remove();
  const outputs = [...container.querySelectorAll<HTMLElement>(".group-output .col")];
  if (outputs.length === 0) return;
  const origin = container.getBoundingClientRect();
  const k = container.offsetWidth > 0 ? origin.width / container.offsetWidth : 1;
  const rects = outputs.map((o) => o.getBoundingClientRect());
  const pad = 6;
  const left = (Math.min(...rects.map((r) => r.left)) - origin.left) / k - pad;
  const top = (Math.min(...rects.map((r) => r.top)) - origin.top) / k - pad;
  const right = (Math.max(...rects.map((r) => r.right)) - origin.left) / k + pad;
  const bottom = (Math.max(...rects.map((r) => r.bottom)) - origin.top) / k + pad;
  const halo = el("div", "outputs-halo");
  halo.style.left = `${left}px`;
  halo.style.top = `${top}px`;
  halo.style.width = `${right - left}px`;
  halo.style.height = `${bottom - top}px`;
  halo.title = "outputs: nodes nothing in this file reads, pub nodes, and asserts";
  halo.append(el("span", "halo-label", "outputs"));
  container.prepend(halo);
}

export function renderView(container: HTMLElement, layout: Layout, options: RenderOptions): void {
  container.replaceChildren();
  container.append(renderZoneHead());
  layout.bands.forEach((band, i) => {
    const channel = el("div", "channel");
    channel.dataset.channel = String(i);
    container.append(channel, renderBand(band, options, layout.axes));
  });
  const last = el("div", "channel");
  last.dataset.channel = String(layout.bands.length);
  container.append(last);
  alignZones(container);
  placeRulers(container);
}

/** Give each zone (and the node / output groups) the same width in every band. */
function alignZones(container: HTMLElement): void {
  const align = (selector: string): void => {
    const parts = [...container.querySelectorAll<HTMLElement>(selector)];
    for (const part of parts) part.style.minWidth = "";
    const width = Math.max(...parts.map((p) => p.getBoundingClientRect().width));
    for (const part of parts) part.style.minWidth = `${Math.ceil(width)}px`;
  };
  for (const group of ["node", "output"]) align(`.group[data-group="${group}"]`);
  for (const zone of ZONES) align(`.zone[data-zone="${zone}"]`);
}
