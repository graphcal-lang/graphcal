// The view page: the host it runs in, the workspace of sheets on the canvas,
// and the events that change them. The pieces live in their own modules
// (camera, sheet, highlight, formula bar, editing, menu, shell); this file
// wires them together.
import { Camera, centredOf, isMode, overviewOf } from "./camera.ts";
import { changeNote, diffModels, summarize, type ModelDiff } from "./compare.ts";
import { markChanges } from "./compareMarks.ts";
import { crossLinks, drawCrossWires, extraLanes, type CrossSheet } from "./cross.ts";
import { button, need, span } from "./dom.ts";
import { cellRange, editableExpr, planCellEdit, planExprEdit, type EditPlan } from "./editing.ts";
import { SourceEditor } from "./editor.ts";
import { errorMessage } from "./errors.ts";
import { Explorer, type RefSource } from "./explorer.ts";
import { FormulaBar } from "./formulaBar.ts";
import { connectExtension } from "./extension.ts";
import { mountGitHubBar } from "./githubBar.ts";
import { GitHubHost } from "./githubHost.ts";
import { applyPicked, applySelection } from "./highlight.ts";
import type { Host, NoteKind } from "./host.ts";
import type { Revised } from "./revisions.ts";
import { bindInput } from "./inlineInput.ts";
import { inlaysOf } from "./inlays.ts";
import { Menu } from "./menu.ts";
import { instanceOf, isExternal, type ViewModel, type ViewNode } from "./model.ts";
import { gapKeys, placeHalo, renderView, RULER_WIDTH } from "./render.ts";
import { parseRoute } from "./route.ts";
import { hiddenOf, pick } from "./rows.ts";
import type { Selection } from "./selection.ts";
import {
  activeSheet,
  adoptModel,
  createSheet,
  entryNames,
  fileNameOf,
  fit,
  mainSheet,
  placePanes,
  renderOptions,
  selected,
  type Sheet,
  type Workspace,
} from "./sheet.ts";
import { mountShell } from "./shell.ts";
import { moduleTarget, readTarget, sameTarget, targetOf, type ViewTarget } from "./target.ts";
import { EmptyHost, mountStart } from "./start.ts";
import { displayOf, oneLine, valueAt } from "./values.ts";
import { inVsCode, VsCodeHost } from "./vscodeHost.ts";
import { drawWires, planRoutes } from "./wires.ts";

/**
 * Where models come from and edits go: the extension in VS Code; a GitHub
 * repository when the address names one (`#owner/repo@ref/path.gcl`); with
 * no address the page only asks for one.
 */
const route = inVsCode() ? null : parseRoute(location.hash);
const host: Host = inVsCode()
  ? new VsCodeHost()
  : route
    ? new GitHubHost(route, connectExtension())
    : new EmptyHost();
/** The web shell has the explorer, the code pane and the layout toggle; VS Code brings its own. */
const webShell = !inVsCode() && route !== null;

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
const codeBodyEl = need<HTMLElement>("#code-body");
const splitEl = need<HTMLElement>("#split");
const sheetEl = need<HTMLElement>("#sheet");
const sheetNameEl = need<HTMLElement>("#sheet-name");
const layoutsEl = need<HTMLElement>("#layouts");
const startEl = need<HTMLElement>("#start");
const canvasEl = need<HTMLElement>("#canvas");

const camera = new Camera({
  canvas: canvasEl,
  stage: need<HTMLElement>("#stage"),
  zoom: need<HTMLButtonElement>("#zoom"),
  popover: need<HTMLElement>("#zoom-popover"),
  number: need<HTMLInputElement>("#zoom-number"),
  modes: [...document.querySelectorAll<HTMLButtonElement>("[data-mode]")],
});
const formula = new FormulaBar(need<HTMLElement>("#formula"), need<HTMLElement>("#code-note"));
const menu = new Menu();

let ws: Workspace | null = null;

/** Whether the host has a model for `module` alone. */
function known(module: string): boolean {
  return host.model(moduleTarget(module)) !== null;
}

/** `power_max.gcl` for a module id, from its model's file. */
function fileNameOfModule(module: string): string {
  const model = host.model(moduleTarget(module));
  return model ? fileNameOf(model) : `${module.split(".").at(-1) ?? module}.gcl`;
}

function sheetOf(target: Element): Sheet | null {
  const pane = target.closest(".pane");
  return ws?.sheets.find((s) => s.paneEl === pane) ?? null;
}

/** What changed in a sheet since the base ref, when the address compares two refs and the base is read. */
function diffOf(s: Sheet): ModelDiff | null {
  const compare = host.compare ?? null;
  if (!compare) return null;
  const base = compare.model(s.model);
  switch (base.kind) {
    case "absent":
      return diffModels(null, s.model);
    case "present":
      return diffModels(base.value, s.model);
    case "failed":
      // Nothing to compare against: no node is marked rather than every one.
      return null;
  }
}

/**
 * The revision the sheet's model was projected from, when the host holds the
 * file `node` is declared in: an edit of the node is a range on that revision.
 */
function editAt(s: Sheet, node: ViewNode): number | null {
  const file = node.source?.file ?? null;
  if (file === null || host.source(s.model, file) === null) return null;
  return host.projectedAt(s.model);
}

/** `subject = expr = value` for the selected cell of the active sheet. */
function renderFormula(): void {
  if (!ws) return;
  const s = activeSheet(ws);
  const hit = selected(s);
  if (!hit) {
    formula.render(null);
    return;
  }
  const { node, keys } = hit;
  const change = diffOf(s)?.nodes.get(node.id);
  formula.render({
    label: node.label,
    entries: entryNames(s, node, keys),
    expr: node.expr === null ? null : oneLine(node.expr),
    editable: editableExpr(node, editAt(s, node)) !== null,
    value: displayOf(valueAt(node.value, keys)),
    change: change ? changeNote(change, node.value, keys) : null,
  });
}

function showNote(message: string | null, kind: NoteKind = "error"): void {
  formula.note(message, kind);
}

function highlight(s: Sheet): void {
  applySelection(s, diffOf(s));
  renderFormula();
}

/** Draw every sheet's wires, then the wires between sheets, and re-apply highlights. */
function redraw(): void {
  if (!ws) return;
  const links = crossLinks(ws.sheets);
  const ownLanes = new Map<CrossSheet, Map<number, number>>();
  for (const s of ws.sheets) {
    ownLanes.set(s, drawWires(s.wiresEl, s.viewEl, s.layout, s.plan, extraLanes(links, s)));
    placeHalo(s.viewEl);
    highlight(s);
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
 * Set when a layout was asked for while the canvas was hidden (the code-only
 * layout): nothing can be measured then, so it happens once the canvas shows.
 */
let layoutPending = false;

/** Whether the sheets can be measured now; if not, they are laid out once the canvas shows. */
function measurable(): boolean {
  if (canvasEl.clientWidth > 0) return true;
  layoutPending = true;
  return false;
}

/** Lay out and draw every sheet; the camera stays where it is (measuring needs an unscaled stage). */
function render(): void {
  if (!ws || !measurable()) return;
  camera.unscale();
  for (const s of ws.sheets) {
    s.layout = fit(s, s === mainSheet(ws) ? availableWidth() : PANE_WIDTH);
    s.plan = planRoutes(s.model, s.layout);
    renderView(s.viewEl, s.layout, renderOptions(s));
    const diff = diffOf(s);
    if (diff) markChanges(s.viewEl, s.model, diff);
  }
  camera.apply();
  redraw();
}

function canvasSize(): { width: number; height: number } {
  return { width: canvasEl.clientWidth, height: canvasEl.clientHeight };
}

function stageSize(): { width: number; height: number } {
  return { width: panesEl.scrollWidth, height: panesEl.scrollHeight };
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

/** Re-apply the mode from scratch: everything open, then scaled down in overview. */
function refit(): void {
  if (!ws || !measurable()) return;
  for (const s of ws.sheets) s.folded = new Set();
  camera.reset();
  render();
  if (ws.mode === "overview") {
    camera.set(overviewOf(canvasSize(), stageSize()));
  } else {
    foldToFit(mainSheet(ws));
    camera.set(centredOf(canvasSize(), stageSize()));
  }
  camera.showMode(ws.mode);
}

/**
 * The canvas changed size: the mode is applied again for the new width,
 * unless the zoom was set by hand, in which case the sheets are just laid
 * out again with their folds kept.
 */
function relayout(): void {
  if (!ws) return;
  if (camera.manual) render();
  else refit();
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

/** A trail to open once the host has its module's current model (an edit left it stale). */
let pendingOpen: string[] | null = null;

/** Show `trail`'s last module as the main sheet, with no panes pinned. */
function open(trail: string[]): void {
  const module = trail.at(-1);
  if (module === undefined) return;
  const target = moduleTarget(module);
  const model = host.model(target);
  if (!model) {
    if (host.modules.includes(module)) {
      pendingOpen = trail;
      host.request(target);
    }
    return;
  }
  pendingOpen = null;
  // Panes asked for on the last sheet are not this one's.
  pendingPins.length = 0;
  ws = { sheets: [createSheet(target, model)], trail, mode: ws?.mode ?? "fit" };
  placePanes(panesEl, ws);
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
const pendingPins: ViewTarget[] = [];

/** Open a sheet for `key` beside its readers; a pane already open just stays. */
function pin(key: ViewTarget): void {
  if (!ws || ws.sheets.some((s) => sameTarget(s.key, key))) return;
  const model = host.model(key);
  if (!model) {
    if (!pendingPins.some((k) => sameTarget(k, key))) pendingPins.push(key);
    host.request(key);
    return;
  }
  ws.sheets.push(createSheet(key, model));
  placePanes(panesEl, ws);
  render();
}

/** Take fresh models from the host: every sheet showing one of them is re-laid out in place. */
function adoptModels(models: ViewModel[]): void {
  const opening = pendingOpen;
  if (opening !== null && known(opening.at(-1) ?? "")) open(opening);
  if (!ws) return;
  let changed = false;
  for (const model of models) {
    for (const sheet of ws.sheets) {
      if (!sameTarget(sheet.key, targetOf(model))) continue;
      adoptModel(sheet, model);
      changed = true;
    }
  }
  for (const key of pendingPins.splice(0)) {
    if (host.model(key) !== null) pin(key);
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
  placePanes(panesEl, ws);
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
  for (const key of gapKeys(gap)) set.delete(key);
  render();
}

/** One selection at a time across sheets. */
function select(s: Sheet, selection: Selection | null): void {
  if (!ws) return;
  for (const other of ws.sheets) {
    if (other !== s && other.selection !== null) {
      other.selection = null;
      highlight(other);
    }
  }
  s.selection = selection;
  highlight(s);
  syncEditorSelection();
}

panesEl.addEventListener("click", (event) => {
  if (!ws) return;
  const target = event.target as HTMLElement;
  // A click in a cell being edited places the caret; it does not select again.
  if (target.closest("input")) return;
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
// GitHub host rewrites its in-memory copy and re-projects the open sheets
// in Wasm; VS Code edits the document and the language server's fresh model
// arrives later. An edit the host refuses shows its message; the input
// stays open with the text.

let evaluating: Promise<void> = Promise.resolve();

/**
 * Hand an edit to the host, one at a time, and take back whatever models it
 * re-projected. `applied` runs once the host took the edit, before the models.
 */
function applyEdit(
  s: Sheet,
  plan: EditPlan,
  applied?: (source: Revised) => void,
): Promise<boolean> {
  if (!plan.ok) {
    showNote(plan.message);
    return Promise.resolve(false);
  }
  const { edit } = plan;
  const current = ws;
  if (!current) return Promise.resolve(false);
  const run = async (): Promise<boolean> => {
    const evaluated = formula.busy("evaluating…");
    const open = current.sheets.map((sheet) => sheet.key);
    let result;
    try {
      result = await host.edit(s.model, edit, open);
    } catch (error) {
      showNote(errorMessage(error));
      return false;
    }
    // The code pane's text is based on the host's, whatever the workspace shows now.
    if (result.ok) applied?.(result.source);
    if (ws !== current) {
      // The workspace changed meanwhile: its result is not shown, nor its progress.
      evaluated();
      return result.ok;
    }
    if (!result.ok) {
      showNote(result.message);
      return false;
    }
    showNote(result.note);
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

/** Edit the selected node's expression in place in the formula bar. */
function startFormulaEdit(): void {
  if (!ws || formula.editing) return;
  const s = activeSheet(ws);
  const hit = selected(s);
  if (!hit) return;
  const { node } = hit;
  const expr = editableExpr(node, editAt(s, node));
  if (expr === null) return;
  formula.startEdit(
    expr,
    node.label,
    (text) => applyEdit(s, planExprEdit(node, editAt(s, node), text)),
    () => renderFormula(),
  );
}

/** Edit a param cell or scalar box in place; other nodes edit through the formula bar. */
function startCellEdit(s: Sheet, cell: HTMLElement): void {
  const id = cell.dataset.node;
  const node = s.model.nodes.find((n) => n.id === id);
  if (!node) return;
  const row = cell.dataset.row ?? null;
  const sub = cell.dataset.sub ?? null;
  select(s, { id: node.id, row, sub, mode: "deps" });
  // A value read from another file is edited in that file, not here.
  // A cell the compiler reports no range for is read-only; the bar shows its expression.
  const keys = [row, sub].filter((x): x is string => x !== null);
  const literal = !isExternal(node) && (node.kind === "param" || node.kind === "const");
  if (!literal || cellRange(node, keys) === null || editAt(s, node) === null) {
    startFormulaEdit();
    return;
  }
  const value = valueAt(node.value, keys);
  const initial = value?.kind === "quantity" ? value.magnitude : displayOf(value);
  const input = document.createElement("input");
  input.className = "cell-input";
  // The default `size` (20 characters) would widen the column to fit the input.
  input.size = 1;
  input.spellcheck = false;
  input.value = initial;
  input.setAttribute("aria-label", `value of ${node.label}`);
  const before = [...cell.childNodes];
  cell.replaceChildren(input);
  cell.classList.add("editing");
  bindInput(
    input,
    initial,
    (text) => applyEdit(s, planCellEdit(node, keys, editAt(s, node), text)),
    () => {
      cell.classList.remove("editing");
      cell.replaceChildren(...before);
    },
  );
  showNote(null);
  input.focus();
  input.select();
}

formula.onClick(startFormulaEdit);

const resetEl = button("bar-reset", "reset", "discard the what-if edits and reload");
resetEl.hidden = true;
resetEl.addEventListener("click", () => {
  // After the edit in progress, which would otherwise land on the reset sources.
  void evaluating.then(() => {
    host.reset();
    if (ws) open(ws.trail);
  });
});

/** The edits as a patch on the clipboard: `git apply` or the VS Code extension puts them in the files. */
const copyEl = button(
  "bar-copy",
  "copy changes",
  "copy the edits as a patch; in VS Code run “Graphcal: Apply Changes from Clipboard”",
);
copyEl.hidden = true;
copyEl.addEventListener("click", () => {
  const patch = host.patch?.() ?? { text: "", files: 0 };
  if (patch.files === 0) {
    showNote("nothing changed");
    return;
  }
  const { files } = patch;
  navigator.clipboard.writeText(patch.text).then(
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
  menu.open(event.clientX, event.clientY, [
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

/**
 * `Open eps.gcl` goes to the module alone (the trail leads back); `Pin eps
 * beside` opens what the sheet `s` reads the node from as a pane: the include
 * instance with its arguments, or the imported module.
 */
function externalMenu(w: Workspace, s: Sheet, node: ViewNode, event: MouseEvent): void {
  const key = readTarget(s.key, node);
  const file = fileNameOfModule(node.module);
  const instance = instanceOf(node);
  const name = instance?.name ?? file;
  const pinned = key === null ? undefined : w.sheets.find((other) => sameTarget(other.key, key));
  menu.open(event.clientX, event.clientY, [
    {
      label: `Open ${file}`,
      disabled: !known(node.module) && !host.modules.includes(node.module),
      run: () => open([...w.trail, node.module]),
    },
    pinned
      ? { label: `Unpin ${name}`, run: () => unpin(pinned) }
      : key === null
        ? {
            // A call is evaluated in place, as part of its caller; an
            // includer's declaration is on the includer's own sheet.
            label:
              node.origin.kind === "called"
                ? `Pin ${name} beside: a call has no sheet of its own`
                : `Pin ${name} beside: it is on the including sheet`,
            disabled: true,
            run: () => undefined,
          }
        : { label: `Pin ${name} beside`, run: () => pin(key) },
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
  externalMenu(ws, s, node, event);
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
if (host instanceof GitHubHost) mountGitHubBar(host, originEl, githubEl);
// The canvas changes width with the window, the explorer, the code pane and
// its split. Every such change asks for a relayout, which happens once per
// frame and only when the width really changed or a layout waits for the
// canvas to show; a hidden canvas (code-only layout) is left as it is.
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
    if (width === 0 || (width === lastWidth && !layoutPending)) return;
    lastWidth = width;
    layoutPending = false;
    relayout();
  });
}
new ResizeObserver(scheduleRelayout).observe(canvasEl);
window.addEventListener("resize", scheduleRelayout);

/** Nothing selected and no rows picked, in any sheet. */
function clearSelection(): void {
  if (!ws) return;
  for (const s of ws.sheets) {
    if (s.picked) {
      s.picked = null;
      applyPicked(s);
    }
  }
  select(activeSheet(ws), null);
}

camera.onEmptyClick(clearSelection);

document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  camera.closePopover();
  menu.close();
  // Escape in a text field (the code pane, the zoom percent) is that field's.
  const target = event.target as HTMLElement | null;
  if (target?.closest("input, textarea, [contenteditable]")) return;
  clearSelection();
});

for (const modeButton of document.querySelectorAll<HTMLButtonElement>("[data-mode]")) {
  modeButton.addEventListener("click", () => {
    const mode = modeButton.dataset.mode;
    if (!ws || !isMode(mode)) return;
    ws.mode = mode;
    refit();
  });
}
new ResizeObserver(() => redraw()).observe(panesEl);

// ---- the web shell: arrangement, split, explorer, code pane ----

/** The code pane shows the main sheet's file; a node picked in the sheet marks its line. */
function syncEditor(): void {
  if (!editor || !ws) return;
  const s = mainSheet(ws);
  const file = s.model.module.file;
  const source = host.source(s.model, file);
  codeFileEl.replaceChildren(span("", fileNameOf(s.model)));
  if (host.edited) codeFileEl.append(span("sub", "● what-if"));
  if (source === null) {
    codeFileEl.append(span("sub", "· source not available"));
    return;
  }
  editor.show(file, source);
  const original = host.compare?.source(file);
  // A new file is all added; a base that could not be read marks nothing.
  editor.setOriginal(
    original?.kind === "present" ? original.value : original?.kind === "absent" ? "" : null,
  );
  syncEditorSelection();
}

function syncEditorSelection(): void {
  if (!editor || !ws) return;
  const s = mainSheet(ws);
  editor.setInlays(inlaysOf(s.model, s.layout.axes, s.selection));
  const hit = selected(s);
  const at = hit?.node.source ?? null;
  editor.markLine(at !== null && at.file === s.model.module.file ? at.line : null);
}

const editor = webShell
  ? new SourceEditor(codeBodyEl, {
      onChange(edit, applied) {
        if (!ws) return;
        void applyEdit(mainSheet(ws), { ok: true, edit }, applied);
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
        const module = host.modules.find((m) => host.model(moduleTarget(m))?.module.file === file);
        if (module !== undefined) open([module]);
      },
    })
  : null;

function renderExplorer(): void {
  if (!explorer) return;
  const files =
    host.files ??
    host.modules
      .map((m) => host.model(moduleTarget(m))?.module.file)
      .filter((f): f is string => f !== undefined);
  const main = ws ? mainSheet(ws).model : null;
  const shown = main ? (host.pathOf?.(main.module.id) ?? main.module.file) : null;
  explorer.render(files, shown, host.compare?.changedFiles);
}

if (webShell) {
  mountShell(
    {
      work: workEl,
      code: codeEl,
      split: splitEl,
      sheet: sheetEl,
      layouts: layoutsEl,
      crumb: crumbEl,
    },
    {
      relayout: scheduleRelayout,
      focusCode: () => editor?.focus(),
      setExplorer: (open) => {
        if (explorer) explorer.open = open;
      },
    },
  );
  if (host instanceof GitHubHost && refSource) {
    host.onState((state) => {
      refSource.current = state.ref;
      explorer?.setTitle(state.route.repo, `${state.route.owner}/${state.route.repo}`);
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
