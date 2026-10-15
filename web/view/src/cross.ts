// Wires between sheets. A value read from another pane leaves that pane at the
// right end of its box's head, runs down the gutter between the pane columns,
// enters the consumer's channel (below its own lanes) and rises into the port
// box.
import type { Layout } from "./layout.ts";
import type { ViewModel } from "./model.ts";
import { keyOf, sameKey, type PaneKey } from "./panes.ts";
import {
  anchorsOf,
  CHANNEL_PAD,
  LANE_GAP,
  marker,
  roundedPath,
  scalarTierCount,
  scaleOf,
} from "./wires.ts";

export interface CrossSheet {
  key: PaneKey;
  model: ViewModel;
  layout: Layout;
  viewEl: HTMLElement;
  paneEl: HTMLElement;
}

export interface CrossLink {
  from: CrossSheet;
  /** The producer's box. */
  fromSelector: string;
  to: CrossSheet;
  /** The consumer's port: the external's column. */
  toSelector: string;
  toSide: "top" | "bottom";
  /** The consumer channel the wire runs through. */
  channel: number;
  fromId: string;
  toId: string;
}

/** Every external value whose producer is open. */
export function crossLinks(sheets: CrossSheet[]): CrossLink[] {
  const links: CrossLink[] = [];
  for (const to of sheets) {
    const tiers = scalarTierCount(to.layout);
    for (const node of to.model.nodes) {
      // A pane shows its module evaluated alone, which a bound include's values are not.
      if (node.kind !== "external" || node.instance?.bound === true) continue;
      const key = keyOf(node);
      const from = sheets.find((s) => sameKey(s.key, key));
      const column = to.layout.columns.get(node.id);
      if (!from || !column || !from.layout.columns.has(node.name)) continue;
      const onTop = to.layout.bands[column.bandIndex]?.axis !== null;
      links.push({
        from,
        fromSelector: `.col[data-node="${node.name}"]`,
        to,
        toSelector: `.col[data-node="${node.id}"]`,
        toSide: onTop ? "top" : "bottom",
        channel: onTop ? column.bandIndex : tiers,
        fromId: node.name,
        toId: node.id,
      });
    }
  }
  return links;
}

/** Lanes the cross wires need in each channel of `sheet`, beyond its own. */
export function extraLanes(links: CrossLink[], sheet: CrossSheet): Map<number, number> {
  const lanes = new Map<number, number>();
  for (const link of links) {
    if (link.to === sheet) lanes.set(link.channel, (lanes.get(link.channel) ?? 0) + 1);
  }
  return lanes;
}

const SVG_NS = "http://www.w3.org/2000/svg";
const GUTTER_PAD = 10;

/**
 * Draw the cross wires on the stage-level svg. `ownLanes` is what each sheet's
 * own wires took per channel (returned by drawWires); cross wires take the
 * lanes below them.
 */
export function drawCrossWires(
  svg: SVGSVGElement,
  stage: HTMLElement,
  links: CrossLink[],
  ownLanes: ReadonlyMap<CrossSheet, ReadonlyMap<number, number>>,
): void {
  svg.replaceChildren();
  svg.setAttribute("width", String(stage.scrollWidth));
  svg.setAttribute("height", String(stage.scrollHeight));
  const defs = document.createElementNS(SVG_NS, "defs");
  defs.append(marker("cross-arrow", "arrow-idle"));
  svg.append(defs);
  const origin = stage.getBoundingClientRect();
  const k = scaleOf(stage, origin);
  const laneUsed = new Map<CrossSheet, Map<number, number>>();
  const gutterUsed = new Map<CrossSheet, number>();
  for (const link of links) {
    const a = anchorsOf(link.from.viewEl, link.fromSelector, origin, k);
    const b = anchorsOf(link.to.viewEl, link.toSelector, origin, k);
    const channel = link.to.viewEl.querySelector<HTMLElement>(
      `.channel[data-channel="${link.channel}"]`,
    );
    if (!a || !b || !channel) continue;
    const used = laneUsed.get(link.to) ?? new Map<number, number>();
    const lane = (ownLanes.get(link.to)?.get(link.channel) ?? 0) + (used.get(link.channel) ?? 0);
    used.set(link.channel, (used.get(link.channel) ?? 0) + 1);
    laneUsed.set(link.to, used);
    const y =
      (channel.getBoundingClientRect().top - origin.top) / k + CHANNEL_PAD + lane * LANE_GAP;
    const gutter = gutterUsed.get(link.from) ?? 0;
    gutterUsed.set(link.from, gutter + 1);
    const paneRight = (link.from.paneEl.getBoundingClientRect().right - origin.left) / k;
    const gx = paneRight + GUTTER_PAD + gutter * LANE_GAP;
    const start = a.headRight;
    const end = b[link.toSide];
    const points = [start, { x: gx, y: start.y }, { x: gx, y }, { x: end.x, y }, end];
    const path = document.createElementNS(SVG_NS, "path");
    path.setAttribute("d", roundedPath(points, 6));
    path.setAttribute("class", "wire cross");
    path.setAttribute("marker-end", "url(#cross-arrow)");
    path.dataset.from = link.fromId;
    path.dataset.to = link.toId;
    const dot = document.createElementNS(SVG_NS, "circle");
    dot.setAttribute("cx", String(start.x));
    dot.setAttribute("cy", String(start.y));
    dot.setAttribute("r", "2.5");
    dot.setAttribute("class", "wire-dot");
    svg.append(path, dot);
  }
}
