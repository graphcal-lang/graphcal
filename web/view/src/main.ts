import { computeLayout, type Layout } from "./layout.ts";
import { parseViewModel, type ViewModel } from "./model.ts";
import { renderView } from "./render.ts";
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
  drawWires(wiresEl, viewEl, state.layout, state.plan);
  applySelection();
}

function render(): void {
  if (!state) return;
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
  const layout = computeLayout(model);
  const folded = new Set(
    [...layout.columns.values()].filter((c) => c.foldedByDefault).map((c) => c.node.id),
  );
  state = { model, layout, plan: planRoutes(model, layout), folded, selection: null };
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
  const unrolled = layout.bands.filter((b) => b.axis !== null);
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
new ResizeObserver(() => redraw()).observe(viewEl);
if (fixtureEl.value) load(fixtureEl.value);
