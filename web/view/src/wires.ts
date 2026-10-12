import type { Layout } from "./layout.ts";
import type { ViewModel } from "./model.ts";

// Wires run only through band heads (the channel above each band) and the right
// gutter, never across rows. Each edge is one orthogonal path.

export interface Edge {
  from: string;
  to: string;
}

export function edgesOf(model: ViewModel, layout: Layout): Edge[] {
  const edges: Edge[] = [];
  for (const node of model.nodes) {
    for (const dep of node.deps) {
      if (layout.columns.has(dep)) edges.push({ from: dep, to: node.id });
    }
  }
  return edges;
}

const SVG_NS = "http://www.w3.org/2000/svg";

interface Anchor {
  x: number;
  top: number;
  bottom: number;
  bandIndex: number;
}

function anchorOf(view: HTMLElement, id: string, origin: DOMRect): Anchor | null {
  const head = view.querySelector<HTMLElement>(`.col-head[data-node="${id}"]`);
  if (!head) return null;
  const rect = head.getBoundingClientRect();
  const band = head.closest<HTMLElement>(".band");
  const bands = [...view.querySelectorAll<HTMLElement>(".band")];
  return {
    x: rect.left + rect.width / 2 - origin.left,
    top: rect.top - origin.top,
    bottom: rect.bottom - origin.top,
    bandIndex: band ? bands.indexOf(band) : -1,
  };
}

function channelY(view: HTMLElement, bandIndex: number, lane: number, origin: DOMRect): number {
  const band = view.querySelectorAll<HTMLElement>(".band")[bandIndex];
  const head = band?.querySelector<HTMLElement>(".band-head");
  if (!head) return 0;
  const rect = head.getBoundingClientRect();
  return rect.bottom - origin.top - 8 - lane * 5;
}

export function drawWires(svg: SVGSVGElement, view: HTMLElement, edges: Edge[]): void {
  const origin = view.getBoundingClientRect();
  svg.replaceChildren();
  svg.setAttribute("width", String(view.scrollWidth));
  svg.setAttribute("height", String(view.scrollHeight));
  svg.style.left = `${view.offsetLeft}px`;
  svg.style.top = `${view.offsetTop}px`;

  const defs = document.createElementNS(SVG_NS, "defs");
  for (const [id, cls] of [
    ["arrow", "arrow-idle"],
    ["arrow-lit", "arrow-lit"],
  ] as const) {
    const marker = document.createElementNS(SVG_NS, "marker");
    marker.setAttribute("id", id);
    marker.setAttribute("viewBox", "0 0 10 10");
    marker.setAttribute("refX", "9");
    marker.setAttribute("refY", "5");
    marker.setAttribute("markerWidth", "7");
    marker.setAttribute("markerHeight", "7");
    marker.setAttribute("orient", "auto-start-reverse");
    const tip = document.createElementNS(SVG_NS, "path");
    tip.setAttribute("d", "M 0 0 L 10 5 L 0 10 z");
    tip.setAttribute("class", cls);
    marker.append(tip);
    defs.append(marker);
  }
  svg.append(defs);

  const gutterX = view.scrollWidth - 10;
  let lane = 0;
  let gutterLane = 0;
  for (const edge of edges) {
    const a = anchorOf(view, edge.from, origin);
    const b = anchorOf(view, edge.to, origin);
    if (!a || !b) continue;
    const laneIndex = lane++ % 5;
    const points: [number, number][] = [];
    if (a.bandIndex === b.bandIndex) {
      const y = channelY(view, a.bandIndex, laneIndex, origin);
      points.push([a.x, a.top], [a.x, y], [b.x, y], [b.x, b.top]);
    } else {
      const ya = channelY(view, a.bandIndex, laneIndex, origin);
      const yb = channelY(view, b.bandIndex, laneIndex, origin);
      const gx = gutterX - (gutterLane++ % 6) * 5;
      points.push([a.x, a.top], [a.x, ya], [gx, ya], [gx, yb], [b.x, yb], [b.x, b.top]);
    }
    const path = document.createElementNS(SVG_NS, "path");
    path.setAttribute("d", points.map(([x, y], i) => `${i === 0 ? "M" : "L"}${x},${y}`).join(" "));
    path.setAttribute("class", "wire");
    path.setAttribute("marker-end", "url(#arrow)");
    path.dataset.from = edge.from;
    path.dataset.to = edge.to;
    svg.append(path);
    const dot = document.createElementNS(SVG_NS, "circle");
    dot.setAttribute("cx", String(a.x));
    dot.setAttribute("cy", String(a.top));
    dot.setAttribute("r", "3");
    dot.setAttribute("class", "wire-dot");
    dot.dataset.from = edge.from;
    dot.dataset.to = edge.to;
    svg.append(dot);
  }
}
