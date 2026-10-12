import type { Layout } from "./layout.ts";
import type { ViewModel } from "./model.ts";

// Wires never cross rows. They run through the channels between bands (and the
// right gutter when two bands are not adjacent); a wire between neighbouring
// columns of one band is a short arrow at header height.

export interface Edge {
  from: string;
  to: string;
}

interface Ends {
  fromSide: "top" | "bottom";
  toSide: "top" | "bottom";
}

export type Route =
  | { kind: "adjacent"; edge: Edge }
  | ({ kind: "channel"; edge: Edge; channel: number; lane: number } & Ends)
  | ({
      kind: "gutter";
      edge: Edge;
      channel: number;
      lane: number;
      channel2: number;
      lane2: number;
      gutterLane: number;
    } & Ends);

export interface RoutePlan {
  routes: Route[];
  /** Number of lanes used in each channel (index = channel). */
  lanes: number[];
}

export const LANE_GAP = 6;
export const CHANNEL_PAD = 10;

export function edgesOf(model: ViewModel, layout: Layout): Edge[] {
  const edges: Edge[] = [];
  for (const node of model.nodes) {
    for (const dep of node.deps) {
      if (layout.columns.has(dep)) edges.push({ from: dep, to: node.id });
    }
  }
  return edges;
}

type Side = "top" | "bottom";

/**
 * Which side of a column a wire attaches to. Axis bands attach at the header
 * (top). No-axis boxes attach on the side facing the other end, so neighbouring
 * tiers share the channel between them.
 */
function sideOf(layout: Layout, bandIndex: number, otherBandIndex: number): Side {
  if (layout.bands[bandIndex]?.axis !== null) return "top";
  return otherBandIndex < bandIndex ? "top" : "bottom";
}

function channelOf(bandIndex: number, side: Side): number {
  return side === "top" ? bandIndex : bandIndex + 1;
}

export function planRoutes(model: ViewModel, layout: Layout): RoutePlan {
  const lanes: number[] = Array.from({ length: layout.bands.length + 1 }, () => 0);
  const pending: { edge: Edge; a: number; b: number; order: number; ends: Ends }[] = [];
  const routes: Route[] = [];
  for (const edge of edgesOf(model, layout)) {
    const from = layout.columns.get(edge.from);
    const to = layout.columns.get(edge.to);
    if (!from || !to) continue;
    if (from.bandIndex === to.bandIndex && Math.abs(from.position - to.position) === 1) {
      routes.push({ kind: "adjacent", edge });
      continue;
    }
    const fromSide = sideOf(layout, from.bandIndex, to.bandIndex);
    const toSide = sideOf(layout, to.bandIndex, from.bandIndex);
    const a = channelOf(from.bandIndex, fromSide);
    const b = channelOf(to.bandIndex, toSide);
    pending.push({
      edge,
      a,
      b,
      order: from.bandIndex * 1000 + from.position,
      ends: { fromSide, toSide },
    });
  }
  // Lanes are handed out in source order so wires from neighbouring columns
  // stay close together; longer wires take the lanes farther from the band.
  pending.sort((x, y) => x.order - y.order);
  let gutterLane = 0;
  for (const { edge, a, b, ends } of pending) {
    if (a === b) {
      routes.push({ kind: "channel", edge, channel: a, lane: lanes[a]++, ...ends });
    } else {
      routes.push({
        kind: "gutter",
        edge,
        channel: a,
        lane: lanes[a]++,
        channel2: b,
        lane2: lanes[b]++,
        gutterLane: gutterLane++,
        ...ends,
      });
    }
  }
  return { routes, lanes };
}

export function channelHeight(laneCount: number): number {
  return laneCount === 0 ? 14 : CHANNEL_PAD * 2 + (laneCount - 1) * LANE_GAP;
}

const SVG_NS = "http://www.w3.org/2000/svg";

interface Port {
  x: number;
  y: number;
}

interface Anchors {
  top: Port;
  bottom: Port;
  headLeft: Port;
  headRight: Port;
}

function anchorsOf(view: HTMLElement, id: string, origin: DOMRect): Anchors | null {
  const col = view.querySelector<HTMLElement>(`.col[data-node="${id}"]`);
  const head = col?.querySelector<HTMLElement>(".col-head");
  if (!col || !head) return null;
  const c = col.getBoundingClientRect();
  const h = head.getBoundingClientRect();
  const x = c.left + c.width / 2 - origin.left;
  return {
    top: { x, y: h.top - origin.top },
    bottom: { x, y: c.bottom - origin.top },
    headLeft: { x: h.left - origin.left, y: h.top + h.height / 2 - origin.top },
    headRight: { x: h.right - origin.left, y: h.top + h.height / 2 - origin.top },
  };
}

/** Orthogonal polyline with rounded corners. */
function roundedPath(points: Port[], radius: number): string {
  const pts = points.filter(
    (p, i) => i === 0 || p.x !== points[i - 1].x || p.y !== points[i - 1].y,
  );
  if (pts.length === 0) return "";
  let d = `M${pts[0].x},${pts[0].y}`;
  for (let i = 1; i < pts.length - 1; i++) {
    const prev = pts[i - 1];
    const p = pts[i];
    const next = pts[i + 1];
    const d1 = Math.hypot(p.x - prev.x, p.y - prev.y);
    const d2 = Math.hypot(next.x - p.x, next.y - p.y);
    const r = Math.min(radius, d1 / 2, d2 / 2);
    const a = { x: p.x - ((p.x - prev.x) / d1) * r, y: p.y - ((p.y - prev.y) / d1) * r };
    const b = { x: p.x + ((next.x - p.x) / d2) * r, y: p.y + ((next.y - p.y) / d2) * r };
    d += ` L${a.x},${a.y} Q${p.x},${p.y} ${b.x},${b.y}`;
  }
  const last = pts[pts.length - 1];
  d += ` L${last.x},${last.y}`;
  return d;
}

function laneY(view: HTMLElement, channel: number, lane: number, origin: DOMRect): number {
  const el = view.querySelector<HTMLElement>(`.channel[data-channel="${channel}"]`);
  if (!el) return 0;
  const r = el.getBoundingClientRect();
  return r.top - origin.top + CHANNEL_PAD + lane * LANE_GAP;
}

function marker(id: string, cls: string): SVGMarkerElement {
  const m = document.createElementNS(SVG_NS, "marker");
  m.setAttribute("id", id);
  m.setAttribute("viewBox", "0 0 10 10");
  m.setAttribute("refX", "8");
  m.setAttribute("refY", "5");
  m.setAttribute("markerWidth", "6");
  m.setAttribute("markerHeight", "6");
  m.setAttribute("orient", "auto-start-reverse");
  const tip = document.createElementNS(SVG_NS, "path");
  tip.setAttribute("d", "M 0 0 L 10 5 L 0 10 z");
  tip.setAttribute("class", cls);
  m.append(tip);
  return m;
}

export function drawWires(svg: SVGSVGElement, view: HTMLElement, plan: RoutePlan): void {
  const origin = view.getBoundingClientRect();
  svg.replaceChildren();
  svg.setAttribute("width", String(view.scrollWidth));
  svg.setAttribute("height", String(view.scrollHeight));
  svg.style.left = `${view.offsetLeft}px`;
  svg.style.top = `${view.offsetTop}px`;
  const defs = document.createElementNS(SVG_NS, "defs");
  defs.append(marker("arrow", "arrow-idle"), marker("arrow-lit", "arrow-lit"));
  svg.append(defs);

  const gutterX = view.clientWidth - 12;
  for (const route of plan.routes) {
    const a = anchorsOf(view, route.edge.from, origin);
    const b = anchorsOf(view, route.edge.to, origin);
    if (!a || !b) continue;
    const points: Port[] = [];
    if (route.kind === "adjacent") {
      const rightward = a.headRight.x <= b.headLeft.x;
      points.push(rightward ? a.headRight : a.headLeft, rightward ? b.headLeft : b.headRight);
    } else {
      const start = a[route.fromSide];
      const end = b[route.toSide];
      if (route.kind === "channel") {
        const y = laneY(view, route.channel, route.lane, origin);
        points.push(start, { x: start.x, y }, { x: end.x, y }, end);
      } else {
        const ya = laneY(view, route.channel, route.lane, origin);
        const yb = laneY(view, route.channel2, route.lane2, origin);
        const gx = gutterX - (route.gutterLane % 8) * LANE_GAP;
        points.push(
          start,
          { x: start.x, y: ya },
          { x: gx, y: ya },
          { x: gx, y: yb },
          { x: end.x, y: yb },
          end,
        );
      }
    }
    const path = document.createElementNS(SVG_NS, "path");
    path.setAttribute("d", roundedPath(points, 6));
    path.setAttribute("class", "wire");
    path.setAttribute("marker-end", "url(#arrow)");
    path.dataset.from = route.edge.from;
    path.dataset.to = route.edge.to;
    svg.append(path);
    if (route.kind !== "adjacent") {
      const dot = document.createElementNS(SVG_NS, "circle");
      dot.setAttribute("cx", String(points[0].x));
      dot.setAttribute("cy", String(points[0].y));
      dot.setAttribute("r", "2.5");
      dot.setAttribute("class", "wire-dot");
      dot.dataset.from = route.edge.from;
      dot.dataset.to = route.edge.to;
      svg.append(dot);
    }
  }
}
