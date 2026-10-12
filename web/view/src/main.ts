import {
  BOX_GAP,
  computeLayout,
  ROLES,
  splitScalarBand,
  type Layout,
  type Role,
} from "./layout.ts";
import { parseViewModel, type ViewModel } from "./model.ts";
import { renderView, RULER_WIDTH } from "./render.ts";
import { trace, type Selection } from "./selection.ts";
import { displayOf, valueAt } from "./values.ts";
import { channelHeight, drawWires, planRoutes, type RoutePlan } from "./wires.ts";

const fixtures = import.meta.glob<unknown>("../fixtures/*.view.json", {
  eager: true,
  import: "default",
});

function need<T extends Element>(selector: string): T {
  const found = document.querySelector<T>(selector);
  if (!found) throw new Error(`view shell is missing ${selector}`);
  return found;
}

const viewEl = need<HTMLElement>("#view");
const wiresEl = need<SVGSVGElement>("#wires");
const fileEl = need<HTMLElement>("#file");
const unrollEl = need<HTMLElement>("#unroll");
const formulaEl = need<HTMLElement>("#formula");
const fixtureEl = need<HTMLSelectElement>("#fixture");

interface State {
  model: ViewModel;
  /** Layout before the no-axis band is split into tiers. */
  base: Layout;
  layout: Layout;
  plan: RoutePlan;
  folded: Set<string>;
  selection: Selection | null;
}
let state: State | null = null;

function describeSelection(s: State): string {
  const { selection, model, layout } = s;
  if (!selection) {
    const unrolled = layout.bands.filter((b) => b.axis !== null).length;
    return `${model.nodes.length} nodes · ${unrolled} unrolled ${unrolled === 1 ? "axis" : "axes"} · click a cell to trace`;
  }
  const node = model.nodes.find((n) => n.id === selection.id);
  const labels = [selection.row, selection.sub].filter((x): x is string => x !== null);
  const subject = labels.length > 0 ? `${selection.id}[${labels.join(", ")}]` : selection.id;
  const value = displayOf(valueAt(node?.value ?? null, labels));
  const expr = node?.expr ? ` = ${node.expr}` : "";
  const mode =
    selection.mode === "deps"
      ? "showing ← depends on (click again: → used by)"
      : "showing → used by (click again: ← depends on)";
  return `${subject}${expr} = ${value}  ·  ${mode}`;
}

function clearHighlights(): void {
  for (const n of viewEl.querySelectorAll<HTMLElement>(".sel, .rel-direct, .rel-all, .row-sel")) {
    n.classList.remove("sel", "rel-direct", "rel-all", "row-sel");
  }
  for (const w of wiresEl.querySelectorAll<SVGElement>(".wire, .wire-dot")) {
    w.classList.remove("lit", "direct");
    if (w.classList.contains("wire")) w.setAttribute("marker-end", "url(#arrow)");
  }
}

function applySelection(): void {
  if (!state) return;
  clearHighlights();
  formulaEl.textContent = describeSelection(state);
  formulaEl.classList.toggle("active", state.selection !== null);
  const { selection, model, layout } = state;
  if (!selection) return;

  const result = trace(selection, model, layout);
  const mark = (id: string, cls: string): void => {
    const column = layout.columns.get(id);
    const col = viewEl.querySelector<HTMLElement>(`.col[data-node="${id}"]`);
    if (!col || !column) return;
    col.classList.add(cls);
    // Row-level: light only the selected row in columns that share the band.
    const sameBand = column.bandIndex === layout.columns.get(selection.id)?.bandIndex;
    if (selection.row !== null && sameBand && column.rows.length > 0) {
      for (const cell of col.querySelectorAll<HTMLElement>(`[data-row="${selection.row}"]`)) {
        cell.classList.add("row-sel");
      }
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
    const direct = result.edges.get(`${w.dataset.from}->${w.dataset.to}`);
    if (direct === undefined) continue;
    w.classList.add("lit");
    if (direct) w.classList.add("direct");
    if (w.classList.contains("wire")) w.setAttribute("marker-end", "url(#arrow-lit)");
  }
}

function redraw(): void {
  if (!state) return;
  drawWires(wiresEl, viewEl, state.plan);
  applySelection();
}

/** Horizontal room for zones: the window minus ruler, paddings, and the gutter. */
function availableWidth(): number {
  const zonePadding = 4 * 24;
  return document.documentElement.clientWidth - 16 - 40 - RULER_WIDTH - zonePadding - 56;
}

/**
 * Render once to measure boxes and axis-band zones, then split the no-axis
 * band into tiers so everything fits the width.
 */
function fit(base: Layout, folded: ReadonlySet<string>): Layout {
  renderView(viewEl, base, { folded });
  const boxWidths = new Map<string, number>();
  for (const col of viewEl.querySelectorAll<HTMLElement>(".band-scalar .col")) {
    if (col.dataset.node) boxWidths.set(col.dataset.node, col.getBoundingClientRect().width);
  }
  const target = {} as Record<Role, number>;
  const need = {} as Record<Role, number>;
  for (const role of ROLES) {
    let axisWidth = 0;
    for (const zone of viewEl.querySelectorAll<HTMLElement>(
      `.band-axis .zone[data-role="${role}"]`,
    )) {
      const cols = [...zone.querySelectorAll<HTMLElement>(":scope > .col")];
      const inner =
        cols.reduce((sum, col) => sum + col.getBoundingClientRect().width, 0) +
        Math.max(0, cols.length - 1) * BOX_GAP;
      axisWidth = Math.max(axisWidth, inner);
    }
    const boxes = base.bands
      .filter((b) => b.axis === null)
      .flatMap((b) => b.columns)
      .filter((c) => c.role === role)
      .map((c) => boxWidths.get(c.node.id) ?? 0);
    const widest = Math.max(0, ...boxes);
    const natural = boxes.reduce((sum, w) => sum + w, 0) + Math.max(0, boxes.length - 1) * BOX_GAP;
    target[role] = Math.max(axisWidth, widest);
    need[role] = Math.max(0, natural - target[role]);
  }
  const leftover = availableWidth() - ROLES.reduce((sum, role) => sum + target[role], 0);
  const totalNeed = ROLES.reduce((sum, role) => sum + need[role], 0);
  if (leftover > 0 && totalNeed > 0) {
    const share = Math.min(1, leftover / totalNeed);
    for (const role of ROLES) target[role] += need[role] * share;
  }
  return splitScalarBand(base, (id) => boxWidths.get(id) ?? 0, target);
}

function render(): void {
  if (!state) return;
  state.layout = fit(state.base, state.folded);
  state.plan = planRoutes(state.model, state.layout);
  renderView(viewEl, state.layout, { folded: state.folded });
  state.plan.lanes.forEach((count, i) => {
    const channel = viewEl.querySelector<HTMLElement>(`.channel[data-channel="${i}"]`);
    if (channel) channel.style.height = `${channelHeight(count)}px`;
  });
  redraw();
}

function load(name: string): void {
  const raw = fixtures[name];
  if (raw === undefined) return;
  const model = parseViewModel(raw);
  const base = computeLayout(model);
  const folded = new Set(
    [...base.columns.values()].filter((c) => c.foldedByDefault).map((c) => c.node.id),
  );
  state = { model, base, layout: base, plan: planRoutes(model, base), folded, selection: null };
  fileEl.replaceChildren();
  fileEl.append(
    Object.assign(document.createElement("span"), {
      className: "pkg",
      textContent: model.module.package,
    }),
    Object.assign(document.createElement("span"), { className: "sep", textContent: "/" }),
    Object.assign(document.createElement("span"), {
      textContent: model.module.file.split("/").at(-1) ?? model.module.file,
    }),
  );
  const unrolled = base.bands.filter((b) => b.axis !== null);
  unrollEl.textContent =
    unrolled.length === 0
      ? "no shared axis"
      : unrolled.map((b) => `unroll ${b.axis} · ${b.labels.length} labels`).join("   ");
  render();
}

viewEl.addEventListener("click", (event) => {
  if (!state) return;
  const target = event.target as HTMLElement;
  const folded = target.closest<HTMLElement>(".col.folded");
  if (folded?.dataset.node) {
    state.folded.delete(folded.dataset.node);
    state.selection = null;
    render();
    return;
  }
  const foldButton = target.closest<HTMLElement>(".fold-button");
  if (foldButton) {
    const id = foldButton.closest<HTMLElement>(".col")?.dataset.node;
    if (id) {
      state.folded.add(id);
      state.selection = null;
      render();
    }
    return;
  }
  const hit = target.closest<HTMLElement>("[data-node]");
  if (!hit?.dataset.node) {
    state.selection = null;
    applySelection();
    return;
  }
  const id = hit.dataset.node;
  const row = hit.dataset.row ?? null;
  const sub = hit.dataset.sub ?? null;
  const same =
    state.selection &&
    state.selection.id === id &&
    state.selection.row === row &&
    state.selection.sub === sub;
  state.selection = same
    ? { id, row, sub, mode: state.selection?.mode === "deps" ? "uses" : "deps" }
    : { id, row, sub, mode: "deps" };
  applySelection();
});

viewEl.addEventListener("dblclick", (event) => {
  if (!state) return;
  const head = (event.target as HTMLElement).closest<HTMLElement>(".col:not(.folded) .col-head");
  const id = head?.dataset.node;
  if (!id || layoutRows(id) === 0) return;
  state.folded.add(id);
  state.selection = null;
  render();
});

function layoutRows(id: string): number {
  return state?.layout.columns.get(id)?.rows.length ?? 0;
}

for (const name of Object.keys(fixtures).sort()) {
  const option = document.createElement("option");
  option.value = name;
  option.textContent = name.replace("../fixtures/", "").replace(".view.json", "");
  fixtureEl.append(option);
}
fixtureEl.addEventListener("change", () => load(fixtureEl.value));
let lastWidth = document.documentElement.clientWidth;
window.addEventListener("resize", () => {
  const width = document.documentElement.clientWidth;
  if (width === lastWidth) return;
  lastWidth = width;
  render();
});
new ResizeObserver(() => redraw()).observe(viewEl);
if (fixtureEl.value) load(fixtureEl.value);
