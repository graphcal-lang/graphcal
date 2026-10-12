import { computeLayout } from "./layout.ts";
import { parseViewModel, type ViewModel } from "./model.ts";
import { renderView } from "./render.ts";
import { trace, type Selection } from "./selection.ts";
import { drawWires, edgesOf } from "./wires.ts";

const fixtures = import.meta.glob<unknown>("../fixtures/*.view.json", {
  eager: true,
  import: "default",
});

const viewEl = document.querySelector<HTMLElement>("#view");
const wiresEl = document.querySelector<SVGSVGElement>("#wires");
const fileEl = document.querySelector<HTMLElement>("#file");
const statusEl = document.querySelector<HTMLElement>("#status");
const fixtureEl = document.querySelector<HTMLSelectElement>("#fixture");
if (!viewEl || !wiresEl || !fileEl || !statusEl || !fixtureEl) {
  throw new Error("view shell is missing required elements");
}

let model: ViewModel | null = null;
let selection: Selection | null = null;

function applySelection(): void {
  if (!model || !viewEl || !wiresEl || !statusEl) return;
  const layout = computeLayout(model);
  for (const col of viewEl.querySelectorAll<HTMLElement>(".col")) {
    col.classList.remove("sel", "rel-direct", "rel-all");
  }
  for (const wire of wiresEl.querySelectorAll<SVGElement>(".wire, .wire-dot")) {
    wire.classList.remove("lit", "direct");
    if (wire.classList.contains("wire")) wire.setAttribute("marker-end", "url(#arrow)");
  }
  if (!selection) {
    const nodeCount = model.nodes.length;
    const bands = layout.bands.length;
    statusEl.textContent = `${nodeCount} nodes · ${bands} bands · click a column to trace`;
    return;
  }
  const result = trace(selection, model, layout);
  viewEl.querySelector(`.col[data-node="${selection.id}"]`)?.classList.add("sel");
  for (const id of result.all) {
    viewEl
      .querySelector(`.col[data-node="${id}"]`)
      ?.classList.add(result.direct.has(id) ? "rel-direct" : "rel-all");
  }
  for (const wire of wiresEl.querySelectorAll<SVGElement>(".wire, .wire-dot")) {
    const key = `${wire.dataset.from}->${wire.dataset.to}`;
    const direct = result.edges.get(key);
    if (direct === undefined) continue;
    wire.classList.add("lit");
    if (direct) wire.classList.add("direct");
    if (wire.classList.contains("wire")) wire.setAttribute("marker-end", "url(#arrow-lit)");
  }
  const arrow = selection.mode === "deps" ? "← depends on" : "→ used by";
  const names = [...result.direct].join(", ") || "nothing";
  statusEl.textContent = `${selection.id} ${arrow} ${names} · ${result.all.size} total · click again to flip`;
}

function redraw(): void {
  if (!model || !viewEl || !wiresEl) return;
  const layout = computeLayout(model);
  drawWires(wiresEl, viewEl, edgesOf(model, layout));
  applySelection();
}

function load(name: string): void {
  const raw = fixtures[name];
  if (raw === undefined || !viewEl || !fileEl) return;
  model = parseViewModel(raw);
  selection = null;
  fileEl.textContent = `${model.module.file} · ${model.module.package}`;
  renderView(viewEl, model, computeLayout(model));
  redraw();
}

viewEl.addEventListener("click", (event) => {
  const target = (event.target as HTMLElement).closest<HTMLElement>("[data-node]");
  if (!target || !target.dataset.node) {
    selection = null;
    applySelection();
    return;
  }
  const id = target.dataset.node;
  selection =
    selection && selection.id === id
      ? { id, mode: selection.mode === "deps" ? "uses" : "deps" }
      : { id, mode: "deps" };
  applySelection();
});

for (const name of Object.keys(fixtures).sort()) {
  const option = document.createElement("option");
  option.value = name;
  option.textContent = name.replace("../fixtures/", "").replace(".view.json", "");
  fixtureEl.append(option);
}
fixtureEl.addEventListener("change", () => load(fixtureEl.value));
new ResizeObserver(() => redraw()).observe(viewEl);
if (fixtureEl.value) load(fixtureEl.value);
