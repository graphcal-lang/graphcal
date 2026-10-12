import {
  BOX_GAP,
  computeLayout,
  orderByBarycenter,
  packScalarBand,
  ZONES,
  zoneOf,
  type BoxSize,
  type Layout,
  type Zone,
} from "./layout.ts";
import { parseViewModel, type ViewModel } from "./model.ts";
import { renderView, RULER_WIDTH } from "./render.ts";
import { trace, type Selection } from "./selection.ts";
import { displayOf, valueAt } from "./values.ts";
import { drawWires, planRoutes, type RoutePlan } from "./wires.ts";

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
const stageEl = need<HTMLElement>("#stage");
const canvasEl = need<HTMLElement>("#canvas");
const zoomEl = need<HTMLButtonElement>("#zoom");
const zoomPopover = need<HTMLElement>("#zoom-popover");
const zoomRange = need<HTMLInputElement>("#zoom-range");
const zoomNumber = need<HTMLInputElement>("#zoom-number");

/** The canvas is a free sheet: the bars stay put, the stage pans and zooms. */
const camera = { x: 0, y: 0, scale: 1 };
function applyCamera(): void {
  stageEl.style.transform = `translate(${camera.x}px, ${camera.y}px) scale(${camera.scale})`;
  const percent = String(Math.round(camera.scale * 100));
  zoomEl.textContent = `${percent}%`;
  zoomRange.value = percent;
  if (document.activeElement !== zoomNumber) zoomNumber.value = percent;
}

/** Zoom to an absolute scale, keeping the point under the canvas centre fixed. */
function zoomTo(scale: number): void {
  const next = Math.min(4, Math.max(0.1, scale));
  const cx = canvasEl.clientWidth / 2;
  const cy = canvasEl.clientHeight / 2;
  camera.x = cx - ((cx - camera.x) * next) / camera.scale;
  camera.y = cy - ((cy - camera.y) * next) / camera.scale;
  camera.scale = next;
  applyCamera();
  leaveMode();
}

/** The user moved the camera by hand: no mode is active any more, the percent is. */
function leaveMode(): void {
  for (const button of modeButtons) button.classList.remove("active");
  zoomEl.classList.add("active");
}
function resetCamera(): void {
  camera.x = 0;
  camera.y = 0;
  camera.scale = 1;
  applyCamera();
}
const modeButtons = [...document.querySelectorAll<HTMLButtonElement>("[data-mode]")];

/**
 * fit: keep text readable, fold low-activity intermediates until the view fits.
 * overview: expand everything and scale the whole stage down to fit.
 */
type Mode = "fit" | "overview";

interface State {
  model: ViewModel;
  /** Layout before the no-axis band is split into tiers. */
  base: Layout;
  layout: Layout;
  plan: RoutePlan;
  folded: Set<string>;
  selection: Selection | null;
  mode: Mode;
}
let state: State | null = null;

function describeSelection(s: State): string {
  const { selection, model } = s;
  if (!selection) return "";
  const node = model.nodes.find((n) => n.id === selection.id);
  const labels = [selection.row, selection.sub].filter((x): x is string => x !== null);
  const subject = labels.length > 0 ? `${selection.id}[${labels.join(", ")}]` : selection.id;
  const value = displayOf(valueAt(node?.value ?? null, labels));
  const expr = node?.expr ? ` = ${node.expr}` : "";
  return `${subject}${expr} = ${value}`;
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

/** Horizontal room for zones: the window minus ruler, paddings, and the gutter. */
function availableWidth(): number {
  const zonePadding = 3 * 24;
  return document.documentElement.clientWidth - 16 - 40 - RULER_WIDTH - zonePadding - 56;
}

/**
 * Render once to measure boxes and axis-band zones, then pack the no-axis
 * boxes into stacks so everything fits the width. Width left over after a
 * first packing is handed to the zones that had to stack, and they repack.
 */
function fit(base: Layout, folded: ReadonlySet<string>): Layout {
  renderView(viewEl, base, { folded });
  const sizes = new Map<string, BoxSize>();
  for (const col of viewEl.querySelectorAll<HTMLElement>(".band-scalar .col")) {
    if (!col.dataset.node) continue;
    const rect = col.getBoundingClientRect();
    sizes.set(col.dataset.node, { width: rect.width, height: rect.height });
  }
  const size = (id: string): BoxSize => sizes.get(id) ?? { width: 0, height: 0 };

  const floor = {} as Record<Zone, number>;
  const natural = {} as Record<Zone, number>;
  for (const zone of ZONES) {
    let axisWidth = 0;
    for (const zoneEl of viewEl.querySelectorAll<HTMLElement>(
      `.band-axis .zone[data-zone="${zone}"]`,
    )) {
      const cols = [...zoneEl.querySelectorAll<HTMLElement>(":scope > .col")];
      const inner =
        cols.reduce((sum, col) => sum + col.getBoundingClientRect().width, 0) +
        Math.max(0, cols.length - 1) * BOX_GAP;
      axisWidth = Math.max(axisWidth, inner);
    }
    const boxes = base.bands
      .filter((b) => b.axis === null)
      .flatMap((b) => b.columns)
      .filter((c) => zoneOf(c.role) === zone)
      .map((c) => size(c.node.id).width);
    floor[zone] = Math.max(axisWidth, ...boxes);
    natural[zone] = boxes.reduce((sum, w) => sum + w, 0) + Math.max(0, boxes.length - 1) * BOX_GAP;
  }

  const available = availableWidth();
  const target = { ...floor };
  let packed = packScalarBand(base, size, target, folded);
  for (let round = 0; round < 3; round++) {
    const leftover =
      available - ZONES.reduce((sum, zone) => sum + Math.max(target[zone], packed.used[zone]), 0);
    const need = {} as Record<Zone, number>;
    let totalNeed = 0;
    for (const zone of ZONES) {
      need[zone] = packed.stacked[zone] ? Math.max(0, natural[zone] - target[zone]) : 0;
      totalNeed += need[zone];
    }
    if (leftover <= 0 || totalNeed === 0) break;
    const share = Math.min(1, leftover / totalNeed);
    for (const zone of ZONES) target[zone] += need[zone] * share;
    packed = packScalarBand(base, size, target, folded);
  }
  return packed.layout;
}

function renderOnce(): void {
  if (!state) return;
  resetCamera();
  state.layout = fit(state.base, state.folded);
  state.plan = planRoutes(state.model, state.layout);
  renderView(viewEl, state.layout, { folded: state.folded });
  redraw();
}

function overflows(): boolean {
  return viewEl.scrollWidth > canvasEl.clientWidth || viewEl.scrollHeight > canvasEl.clientHeight;
}

/** Re-apply the mode from scratch: fold candidates one by one (fit) or scale (overview). */
function refit(): void {
  if (!state) return;
  state.folded = new Set();
  renderOnce();
  if (state.mode === "fit") {
    const candidates = [...state.base.columns.values()]
      .filter((c) => c.foldable)
      .sort((a, b) => a.activity - b.activity || b.depth - a.depth)
      .map((c) => c.node.id);
    for (const id of candidates) {
      if (!overflows()) break;
      state.folded.add(id);
      renderOnce();
    }
  } else {
    const width = canvasEl.clientWidth;
    const height = canvasEl.clientHeight;
    camera.scale = Math.min(1, width / viewEl.scrollWidth, height / viewEl.scrollHeight);
    camera.x = Math.max(0, (width - viewEl.scrollWidth * camera.scale) / 2);
    camera.y = Math.max(0, (height - viewEl.scrollHeight * camera.scale) / 2);
    applyCamera();
  }
  for (const button of modeButtons) {
    button.classList.toggle("active", button.dataset.mode === state.mode);
  }
  zoomEl.classList.remove("active");
}

/** Render after a manual fold or unfold: the view may overflow now, and that is fine. */
function render(): void {
  renderOnce();
}

function load(name: string): void {
  const raw = fixtures[name];
  if (raw === undefined) return;
  const model = parseViewModel(raw);
  const base = orderByBarycenter(computeLayout(model), model);
  state = {
    model,
    base,
    layout: base,
    plan: planRoutes(model, base),
    folded: new Set(),
    selection: null,
    mode: state?.mode ?? "fit",
  };
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
  refit();
}

viewEl.addEventListener("click", (event) => {
  if (!state) return;
  const target = event.target as HTMLElement;
  const folded = target.closest<HTMLElement>(".col.folded, .col.chip");
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
  const head = (event.target as HTMLElement).closest<HTMLElement>(
    ".col:not(.folded):not(.chip) .col-head",
  );
  const id = head?.dataset.node;
  if (!id || !state.base.columns.get(id)?.foldable) return;
  state.folded.add(id);
  state.selection = null;
  render();
});

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
  refit();
});
// Wheel: pan; with ctrl / cmd (or a trackpad pinch): zoom around the cursor.
canvasEl.addEventListener(
  "wheel",
  (event) => {
    event.preventDefault();
    if (event.ctrlKey || event.metaKey) {
      const rect = canvasEl.getBoundingClientRect();
      const px = event.clientX - rect.left;
      const py = event.clientY - rect.top;
      const next = Math.min(4, Math.max(0.1, camera.scale * Math.exp(-event.deltaY * 0.01)));
      camera.x = px - ((px - camera.x) * next) / camera.scale;
      camera.y = py - ((py - camera.y) * next) / camera.scale;
      camera.scale = next;
    } else {
      camera.x -= event.deltaX;
      camera.y -= event.deltaY;
    }
    applyCamera();
    leaveMode();
  },
  { passive: false },
);

// Drag on empty canvas: pan.
let drag: { x: number; y: number; moved: boolean } | null = null;
canvasEl.addEventListener("pointerdown", (event) => {
  if ((event.target as HTMLElement).closest(".col")) return;
  drag = { x: event.clientX, y: event.clientY, moved: false };
  canvasEl.setPointerCapture(event.pointerId);
  canvasEl.classList.add("panning");
});
canvasEl.addEventListener("pointermove", (event) => {
  if (!drag) return;
  camera.x += event.clientX - drag.x;
  camera.y += event.clientY - drag.y;
  drag = { x: event.clientX, y: event.clientY, moved: true };
  applyCamera();
  leaveMode();
});
canvasEl.addEventListener("pointerup", () => {
  drag = null;
  canvasEl.classList.remove("panning");
});

// Zoom popover: slider (snaps to 100%) and a number field.
zoomEl.addEventListener("click", () => {
  zoomPopover.hidden = !zoomPopover.hidden;
  if (!zoomPopover.hidden) {
    const rect = zoomEl.getBoundingClientRect();
    zoomPopover.style.left = `${Math.min(rect.left, window.innerWidth - zoomPopover.offsetWidth - 8)}px`;
    zoomPopover.style.top = `${rect.bottom + 4}px`;
    zoomRange.focus();
  }
});
zoomRange.addEventListener("input", () => {
  let percent = Number(zoomRange.value);
  if (Math.abs(percent - 100) <= 4) percent = 100;
  zoomTo(percent / 100);
});
zoomNumber.addEventListener("change", () => {
  const percent = Number(zoomNumber.value);
  if (Number.isFinite(percent) && percent > 0) zoomTo(percent / 100);
});
zoomNumber.addEventListener("keydown", (event) => {
  if (event.key === "Enter") zoomNumber.blur();
});
document.addEventListener("pointerdown", (event) => {
  const target = event.target as HTMLElement;
  if (!zoomPopover.hidden && !zoomPopover.contains(target) && target !== zoomEl) {
    zoomPopover.hidden = true;
  }
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") zoomPopover.hidden = true;
});

for (const button of modeButtons) {
  button.addEventListener("click", () => {
    if (!state || !button.dataset.mode) return;
    state.mode = button.dataset.mode as Mode;
    refit();
  });
}
new ResizeObserver(() => redraw()).observe(viewEl);
if (fixtureEl.value) load(fixtureEl.value);
