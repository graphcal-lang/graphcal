// A module shown as a sheet, and the workspace of sheets on the canvas: the
// main one and the panes pinned beside it. A sheet keeps its model, its
// layout and what the reader did to it (folds, hidden rows, picked rows, the
// selection); this module creates the sheets' elements, places them in pane
// columns and fits a sheet's no-axis boxes to a width.
import { button, span, SVG_NS } from "./dom.ts";
import type { Mode } from "./camera.ts";
import {
  BOX_GAP,
  computeLayout,
  entryName,
  orderByBarycenter,
  packScalarBand,
  ZONES,
  zoneOf,
  type BoxSize,
  type Layout,
  type Zone,
} from "./layout.ts";
import type { ViewModel, ViewNode } from "./model.ts";
import { depthsOf, type PaneKey } from "./panes.ts";
import { EXTERNAL_MARK, renderView, type RenderOptions } from "./render.ts";
import type { Hidden, Picked } from "./rows.ts";
import type { Selection } from "./selection.ts";
import { planRoutes, type RoutePlan } from "./wires.ts";

/** One module shown as a sheet: the main one, or a pane pinned beside it. */
export interface Sheet {
  key: PaneKey;
  model: ViewModel;
  /** Layout before the no-axis band is packed to the width. */
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

export interface Workspace {
  /** The main sheet first, then the pinned panes in the order they were pinned. */
  sheets: Sheet[];
  /** Modules opened in turn with "Open"; the last is the main sheet's. */
  trail: string[];
  mode: Mode;
}

export function mainSheet(w: Workspace): Sheet {
  return w.sheets[0];
}

/** The sheet holding the selection, else the main one. */
export function activeSheet(w: Workspace): Sheet {
  return w.sheets.find((s) => s.selection !== null) ?? mainSheet(w);
}

/** `power_max.gcl`, from the model's file. */
export function fileNameOf(model: ViewModel): string {
  return model.module.file.split("/").at(-1) ?? model.module.file;
}

/** The layout a model starts from, before it is fitted to a width. */
export function baseLayoutOf(model: ViewModel): Layout {
  return orderByBarycenter(computeLayout(model), model);
}

export function createSheet(key: PaneKey, model: ViewModel): Sheet {
  const base = baseLayoutOf(model);
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

/** Take a fresh model of the sheet's module; the reader's folds and selection stay. */
export function adoptModel(sheet: Sheet, model: ViewModel): void {
  sheet.model = model;
  sheet.base = baseLayoutOf(model);
}

export function renderOptions(s: Sheet): RenderOptions {
  return { folded: s.folded, hidden: s.hidden };
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
export function placePanes(panesEl: HTMLElement, w: Workspace): void {
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

/** The selected node and the entry keys of the selected cell, one per axis. */
export function selected(s: Sheet): { node: ViewNode; keys: string[] } | null {
  const { selection, model } = s;
  if (!selection) return null;
  const node = model.nodes.find((n) => n.id === selection.id);
  if (!node) return null;
  const keys = [selection.row, selection.sub].filter((x): x is string => x !== null);
  return { node, keys };
}

/** What a reader sees for the entry keys of a cell of `node`, one per axis. */
export function entryNames(s: Sheet, node: ViewNode, keys: readonly string[]): string[] {
  return keys.map((key, i) => entryName(s.layout.axes, node.axes[i] ?? "", key));
}

/**
 * Render once to measure boxes and axis-band zones, then pack the no-axis
 * boxes into stacks so everything fits the width. Width left over after a
 * first packing is handed to the zones that had to stack, and they repack.
 */
export function fit(s: Sheet, available: number): Layout {
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
