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
import { diffModels, markChanges, previousValue, summarize, type ModelDiff } from "./compare.ts";
import { crossLinks, drawCrossWires, extraLanes, type CrossSheet } from "./cross.ts";
import { SourceEditor, type Inlay } from "./editor.ts";
import { Explorer, type RefSource } from "./explorer.ts";
import { spansOf, subjectTokens, tokenize, type Token } from "./formula.ts";
import { mountGitHubBar } from "./githubBar.ts";
import { GitHubHost } from "./githubHost.ts";
import type { Host, NoteKind } from "./host.ts";
import { TokenStore } from "./login.ts";
import type { ViewModel, ViewNode } from "./model.ts";
import { depthsOf, sameKey, type PaneKey } from "./panes.ts";
import { parseRoute } from "./route.ts";
import { EditError, replaceExpr, replaceRange, replaceTableCell } from "./source.ts";
import { EmptyHost, mountStart } from "./start.ts";
import { inVsCode, VsCodeHost } from "./vscodeHost.ts";
import { EXTERNAL_MARK, placeHalo, renderView, RULER_WIDTH, type RenderOptions } from "./render.ts";
import { hiddenOf, pick, type Hidden, type Picked } from "./rows.ts";
import { trace, type Selection } from "./selection.ts";
import { displayOf, oneLine, valueAt } from "./values.ts";
import { drawWires, planRoutes, type RoutePlan } from "./wires.ts";

/**
 * Where models come from and edits go: the extension in VS Code; a GitHub
 * repository when the address names one (`#owner/repo@ref/path.gcl`); with
 * no address the page only asks for one.
 */
const route = inVsCode() ? null : parseRoute(location.hash);
const tokens = new TokenStore();
const host: Host = inVsCode()
  ? new VsCodeHost()
  : route
    ? new GitHubHost(route, tokens)
    : new EmptyHost();
/** The web shell has the explorer, the code pane and the layout toggle; VS Code brings its own. */
const webShell = !inVsCode() && route !== null;

/** Whether the host has a model for `module`. */
function known(module: string): boolean {
  return host.model(module) !== null;
}

/** `power_max.gcl` for a module id, from its model's file. */
function fileNameOfModule(module: string): string {
  const model = host.model(module);
  return model ? fileNameOf(model) : `${module.split(".").at(-1) ?? module}.gcl`;
}

function need<T extends Element>(selector: string): T {
  const found = document.querySelector<T>(selector);
  if (!found) throw new Error(`view shell is missing ${selector}`);
  return found;
}

const SVG_NS = "http://www.w3.org/2000/svg";
const panesEl = need<HTMLElement>("#panes");
const crossEl = need<SVGSVGElement>("#cross");
const fileEl = need<HTMLElement>("#file");
const originEl = need<HTMLElement>("#origin");
const crumbEl = need<HTMLButtonElement>("#crumb");
const githubEl = need<HTMLElement>("#github");
const workEl = need<HTMLElement>("#work");
const explorerEl = need<HTMLElement>("#explorer");
const codeEl = need<HTMLElement>("#code");
const codeFileEl = need<HTMLElement>("#code-file");
const codeNoteEl = need<HTMLElement>("#code-note");
const codeBodyEl = need<HTMLElement>("#code-body");
const splitEl = need<HTMLElement>("#split");
const sheetEl = need<HTMLElement>("#sheet");
const sheetNameEl = need<HTMLElement>("#sheet-name");
const layoutsEl = need<HTMLElement>("#layouts");
const formulaEl = need<HTMLElement>("#formula");
const startEl = need<HTMLElement>("#start");
const stageEl = need<HTMLElement>("#stage");
const canvasEl = need<HTMLElement>("#canvas");
const zoomEl = need<HTMLButtonElement>("#zoom");
const zoomPopover = need<HTMLElement>("#zoom-popover");
const zoomNumber = need<HTMLInputElement>("#zoom-number");

/** The canvas is a free sheet: the bars stay put, the stage pans and zooms. */
const camera = { x: 0, y: 0, scale: 1 };
function applyCamera(): void {
  stageEl.style.transform = `translate(${camera.x}px, ${camera.y}px) scale(${camera.scale})`;
  const percent = String(Math.round(camera.scale * 100));
  zoomEl.textContent = `${percent}%`;
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
 * fit: text at full size; low-activity intermediate nodes are folded, one by
 * one, until the sheet fits the canvas, or until none are left to fold (then
 * it overflows and is panned). overview: everything open, the whole stage
 * scaled down to fit.
 */
type Mode = "fit" | "overview";

/** One module shown as a sheet: the main one, or a pane pinned beside it. */
interface Sheet {
  key: PaneKey;
  model: ViewModel;
  /** Layout before the no-axis band is split into tiers. */
  base: Layout;
  layout: Layout;
  plan: RoutePlan;
  folded: Set<string>;
  /** Hidden row labels per axis. */
  hidden: Hidden;
  /** Row labels picked in a ruler (the subject of the context menu). */
  picked: Picked | null;
  selection: Selection | null;
  paneEl: HTMLElement;
  viewEl: HTMLElement;
  wiresEl: SVGSVGElement;
  /** Pane column: 0 for the main sheet, growing leftwards. */
  depth: number;
}

interface Workspace {
  /** The main sheet first, then the pinned panes in the order they were pinned. */
  sheets: Sheet[];
  /** Modules opened in turn with "Open"; the last is the main sheet's. */
  trail: string[];
  mode: Mode;
}
let ws: Workspace | null = null;

function mainSheet(w: Workspace): Sheet {
  return w.sheets[0];
}

/** The sheet holding the selection, else the main one. */
function activeSheet(w: Workspace): Sheet {
  return w.sheets.find((s) => s.selection !== null) ?? mainSheet(w);
}

function sheetOf(target: Element): Sheet | null {
  const pane = target.closest(".pane");
  return ws?.sheets.find((s) => s.paneEl === pane) ?? null;
}

function renderOptions(s: Sheet): RenderOptions {
  return { folded: s.folded, hidden: s.hidden };
}

/** What changed in a sheet since the base ref, when the address compares two refs and the base is read. */
function diffOf(s: Sheet): ModelDiff | null {
  const compare = host.compare ?? null;
  if (!compare) return null;
  return diffModels(compare.model(s.model.module.id), s.model);
}

function fileNameOf(model: ViewModel): string {
  return model.module.file.split("/").at(-1) ?? model.module.file;
}

function span(className: string, textContent: string): HTMLSpanElement {
  return Object.assign(document.createElement("span"), { className, textContent });
}

function button(className: string, textContent: string, title: string): HTMLButtonElement {
  return Object.assign(document.createElement("button"), {
    type: "button",
    className,
    textContent,
    title,
  });
}

function createSheet(key: PaneKey): Sheet {
  const model = host.model(key.module);
  if (!model) throw new Error(`no model for ${key.module}`);
  const base = orderByBarycenter(computeLayout(model), model);
  const paneEl = document.createElement("section");
  paneEl.className = "pane";
  const headEl = document.createElement("header");
  headEl.className = "pane-head";
  const viewEl = document.createElement("main");
  viewEl.className = "view";
  const wiresEl = document.createElementNS(SVG_NS, "svg");
  wiresEl.setAttribute("class", "wires");
  wiresEl.setAttribute("aria-hidden", "true");
  paneEl.append(headEl, viewEl, wiresEl);
  return {
    key,
    model,
    base,
    layout: base,
    plan: planRoutes(model, base),
    folded: new Set(),
    hidden: new Map(),
    picked: null,
    selection: null,
    paneEl,
    viewEl,
    wiresEl,
    depth: 0,
  };
}

/** `↗ eps.gcl as eps · include · pinned ×`, or just the file name for the main sheet. */
function renderPaneHead(w: Workspace, sheet: Sheet): void {
  const head = sheet.paneEl.querySelector<HTMLElement>(".pane-head");
  if (!head) return;
  head.replaceChildren();
  const name = fileNameOf(sheet.model);
  if (sheet === mainSheet(w)) {
    head.append(span("pane-name", name));
    return;
  }
  const sub =
    sheet.key.instance === null
      ? "· import · pinned"
      : `as ${sheet.key.instance} · include · pinned`;
  head.append(
    span("pane-mark", EXTERNAL_MARK),
    button("pane-file", name, `open ${name}`),
    span("pane-sub", sub),
    button("pane-close", "×", "unpin"),
  );
}

/** Put the sheets into pane columns by depth: deepest at the left, the main sheet at the right. */
function placePanes(w: Workspace): void {
  const depths = depthsOf(w.sheets.map((s) => ({ key: s.key, model: s.model })));
  w.sheets.forEach((s, i) => {
    s.depth = depths[i] ?? 0;
  });
  panesEl.replaceChildren();
  panesEl.classList.toggle("multi", w.sheets.length > 1);
  for (let depth = Math.max(...depths); depth >= 0; depth--) {
    const column = document.createElement("div");
    column.className = "pane-col";
    for (const sheet of w.sheets) if (sheet.depth === depth) column.append(sheet.paneEl);
    panesEl.append(column);
  }
  for (const sheet of w.sheets) renderPaneHead(w, sheet);
}

/** The selected node and the labels of the selected cell, one per axis. */
function selected(s: Sheet): { node: ViewNode; labels: string[] } | null {
  const { selection, model } = s;
  if (!selection) return null;
  const node = model.nodes.find((n) => n.id === selection.id);
  if (!node) return null;
  const labels = [selection.row, selection.sub].filter((x): x is string => x !== null);
  return { node, labels };
}

/** The expression as edited in the bar: the source text on one line, when its file is at hand. */
function editableExpr(s: Sheet, node: ViewNode): string | null {
  if (node.expr === null || node.source === null || node.source.file === null) return null;
  if (host.source(s.model, node.source.file) === null) return null;
  return oneLine(node.expr);
}

/** `subject = expr = value`, coloured like the editor; the expr part is editable. */
function renderFormula(): void {
  if (!ws) return;
  const s = activeSheet(ws);
  const line = document.createElement("span");
  line.className = "formula-line";
  const hit = selected(s);
  // A note (an error, progress) outlives the selection it was shown with.
  const note = formulaEl.querySelector(".formula-note");
  const show = (...children: Element[]): void => {
    formulaEl.replaceChildren(...children);
    if (note) formulaEl.append(note);
  };
  formulaEl.classList.toggle("active", hit !== null);
  formulaEl.classList.remove("editing");
  if (!hit) {
    show(line);
    return;
  }
  const { node, labels } = hit;
  const eq = (): Token => ({ text: " = ", cls: "operator" });
  line.append(...spansOf(subjectTokens(node.id, labels)));
  if (node.expr) {
    const expr = document.createElement("span");
    expr.className = "formula-expr";
    expr.classList.toggle("editable", editableExpr(s, node) !== null);
    const coloured = document.createElement("span");
    coloured.className = "formula-coloured";
    coloured.append(...spansOf(tokenize(oneLine(node.expr))));
    expr.append(coloured);
    line.append(...spansOf([eq()]), expr);
  }
  line.append(...spansOf([eq(), ...tokenize(displayOf(valueAt(node.value, labels)))]));
  const diff = diffOf(s);
  const change = diff?.nodes.get(node.id);
  if (change) {
    const was = previousValue(change, node.value, labels);
    const text =
      change.kind === "added"
        ? "new since the base"
        : was === undefined
          ? change.kind === "expr"
            ? `was ${oneLine(change.before.expr ?? "")}`
            : "value unchanged here"
          : `was ${displayOf(was)}`;
    line.append(span("formula-was", text));
  }
  show(line);
}

/** A note in the formula bar, mirrored in the code pane's head (the bar is hidden in code-only layout). */
function showNote(message: string | null, kind: "error" | "busy" = "error"): void {
  formulaEl.querySelector(".formula-note")?.remove();
  codeNoteEl.textContent = message ?? "";
  codeNoteEl.className = `code-note ${message === null ? "" : kind}`;
  if (message === null) return;
  const note = document.createElement("span");
  note.className = `formula-note ${kind}`;
  note.textContent = message;
  formulaEl.append(note);
}

function clearHighlights(s: Sheet): void {
  for (const n of s.viewEl.querySelectorAll<HTMLElement>(
    ".sel, .rel-direct, .rel-all, .row-sel, .rel-cell",
  )) {
    n.classList.remove("sel", "rel-direct", "rel-all", "row-sel", "rel-cell");
  }
  for (const w of s.wiresEl.querySelectorAll<SVGElement>(".wire, .wire-dot")) {
    w.classList.remove("lit", "direct");
    if (w.classList.contains("wire")) w.setAttribute("marker-end", "url(#arrow)");
  }
}

function applySelection(s: Sheet): void {
  clearHighlights(s);
  renderFormula();
  const { selection, model, layout, viewEl, wiresEl } = s;
  if (!selection) return;

  const result = trace(selection, model, layout);
  // The selected cell's labels per axis; a related cell matches when it agrees
  // on every axis the two nodes share (a sum over Component still shares Mode).
  const selectedNode = model.nodes.find((n) => n.id === selection.id);
  const selectedLabels = new Map<string, string>();
  if (selectedNode) {
    const [first, second] = selectedNode.axes;
    if (first !== undefined && selection.row !== null) selectedLabels.set(first, selection.row);
    if (second !== undefined && selection.sub !== null) selectedLabels.set(second, selection.sub);
  }
  const mark = (id: string, cls: string): void => {
    const column = layout.columns.get(id);
    const col = viewEl.querySelector<HTMLElement>(`.col[data-node="${id}"]`);
    if (!col || !column) return;
    col.classList.add(cls);
    if (id === selection.id) return;
    const [first, second] = column.node.axes;
    const shared = column.node.axes.filter((axis) => selectedLabels.has(axis));
    if (shared.length === 0) return;
    for (const cell of col.querySelectorAll<HTMLElement>(".cell")) {
      const labelOf = (axis: string): string | undefined =>
        axis === first ? cell.dataset.row : axis === second ? cell.dataset.sub : undefined;
      if (shared.every((axis) => labelOf(axis) === selectedLabels.get(axis))) {
        cell.classList.add("rel-cell");
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

/** Picked row labels: the label and the whole row across the band, like a sheet. */
function applyPicked(s: Sheet): void {
  for (const n of s.viewEl.querySelectorAll<HTMLElement>(".picked, .row-picked")) {
    n.classList.remove("picked", "row-picked");
  }
  const picked = s.picked;
  if (!picked) return;
  for (const label of picked.labels) {
    for (const n of s.viewEl.querySelectorAll<HTMLElement>(`.row-label[data-row="${label}"]`)) {
      if (n.dataset.axis === picked.axis) n.classList.add("picked");
    }
    const band = bandElementOf(s, picked.axis);
    for (const cell of band?.querySelectorAll<HTMLElement>(`.cell[data-row="${label}"]`) ?? []) {
      cell.classList.add("row-picked");
    }
  }
}

function bandElementOf(s: Sheet, axis: string): HTMLElement | null {
  const index = s.layout.bands.find((b) => b.axis === axis)?.index;
  return index === undefined ? null : s.viewEl.querySelector(`.band[data-band="${index}"]`);
}

/** Draw every sheet's wires, then the wires between sheets, and re-apply highlights. */
function redraw(): void {
  if (!ws) return;
  const links = crossLinks(ws.sheets);
  const ownLanes = new Map<CrossSheet, Map<number, number>>();
  for (const s of ws.sheets) {
    ownLanes.set(s, drawWires(s.wiresEl, s.viewEl, s.layout, s.plan, extraLanes(links, s)));
    placeHalo(s.viewEl);
    applySelection(s);
    applyPicked(s);
  }
  drawCrossWires(crossEl, panesEl, links, ownLanes);
}

/** Horizontal room for the main sheet's zones: the canvas minus ruler, paddings, and the gutter. */
function availableWidth(): number {
  const zonePadding = 3 * 24;
  return canvasEl.clientWidth - 16 - 40 - RULER_WIDTH - zonePadding - 56;
}

/** A pinned pane gets a fixed budget, so its scalar boxes stack into a narrow column. */
const PANE_WIDTH = 640;

/**
 * Render once to measure boxes and axis-band zones, then pack the no-axis
 * boxes into stacks so everything fits the width. Width left over after a
 * first packing is handed to the zones that had to stack, and they repack.
 */
function fit(s: Sheet, available: number): Layout {
  const { base, viewEl } = s;
  const options = renderOptions(s);
  const { folded } = options;
  renderView(viewEl, base, options);
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
      const cols = [...zoneEl.querySelectorAll<HTMLElement>(".col")];
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

/** Lay out and draw every sheet; the camera stays where it is (measuring needs an unscaled stage). */
function render(): void {
  if (!ws) return;
  stageEl.style.transform = "";
  for (const s of ws.sheets) {
    s.layout = fit(s, s === mainSheet(ws) ? availableWidth() : PANE_WIDTH);
    s.plan = planRoutes(s.model, s.layout);
    renderView(s.viewEl, s.layout, renderOptions(s));
    const diff = diffOf(s);
    if (diff) markChanges(s.viewEl, s.model, diff);
  }
  applyCamera();
  redraw();
}

/** Re-apply the mode from scratch: everything open, then scaled down in overview. */
function refit(): void {
  if (!ws) return;
  for (const s of ws.sheets) s.folded = new Set();
  resetCamera();
  render();
  if (ws.mode === "overview") fitOverview();
  else foldToFit(mainSheet(ws));
  for (const button of modeButtons) {
    button.classList.toggle("active", button.dataset.mode === ws.mode);
  }
  zoomEl.classList.remove("active");
}

function overflows(): boolean {
  return panesEl.scrollWidth > canvasEl.clientWidth || panesEl.scrollHeight > canvasEl.clientHeight;
}

/**
 * Fit mode's folding: the sheet's intermediate nodes (never an output) are
 * folded in order of activity, fewest consumers first and deepest first
 * among equals, until the stage fits the canvas. What is still too wide with
 * every candidate folded overflows, and that is fine.
 */
function foldToFit(s: Sheet): void {
  const candidates = [...s.base.columns.values()]
    .filter((c) => c.foldable)
    .sort((a, b) => a.activity - b.activity || b.depth - a.depth)
    .map((c) => c.node.id);
  for (const id of candidates) {
    if (!overflows()) break;
    s.folded.add(id);
    render();
  }
}

/** Scale the whole stage down to fit the canvas, centred. */
function fitOverview(): void {
  const width = canvasEl.clientWidth;
  const height = canvasEl.clientHeight;
  camera.scale = Math.min(1, width / panesEl.scrollWidth, height / panesEl.scrollHeight);
  camera.x = Math.max(0, (width - panesEl.scrollWidth * camera.scale) / 2);
  camera.y = Math.max(0, (height - panesEl.scrollHeight * camera.scale) / 2);
  applyCamera();
}

/**
 * The canvas changed size: the mode is applied again for the new width,
 * unless the zoom was set by hand, in which case the sheets are just laid
 * out again with their folds kept.
 */
function relayout(): void {
  if (!ws) return;
  if (zoomEl.classList.contains("active")) {
    render();
    return;
  }
  // The mode is applied again for the new width: fit folds afresh, overview rescales.
  refit();
}

/** `package / a.gcl › b.gcl`: the files opened in turn; an earlier one goes back. */
function renderTrail(w: Workspace): void {
  fileEl.replaceChildren(span("pkg", mainSheet(w).model.module.package), span("sep", "/"));
  w.trail.forEach((module, i) => {
    const name = fileNameOfModule(module);
    if (i > 0) fileEl.append(span("sep", "›"));
    if (i === w.trail.length - 1) {
      fileEl.append(span("", name));
      return;
    }
    const crumb = button("crumb", name, "go back");
    crumb.addEventListener("click", () => open(w.trail.slice(0, i + 1)));
    fileEl.append(crumb);
  });
  fileEl.append(copyEl, resetEl);
}

/** Show `trail`'s last module as the main sheet, with no panes pinned. */
function open(trail: string[]): void {
  const module = trail.at(-1);
  if (module === undefined || !known(module)) return;
  const main = createSheet({ module, instance: null });
  ws = { sheets: [main], trail, mode: ws?.mode ?? "fit" };
  placePanes(ws);
  renderTrail(ws);
  markEdited();
  refit();
  renderSheetHead();
  syncEditor();
  renderExplorer();
  host.shown?.(module);
}

/** `power_max.gcl · sheet`, with what changed since the base when comparing. */
function renderSheetHead(): void {
  if (!ws) return;
  const main = mainSheet(ws);
  sheetNameEl.replaceChildren(span("", fileNameOf(main.model)), span("sub", "· sheet"));
  const diff = diffOf(main);
  if (diff) {
    sheetNameEl.append(
      span("sub diff-summary", `· since ${host.compare?.ref ?? "base"}: ${summarize(diff)}`),
    );
  }
}

/** Panes asked for before the host had their model. */
const pendingPins: PaneKey[] = [];

/** Open a sheet for `key` beside its readers; a pane already open just stays. */
function pin(key: PaneKey): void {
  if (!ws || ws.sheets.some((s) => sameKey(s.key, key))) return;
  if (!known(key.module)) {
    if (!pendingPins.some((k) => sameKey(k, key))) pendingPins.push(key);
    host.request(key.module);
    return;
  }
  ws.sheets.push(createSheet(key));
  placePanes(ws);
  render();
}

/** Take fresh models from the host: every sheet showing one of them is re-laid out in place. */
function adoptModels(models: ViewModel[]): void {
  if (!ws) return;
  let changed = false;
  for (const model of models) {
    for (const sheet of ws.sheets) {
      if (sheet.model.module.id !== model.module.id) continue;
      sheet.model = model;
      sheet.base = orderByBarycenter(computeLayout(model), model);
      changed = true;
    }
  }
  for (const key of pendingPins.splice(0)) {
    if (known(key.module)) pin(key);
    else pendingPins.push(key);
  }
  if (changed) {
    render();
    renderFormula();
  }
  markEdited();
  renderSheetHead();
  syncEditor();
  renderExplorer();
}

function unpin(sheet: Sheet): void {
  if (!ws || sheet === mainSheet(ws)) return;
  ws.sheets = ws.sheets.filter((s) => s !== sheet);
  placePanes(ws);
  render();
}

/** Click on a ruler label: pick rows (shift: range, cmd / ctrl: toggle). */
function pickFromEvent(s: Sheet, label: HTMLElement, event: MouseEvent): void {
  const axis = label.dataset.axis;
  const row = label.dataset.row;
  if (axis === undefined || row === undefined) return;
  const order = s.layout.bands.find((b) => b.axis === axis)?.labels ?? [];
  s.picked = pick(s.picked, axis, order, row, {
    range: event.shiftKey,
    toggle: event.metaKey || event.ctrlKey,
  });
  applyPicked(s);
}

/** Show the rows of a collapsed gap again. */
function revealGap(s: Sheet, gap: HTMLElement): void {
  const axis = gap.dataset.axis;
  if (axis === undefined) return;
  const set = hiddenOf(s.hidden, axis);
  for (const label of (gap.dataset.labels ?? "").split("\u0000")) set.delete(label);
  render();
}

/** One selection at a time across sheets. */
function select(s: Sheet, selection: Selection | null): void {
  if (!ws) return;
  for (const other of ws.sheets) {
    if (other !== s && other.selection !== null) {
      other.selection = null;
      applySelection(other);
    }
  }
  s.selection = selection;
  applySelection(s);
  syncEditorSelection();
}

panesEl.addEventListener("click", (event) => {
  if (!ws) return;
  const target = event.target as HTMLElement;
  const s = sheetOf(target);
  if (!s) return;
  if (target.closest(".pane-file")) {
    open([...ws.trail, s.model.module.id]);
    return;
  }
  if (target.closest(".pane-close")) {
    unpin(s);
    return;
  }
  const label = target.closest<HTMLElement>(".row-label");
  if (label) {
    pickFromEvent(s, label, event);
    return;
  }
  const gap = target.closest<HTMLElement>(".row-gap[data-axis]");
  if (gap) {
    revealGap(s, gap);
    return;
  }
  if (s.picked) {
    s.picked = null;
    applyPicked(s);
  }
  const folded = target.closest<HTMLElement>(".col.folded");
  if (folded?.dataset.node) {
    s.folded.delete(folded.dataset.node);
    select(s, null);
    render();
    return;
  }
  const hit = target.closest<HTMLElement>("[data-node]");
  if (!hit?.dataset.node) {
    select(s, null);
    return;
  }
  const id = hit.dataset.node;
  const row = hit.dataset.row ?? null;
  const sub = hit.dataset.sub ?? null;
  const same =
    s.selection && s.selection.id === id && s.selection.row === row && s.selection.sub === sub;
  select(
    s,
    same
      ? { id, row, sub, mode: s.selection?.mode === "deps" ? "uses" : "deps" }
      : { id, row, sub, mode: "deps" },
  );
});

// ---- live editing ----
// Every edit is a textual change to a source file, handed to the host. The
// dev shell rewrites an in-memory copy and re-projects the open sheets in
// WASM; VS Code edits the document and the language server's fresh model
// arrives later. An edit the host refuses shows its message; the editor
// stays open with the text.

/** A pending change: which file and its new text. */
interface Edit {
  file: string;
  next: string;
}

let evaluating: Promise<void> = Promise.resolve();

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Hand an edit to the host, one at a time, and take back whatever models it re-projected. */
function applyEdit(s: Sheet, edit: Edit): Promise<boolean> {
  const current = ws;
  if (!current) return Promise.resolve(false);
  const run = async (): Promise<boolean> => {
    showNote("evaluating…", "busy");
    const open = current.sheets.map((sheet) => sheet.model.module.id);
    let result;
    try {
      result = await host.edit(s.model, edit, open);
    } catch (error) {
      showNote(errorMessage(error));
      return false;
    }
    if (ws !== current) return false;
    if (!result.ok) {
      showNote(result.message);
      return false;
    }
    showNote(null);
    adoptModels(result.models);
    return true;
  };
  const result = evaluating.then(run, run);
  evaluating = result.then(
    () => undefined,
    () => undefined,
  );
  return result;
}

/**
 * Turn typed text into a literal for the node. A quantity needs a floating-point
 * literal (`0` is an integer; `0.0` is not), so a bare number is given a decimal
 * point and the column's unit; anything else is passed through as written.
 */
function literalFor(node: ViewNode, text: string): string {
  const trimmed = text.trim();
  if (node.dimension === null) return trimmed;
  const match = /^(-?)(\d[\d_]*)?(?:\.(\d[\d_]*))?([eE][-+]?\d+)?$/.exec(trimmed);
  if (!match || (match[2] === undefined && match[3] === undefined)) return trimmed;
  const [, sign, whole = "0", fraction = "0", exponent = ""] = match;
  const number = `${sign}${whole}.${fraction}${exponent}`;
  return node.unit ? `${number} ${node.unit}` : number;
}

/** The file a node is declared in and its text, when the host has it. */
function sourceOf(s: Sheet, node: ViewNode): { file: string; source: string } | null {
  const file = node.source?.file ?? null;
  const source = file === null ? null : host.source(s.model, file);
  return file === null || source === null ? null : { file, source };
}

/** Replace the whole expression of a node (formula bar). */
function editExpr(s: Sheet, node: ViewNode, next: string): Promise<boolean> {
  const expr = editableExpr(s, node);
  const at = sourceOf(s, node);
  if (expr === null || at === null || node.source === null) {
    showNote("this expression cannot be edited");
    return Promise.resolve(false);
  }
  const { file, source } = at;
  let rewritten: string;
  try {
    // The compiler's range pins the expression; the verbatim search is the
    // fallback for a model that carries no range.
    const range = node.source.expr_range;
    rewritten = range
      ? replaceRange(source, range, next)
      : replaceExpr(source, node.source.line, node.expr ?? expr, next);
  } catch (error) {
    showNote(error instanceof EditError ? error.message : errorMessage(error));
    return Promise.resolve(false);
  }
  return applyEdit(s, { file, next: rewritten });
}

/** Replace one entry of a table literal, or the literal of a scalar param. */
function editCell(
  s: Sheet,
  node: ViewNode,
  row: string | null,
  sub: string | null,
  text: string,
): Promise<boolean> {
  if (row === null) return editExpr(s, node, literalFor(node, text));
  const at = sourceOf(s, node);
  if (at === null || node.source === null) {
    showNote("this value cannot be edited");
    return Promise.resolve(false);
  }
  const { file, source } = at;
  let rewritten: string;
  try {
    rewritten = replaceTableCell(source, node.source.line, row, sub, literalFor(node, text));
  } catch (error) {
    showNote(error instanceof EditError ? error.message : errorMessage(error));
    return Promise.resolve(false);
  }
  return applyEdit(s, { file, next: rewritten });
}

/** Wire an input: Enter or blur commits, Escape cancels; the input stays while evaluating. */
function bindEditor(
  input: HTMLInputElement,
  initial: string,
  commit: (text: string) => Promise<boolean>,
  cancel: () => void,
): void {
  let busy = false;
  let done = false;
  const finish = (): void => {
    done = true;
  };
  const submit = (): void => {
    if (busy || done) return;
    if (input.value === initial) {
      finish();
      cancel();
      return;
    }
    busy = true;
    void commit(input.value).then((ok) => {
      busy = false;
      if (ok) finish();
      else if (document.activeElement !== input && input.isConnected) input.focus();
    });
  };
  input.addEventListener("keydown", (event) => {
    event.stopPropagation();
    if (event.key === "Enter") {
      event.preventDefault();
      submit();
    } else if (event.key === "Escape") {
      event.preventDefault();
      finish();
      cancel();
    }
  });
  input.addEventListener("blur", () => {
    if (!done && !busy) submit();
  });
  input.addEventListener("pointerdown", (event) => event.stopPropagation());
}

/** Edit the selected node's expression in place in the formula bar. */
function startFormulaEdit(): void {
  if (!ws || formulaEl.classList.contains("editing")) return;
  const s = activeSheet(ws);
  const hit = selected(s);
  if (!hit) return;
  const expr = editableExpr(s, hit.node);
  const exprEl = formulaEl.querySelector<HTMLElement>(".formula-expr");
  const coloured = exprEl?.querySelector<HTMLElement>(".formula-coloured");
  if (expr === null || !exprEl || !coloured) return;
  const node = hit.node;
  const input = document.createElement("input");
  input.className = "formula-input";
  input.spellcheck = false;
  input.value = expr;
  input.setAttribute("aria-label", `expression of ${node.id}`);
  input.addEventListener("input", () => {
    coloured.replaceChildren(...spansOf(tokenize(input.value.length > 0 ? input.value : " ")));
  });
  bindEditor(
    input,
    expr,
    (text) => editExpr(s, node, text),
    () => renderFormula(),
  );
  exprEl.append(input);
  formulaEl.classList.add("editing");
  showNote(null);
  input.focus();
  input.setSelectionRange(input.value.length, input.value.length);
}

/** Edit a param cell or scalar box in place; other nodes edit through the formula bar. */
function startCellEdit(s: Sheet, cell: HTMLElement): void {
  const id = cell.dataset.node;
  const node = s.model.nodes.find((n) => n.id === id);
  if (!node) return;
  const row = cell.dataset.row ?? null;
  const sub = cell.dataset.sub ?? null;
  select(s, { id: node.id, row, sub, mode: "deps" });
  const literal = node.kind === "param" || node.kind === "const";
  if (!literal || (row === null && editableExpr(s, node) === null)) {
    startFormulaEdit();
    return;
  }
  const value = valueAt(
    node.value,
    [row, sub].filter((x): x is string => x !== null),
  );
  const initial = value?.kind === "quantity" ? value.magnitude : displayOf(value);
  const input = document.createElement("input");
  input.className = "cell-input";
  // The default `size` (20 characters) would widen the column to fit the input.
  input.size = 1;
  input.spellcheck = false;
  input.value = initial;
  input.setAttribute("aria-label", `value of ${node.id}`);
  const before = [...cell.childNodes];
  cell.replaceChildren(input);
  cell.classList.add("editing");
  bindEditor(
    input,
    initial,
    (text) => editCell(s, node, row, sub, text),
    () => {
      cell.classList.remove("editing");
      cell.replaceChildren(...before);
    },
  );
  showNote(null);
  input.focus();
  input.select();
}

formulaEl.addEventListener("click", (event) => {
  if ((event.target as HTMLElement).closest(".formula-note")) return;
  startFormulaEdit();
});

const resetEl = button("bar-reset", "reset", "discard the what-if edits and reload");
resetEl.hidden = true;
resetEl.addEventListener("click", () => {
  host.reset();
  if (ws) open(ws.trail);
});

/** The edits as a patch on the clipboard: `git apply` or the VS Code extension puts them in the files. */
const copyEl = button(
  "bar-copy",
  "copy changes",
  "copy the edits as a patch; in VS Code run “Graphcal: Apply Changes from Clipboard”",
);
copyEl.hidden = true;
copyEl.addEventListener("click", () => {
  const patch = host.patch?.() ?? "";
  if (patch === "") {
    showNote("nothing changed");
    return;
  }
  const files = patch.split("\n").filter((l) => l.startsWith("diff --git")).length;
  navigator.clipboard.writeText(patch).then(
    () => showNote(`copied the changes to ${files} file${files === 1 ? "" : "s"}`, "busy"),
    () => showNote("the clipboard is not available; the page needs to be focused"),
  );
});

function markEdited(): void {
  resetEl.hidden = !host.edited;
  copyEl.hidden = !host.edited || host.patch === undefined;
}

// Context menus: on a ruler label, hide the picked rows or show only them; on
// an external's head, open its module or pin it beside.
const menuEl = document.createElement("div");
menuEl.className = "menu";
menuEl.hidden = true;
document.body.append(menuEl);

function closeMenu(): void {
  menuEl.hidden = true;
}

interface MenuItem {
  label: string;
  disabled?: boolean;
  run: () => void;
}

function openMenu(x: number, y: number, items: MenuItem[]): void {
  menuEl.replaceChildren();
  for (const item of items) {
    const entry = button("", item.label, "");
    entry.disabled = item.disabled ?? false;
    entry.addEventListener("click", () => {
      closeMenu();
      item.run();
    });
    menuEl.append(entry);
  }
  menuEl.hidden = false;
  menuEl.style.left = `${Math.min(x, window.innerWidth - menuEl.offsetWidth - 8)}px`;
  menuEl.style.top = `${Math.min(y, window.innerHeight - menuEl.offsetHeight - 8)}px`;
}

function rowMenu(s: Sheet, label: HTMLElement, event: MouseEvent): void {
  const axis = label.dataset.axis;
  const row = label.dataset.row;
  if (axis === undefined || row === undefined) return;
  // Right-click outside the picked rows picks that row alone, as in a sheet.
  if (!(s.picked?.axis === axis && s.picked.labels.has(row))) {
    s.picked = { axis, labels: new Set([row]), anchor: row };
    applyPicked(s);
  }
  const picked = s.picked;
  if (!picked) return;
  const all = s.layout.bands.find((b) => b.axis === axis)?.labels ?? [];
  const hidden = hiddenOf(s.hidden, axis);
  const n = picked.labels.size;
  const rows = n === 1 ? "row" : `${n} rows`;
  openMenu(event.clientX, event.clientY, [
    {
      label: `Hide ${rows}`,
      run: () => {
        for (const l of picked.labels) hidden.add(l);
        s.picked = null;
        render();
      },
    },
    {
      label: `Show only ${rows}`,
      run: () => {
        for (const l of all) if (!picked.labels.has(l)) hidden.add(l);
        for (const l of picked.labels) hidden.delete(l);
        s.picked = null;
        render();
      },
    },
    {
      label: hidden.size > 0 ? `Show all ${all.length} rows` : "Show all rows",
      disabled: hidden.size === 0,
      run: () => {
        hidden.clear();
        render();
      },
    },
  ]);
}

/** `Open eps.gcl` goes there (the trail leads back); `Pin eps beside` opens it as a pane. */
function externalMenu(w: Workspace, node: ViewNode, event: MouseEvent): void {
  const key = { module: node.module, instance: node.instance };
  const file = fileNameOfModule(node.module);
  const pinned = w.sheets.find((other) => sameKey(other.key, key));
  openMenu(event.clientX, event.clientY, [
    {
      label: `Open ${file}`,
      disabled: !known(node.module),
      run: () => {
        if (known(node.module)) open([...w.trail, node.module]);
      },
    },
    pinned
      ? { label: `Unpin ${node.instance ?? file}`, run: () => unpin(pinned) }
      : { label: `Pin ${node.instance ?? file} beside`, run: () => pin(key) },
  ]);
}

panesEl.addEventListener("contextmenu", (event) => {
  if (!ws) return;
  const target = event.target as HTMLElement;
  const s = sheetOf(target);
  if (!s) return;
  const label = target.closest<HTMLElement>(".row-label");
  if (label) {
    event.preventDefault();
    rowMenu(s, label, event);
    return;
  }
  const head = target.closest<HTMLElement>(".col-head.kind-external");
  const node = head?.dataset.node ? s.model.nodes.find((n) => n.id === head.dataset.node) : null;
  if (!node) return;
  event.preventDefault();
  externalMenu(ws, node, event);
});

panesEl.addEventListener("dblclick", (event) => {
  if (!ws) return;
  const target = event.target as HTMLElement;
  const s = sheetOf(target);
  if (!s) return;
  const cell = target.closest<HTMLElement>(".cell[data-node], .box-value[data-node]");
  if (cell) {
    event.preventDefault();
    startCellEdit(s, cell);
    return;
  }
  const head = target.closest<HTMLElement>(".col:not(.folded) .col-head");
  const id = head?.dataset.node;
  if (!id) return;
  s.folded.add(id);
  select(s, null);
  render();
});

host.onModels(adoptModels);
host.onOpen((module) => open([module]));
host.onNote((message: string | null, kind: NoteKind) => showNote(message, kind));
if (host instanceof GitHubHost) mountGitHubBar(host, tokens, originEl, githubEl);
// The canvas changes width with the window, the explorer, the code pane and
// its split. Every such change asks for a relayout, which happens once per
// frame and only when the width really changed; a hidden canvas (code-only
// layout) is left as it is.
let lastWidth = canvasEl.clientWidth;
let relayoutQueued = false;
function scheduleRelayout(): void {
  if (relayoutQueued) return;
  relayoutQueued = true;
  // A timer, not an animation frame: frames stop in a background tab, and a
  // webview panel that is not in front still has to lay out for its width.
  setTimeout(() => {
    relayoutQueued = false;
    const width = canvasEl.clientWidth;
    if (width === 0 || width === lastWidth) return;
    lastWidth = width;
    relayout();
  });
}
new ResizeObserver(scheduleRelayout).observe(canvasEl);
window.addEventListener("resize", scheduleRelayout);
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
  // Pointer capture would retarget the click to the canvas, so boxes, ruler
  // labels and pane headers never start a pan.
  const target = event.target as HTMLElement;
  if (event.button !== 0 || target.closest(".col, .ruler-inner, .pane-head")) return;
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

// Zoom popover: a number field.
zoomEl.addEventListener("click", () => {
  zoomPopover.hidden = !zoomPopover.hidden;
  if (!zoomPopover.hidden) {
    const rect = zoomEl.getBoundingClientRect();
    zoomPopover.style.left = `${Math.min(rect.left, window.innerWidth - zoomPopover.offsetWidth - 8)}px`;
    zoomPopover.style.top = `${rect.bottom + 4}px`;
    zoomNumber.focus();
    zoomNumber.select();
  }
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
  if (!menuEl.hidden && !menuEl.contains(target)) closeMenu();
});
document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  zoomPopover.hidden = true;
  closeMenu();
  for (const s of ws?.sheets ?? []) {
    if (s.picked) {
      s.picked = null;
      applyPicked(s);
    }
  }
});

for (const modeButton of modeButtons) {
  modeButton.addEventListener("click", () => {
    if (!ws || !modeButton.dataset.mode) return;
    ws.mode = modeButton.dataset.mode as Mode;
    refit();
  });
}
new ResizeObserver(() => redraw()).observe(panesEl);

// ---- the web shell: arrangement, split, explorer, code pane ----
// Remembered per browser as conveniences; nothing here is shared or saved.

function recall(key: string): string | null {
  try {
    return localStorage.getItem(`graphcal.view.${key}`);
  } catch {
    return null;
  }
}
function remember(key: string, value: string): void {
  try {
    localStorage.setItem(`graphcal.view.${key}`, value);
  } catch {
    // Storage is blocked: the choice lasts for this page only.
  }
}

/** Which panes are shown: the code, the sheet, or both side by side. */
type Arrangement = "code" | "both" | "view";
const ARRANGEMENTS: readonly Arrangement[] = ["code", "both", "view"];
function isArrangement(value: string | null): value is Arrangement {
  return ARRANGEMENTS.some((a) => a === value);
}
let arrangement: Arrangement = "view";
const layoutButtons = [...layoutsEl.querySelectorAll<HTMLButtonElement>("[data-layout]")];

function applyArrangement(): void {
  codeEl.hidden = arrangement === "view";
  splitEl.hidden = arrangement !== "both";
  sheetEl.hidden = arrangement === "code";
  workEl.classList.toggle("code-only", arrangement === "code");
  for (const b of layoutButtons) b.classList.toggle("active", b.dataset.layout === arrangement);
  remember("arrangement", arrangement);
  if (arrangement === "code") editor?.focus();
  scheduleRelayout();
}

/** Drag the split: the code pane keeps the width it is dragged to. */
let splitting: { pointer: number } | null = null;
splitEl.addEventListener("pointerdown", (event) => {
  if (event.button !== 0) return;
  splitting = { pointer: event.pointerId };
  splitEl.setPointerCapture(event.pointerId);
  workEl.classList.add("splitting");
  event.preventDefault();
});
splitEl.addEventListener("pointermove", (event) => {
  if (!splitting) return;
  const left = codeEl.getBoundingClientRect().left;
  const width = Math.round(event.clientX - left);
  codeEl.style.width = `${width}px`;
  scheduleRelayout();
});
const endSplit = (): void => {
  if (!splitting) return;
  splitting = null;
  workEl.classList.remove("splitting");
  remember("code-width", String(codeEl.getBoundingClientRect().width));
};
splitEl.addEventListener("pointerup", endSplit);
splitEl.addEventListener("pointercancel", endSplit);

/** The main sheet's file, as the editor shows it, with the sheet's values at its lines. */
function inlaysOf(s: Sheet): Inlay[] {
  const file = s.model.module.file;
  const selection = s.selection;
  const inlays: Inlay[] = [];
  for (const node of s.model.nodes) {
    if (node.source === null || node.source.file !== file) continue;
    const picked = selection !== null && selection.id === node.id;
    let text: string;
    if (node.axes.length === 0) {
      text = `= ${displayOf(node.value)}`;
    } else {
      const axes = `[${node.axes.join(", ")}]`;
      const labels = picked
        ? [selection.row, selection.sub].filter((x): x is string => x !== null)
        : [];
      text =
        labels.length > 0
          ? `${axes} · ${labels.join(", ")} = ${displayOf(valueAt(node.value, labels))}`
          : axes;
    }
    inlays.push({ line: node.source.line, text, selected: picked });
  }
  return inlays;
}

/** The code pane shows the main sheet's file; a node picked in the sheet marks its line. */
function syncEditor(): void {
  if (!editor || !ws) return;
  const s = mainSheet(ws);
  const file = s.model.module.file;
  const text = host.source(s.model, file);
  codeFileEl.replaceChildren(span("", fileNameOf(s.model)));
  if (host.edited) codeFileEl.append(span("sub", "● what-if"));
  if (text === null) {
    codeFileEl.append(span("sub", "· source not available"));
    return;
  }
  editor.show(file, text);
  editor.setOriginal(host.compare ? (host.compare.source(file) ?? "") : null);
  editor.setInlays(inlaysOf(s));
  syncEditorSelection();
}

function syncEditorSelection(): void {
  if (!editor || !ws) return;
  const s = mainSheet(ws);
  editor.setInlays(inlaysOf(s));
  const hit = selected(s);
  const at = hit?.node.source ?? null;
  editor.markLine(at !== null && at.file === s.model.module.file ? at.line : null);
}

const editor = webShell
  ? new SourceEditor(codeBodyEl, {
      onChange(file, text) {
        if (!ws) return;
        const s = mainSheet(ws);
        if (host.source(s.model, file) === text) return;
        void applyEdit(s, { file, next: text });
      },
      onCaretLine(line) {
        if (!ws) return;
        const s = mainSheet(ws);
        const file = s.model.module.file;
        const node = s.model.nodes.find(
          (n) => n.source !== null && n.source.file === file && n.source.line === line,
        );
        if (!node || s.selection?.id === node.id) return;
        select(s, { id: node.id, row: null, sub: null, mode: "deps" });
      },
    })
  : null;

/** The explorer lists the project's files and, for a repository, its refs. */
const refSource: RefSource | null =
  host instanceof GitHubHost
    ? {
        current: null,
        list: () => host.refs(),
        choose: (ref) => host.switchRef(ref),
        edited: () => host.edited,
      }
    : null;
const explorer = webShell
  ? new Explorer(explorerEl, {
      title: route?.repo ?? "",
      subtitle: route ? `${route.owner}/${route.repo}` : "",
      refs: refSource,
      onOpen(file) {
        if (host.openFile?.(file)) return;
        const module = host.modules.find((m) => host.model(m)?.module.file === file);
        if (module !== undefined) open([module]);
      },
    })
  : null;

function renderExplorer(): void {
  if (!explorer) return;
  const files =
    host.files ??
    host.modules.map((m) => host.model(m)?.module.file).filter((f): f is string => f !== undefined);
  const main = ws ? mainSheet(ws).model : null;
  const selected = main ? (host.pathOf?.(main.module.id) ?? main.module.file) : null;
  explorer.render(files, selected, host.compare?.changedFiles);
}

if (webShell) {
  const storedWidth = Number(recall("code-width"));
  if (Number.isFinite(storedWidth) && storedWidth > 0) codeEl.style.width = `${storedWidth}px`;
  const stored = recall("arrangement");
  arrangement = isArrangement(stored) ? stored : "both";
  layoutsEl.hidden = false;
  crumbEl.hidden = false;
  for (const b of layoutButtons) {
    b.addEventListener("click", () => {
      if (!isArrangement(b.dataset.layout ?? null)) return;
      arrangement = b.dataset.layout as Arrangement;
      applyArrangement();
    });
  }
  const setExplorer = (open: boolean): void => {
    if (explorer) explorer.open = open;
    crumbEl.classList.toggle("open", open);
    remember("explorer", open ? "open" : "closed");
    scheduleRelayout();
  };
  crumbEl.addEventListener("click", () => setExplorer(!(explorer?.open ?? false)));
  setExplorer(recall("explorer") === "open");
  applyArrangement();
  if (host instanceof GitHubHost && refSource) {
    host.onState((state) => {
      refSource.current = state.ref;
      explorer?.renderRef();
    });
  }
  document.addEventListener("keydown", (event) => {
    // ⌘/Ctrl-Enter: evaluate what is typed now instead of waiting for the pause.
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) editor?.flush();
  });
}

if (!route && !inVsCode()) {
  // No address: the sheet has nothing to show, so the page asks for a file.
  sheetEl.hidden = true;
  fileEl.textContent = "";
  mountStart(startEl, location.hash.length > 1 ? location.hash : null);
}
