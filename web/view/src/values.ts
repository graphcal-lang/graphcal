import type { Value } from "./model.ts";

/** Resolve the entry of an indexed value at the given labels, one per axis. */
export function valueAt(value: Value | null, labels: readonly string[]): Value | null {
  let current: Value | null = value;
  for (const label of labels) {
    if (current === null || current.kind !== "indexed") return null;
    const entry = current.entries.find((e) => e.key === label);
    current = entry ? entry.value : null;
  }
  return current;
}

export function displayOf(value: Value | null): string {
  return value === null ? "—" : value.display;
}

/** A magnitude rounded for a cell: four significant digits, no grouping. */
export function shortMagnitude(magnitude: string): string {
  const x = Number(magnitude);
  if (!Number.isFinite(x)) return magnitude;
  const rounded = Number(x.toPrecision(4));
  return rounded.toLocaleString("en-US", { maximumFractionDigits: 6, useGrouping: false });
}

/** An expression on one line: runs of whitespace become one space. */
export function oneLine(expr: string): string {
  return expr.replace(/\s+/g, " ").trim();
}
