import type { Layout } from "./layout.ts";
import type { ViewModel } from "./model.ts";

// Wires never cross the rows of a table. They run through the channels between
// bands (and the right gutter when two bands are not adjacent); a wire between
// neighbouring columns of one band is a short arrow at header height. The
// no-axis tiers are packed together and share the channel below the last tier.

export interface Edge {
  from: string;
  to: string;
}

type Side = "top" | "bottom";

export type Route =
  | { kind: "adjacent"; edge: Edge }
  | { kind: "channel"; edge: Edge; fromSide: Side; toSide: Side; channel: number }
  | {
      kind: "gutter";
      edge: Edge;
      fromSide: Side;
      toSide: Side;
      channel: number;
      channel2: number;
      gutterLane: number;
    };

export interface RoutePlan {
  routes: Route[];
}

export const LANE_GAP = 6;
export const CHANNEL_PAD = 10;
const TIER_GAP = 8;
const EMPTY_CHANNEL = 14;

export function edgesOf(model: ViewModel, layout: Layout): Edge[] {
  const edges: Edge[] = [];
  for (const node of model.nodes) {
    for (const dep of node.deps) {
      if (layout.columns.has(dep)) edges.push({ from: dep, to: node.id });
    }
  }
  return edges;
}

export function scalarTierCount(layout: Layout): number {
  return layout.bands.filter((b) => b.axis === null).length;
}

/** Axis bands attach at the header (top); no-axis boxes always at the bottom. */
function sideOf(layout: Layout, bandIndex: number): Side {
  return layout.bands[bandIndex]?.axis === null ? "bottom" : "top";
}

/** Channel k sits above band k; the no-axis tiers all exit into the channel below the last tier. */
function channelOf(layout: Layout, bandIndex: number, side: Side): number {
  return side === "top" ? bandIndex : scalarTierCount(layout);
}

export function planRoutes(model: ViewModel, layout: Layout): RoutePlan {
  const routes: Route[] = [];
  let gutterLane = 0;
  for (const edge of edgesOf(model, layout)) {
    const from = layout.columns.get(edge.from);
    const to = layout.columns.get(edge.to);
    if (!from || !to) continue;
    if (
      from.bandIndex === to.bandIndex &&
      Math.abs(from.position - to.position) === 1 &&
      from.tierRow === 0 &&
      to.tierRow === 0
    ) {
      routes.push({ kind: "adjacent", edge });
      continue;
    }
    const fromSide = sideOf(layout, from.bandIndex);
    const toSide = sideOf(layout, to.bandIndex);
    const channel = channelOf(layout, from.bandIndex, fromSide);
    const channel2 = channelOf(layout, to.bandIndex, toSide);
    if (channel === channel2) {
      routes.push({ kind: "channel", edge, fromSide, toSide, channel });
    } else {
      routes.push({
        kind: "gutter",
        edge,
        fromSide,
        toSide,
        channel,
        channel2,
        gutterLane: gutterLane++,
      });
    }
  }
  return { routes };
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

interface Span {
  channel: number;
  x0: number;
  x1: number;
  /** Receives the assigned lane. */
  assign: (lane: number) => void;
}

/** Pack horizontal spans into lanes: spans that do not overlap share a lane. */
function packLanes(spans: Span[]): Map<number, number> {
  const byChannel = new Map<number, Span[]>();
  for (const span of spans)
    byChannel.set(span.channel, [...(byChannel.get(span.channel) ?? []), span]);
  const counts = new Map<number, number>();
  for (const [channel, list] of byChannel) {
    list.sort((a, b) => a.x0 - b.x0 || a.x1 - b.x1);
    const laneEnds: number[] = [];
    for (const span of list) {
      let lane = laneEnds.findIndex((end) => end + LANE_GAP * 2 < span.x0);
      if (lane === -1) {
        lane = laneEnds.length;
        laneEnds.push(span.x1);
      } else {
        laneEnds[lane] = span.x1;
      }
      span.assign(lane);
    }
    counts.set(channel, laneEnds.length);
  }
  return counts;
}

export function drawWires(
  svg: SVGSVGElement,
  view: HTMLElement,
  layout: Layout,
  plan: RoutePlan,
): void {
  const gutterX = view.clientWidth - 12;
  const tiers = scalarTierCount(layout);

  // Pass 1: x positions do not depend on channel heights, so pack lanes first.
  let origin = view.getBoundingClientRect();
  const lanes = new Map<Route, { lane: number; lane2: number }>();
  const spans: Span[] = [];
  for (const route of plan.routes) {
    if (route.kind === "adjacent") continue;
    const a = anchorsOf(view, route.edge.from, origin);
    const b = anchorsOf(view, route.edge.to, origin);
    if (!a || !b) continue;
    const slot = { lane: 0, lane2: 0 };
    lanes.set(route, slot);
    const sx = a[route.fromSide].x;
    const tx = b[route.toSide].x;
    if (route.kind === "channel") {
      spans.push({
        channel: route.channel,
        x0: Math.min(sx, tx),
        x1: Math.max(sx, tx),
        assign: (l) => (slot.lane = l),
      });
    } else {
      const gx = gutterX - (route.gutterLane % 8) * LANE_GAP;
      spans.push({
        channel: route.channel,
        x0: Math.min(sx, gx),
        x1: Math.max(sx, gx),
        assign: (l) => (slot.lane = l),
      });
      spans.push({
        channel: route.channel2,
        x0: Math.min(tx, gx),
        x1: Math.max(tx, gx),
        assign: (l) => (slot.lane2 = l),
      });
    }
  }
  const counts = packLanes(spans);
  for (const channel of view.querySelectorAll<HTMLElement>(".channel")) {
    const k = Number(channel.dataset.channel);
    const betweenTiers = k > 0 && k < tiers;
    const count = counts.get(k) ?? 0;
    const height = betweenTiers
      ? TIER_GAP
      : count === 0
        ? EMPTY_CHANNEL
        : CHANNEL_PAD * 2 + (count - 1) * LANE_GAP;
    channel.style.height = `${height}px`;
  }

  // Pass 2: heights are final; measure again and draw.
  origin = view.getBoundingClientRect();
  svg.replaceChildren();
  svg.setAttribute("width", String(view.scrollWidth));
  svg.setAttribute("height", String(view.scrollHeight));
  svg.style.left = `${view.offsetLeft}px`;
  svg.style.top = `${view.offsetTop}px`;
  const defs = document.createElementNS(SVG_NS, "defs");
  defs.append(marker("arrow", "arrow-idle"), marker("arrow-lit", "arrow-lit"));
  svg.append(defs);
  const laneY = (channel: number, lane: number): number => {
    const el = view.querySelector<HTMLElement>(`.channel[data-channel="${channel}"]`);
    if (!el) return 0;
    return el.getBoundingClientRect().top - origin.top + CHANNEL_PAD + lane * LANE_GAP;
  };

  for (const route of plan.routes) {
    const a = anchorsOf(view, route.edge.from, origin);
    const b = anchorsOf(view, route.edge.to, origin);
    if (!a || !b) continue;
    const points: Port[] = [];
    if (route.kind === "adjacent") {
      const rightward = a.headRight.x <= b.headLeft.x;
      points.push(rightward ? a.headRight : a.headLeft, rightward ? b.headLeft : b.headRight);
    } else {
      const slot = lanes.get(route) ?? { lane: 0, lane2: 0 };
      const start = a[route.fromSide];
      const end = b[route.toSide];
      if (route.kind === "channel") {
        const y = laneY(route.channel, slot.lane);
        points.push(start, { x: start.x, y }, { x: end.x, y }, end);
      } else {
        const ya = laneY(route.channel, slot.lane);
        const yb = laneY(route.channel2, slot.lane2);
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
